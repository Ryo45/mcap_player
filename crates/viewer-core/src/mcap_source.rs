use crate::{
    ArrivalTime, RangeQuery, RangeQueryError, RangeQueryResult, RawMessage, RecordingTimeRange,
    SourceCatalog, StreamDescriptor, StreamId, StreamTimingSummary,
};
use bytes::Bytes;
use mcap::{MessageStream, Summary};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
};

const LINEAR_FALLBACK_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum McapOpenError {
    Mcap(mcap::McapError),
    SummaryRequired(usize),
    MessageIndexRequired(StreamId),
    TimestampOverflow,
}

impl fmt::Display for McapOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mcap(error) => write!(f, "MCAP error: {error}"),
            Self::SummaryRequired(size) => write!(
                f,
                "MCAP file without a summary is too large for linear fallback ({size} bytes)"
            ),
            Self::TimestampOverflow => write!(f, "MCAP timestamp exceeds signed nanosecond range"),
            Self::MessageIndexRequired(stream) => write!(
                f,
                "MCAP Message Index is required to restore stream {}",
                stream.0
            ),
        }
    }
}

impl std::error::Error for McapOpenError {}
impl From<mcap::McapError> for McapOpenError {
    fn from(value: mcap::McapError) -> Self {
        Self::Mcap(value)
    }
}

#[derive(Clone)]
struct CachedMessage {
    arrival: ArrivalTime,
    stream_id: StreamId,
    payload: Bytes,
}

pub struct McapSource<B: AsRef<[u8]>> {
    backing: B,
    summary: Option<Summary>,
    catalog: SourceCatalog,
    range: (ArrivalTime, ArrivalTime),
    cache: Vec<CachedMessage>,
    chunk: Option<usize>,
    position: usize,
    selected_streams: Option<HashSet<StreamId>>,
    /// Sparse physical index: stream -> chunks that contain a Message Index for that stream.
    /// Individual MessageIndexEntry records remain on disk and are read only for a restore.
    message_index_chunks: HashMap<StreamId, Vec<usize>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexedReadDiagnostics {
    pub chunks_streamed: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndexedMessages {
    pub messages: Vec<RawMessage>,
    pub diagnostics: IndexedReadDiagnostics,
}

impl<B: AsRef<[u8]>> McapSource<B> {
    pub fn new(backing: B) -> Result<Self, McapOpenError> {
        let bytes = backing.as_ref();
        let summary = Summary::read(bytes)?;
        if summary.is_none() && bytes.len() > LINEAR_FALLBACK_LIMIT {
            return Err(McapOpenError::SummaryRequired(bytes.len()));
        }
        let mut catalog = SourceCatalog::default();
        if let Some(value) = &summary {
            for channel in value.channels.values() {
                catalog.streams.push(StreamDescriptor {
                    id: StreamId(u32::from(channel.id)),
                    topic: channel.topic.clone(),
                    schema: channel
                        .schema
                        .as_ref()
                        .map(|schema| schema.name.clone())
                        .unwrap_or_default(),
                    message_encoding: channel.message_encoding.clone(),
                    timing: StreamTimingSummary {
                        message_count: value.stats.as_ref().and_then(|stats| {
                            stats.channel_message_counts.get(&channel.id).copied()
                        }),
                    },
                });
            }
            catalog.streams.sort_by_key(|stream| stream.id.0);
        }
        let range = if let Some(value) = &summary {
            let (start, end) = value.stats.as_ref().map_or_else(
                || {
                    let start = value
                        .chunk_indexes
                        .iter()
                        .filter(|chunk| !chunk.message_index_offsets.is_empty())
                        .map(|chunk| chunk.message_start_time)
                        .min()
                        .unwrap_or(0);
                    let end = value
                        .chunk_indexes
                        .iter()
                        .filter(|chunk| !chunk.message_index_offsets.is_empty())
                        .map(|chunk| chunk.message_end_time)
                        .max()
                        .unwrap_or(start);
                    (start, end)
                },
                |stats| (stats.message_start_time, stats.message_end_time),
            );
            (to_arrival(start)?, to_arrival(end)?)
        } else {
            (ArrivalTime(0), ArrivalTime(0))
        };
        catalog.time_range = range
            .1
            .0
            .checked_add(1)
            .and_then(|end| RecordingTimeRange::new(range.0, ArrivalTime(end)));
        let mut message_index_chunks = HashMap::<StreamId, Vec<usize>>::new();
        if let Some(summary) = &summary {
            for (chunk_index, chunk) in summary.chunk_indexes.iter().enumerate() {
                for channel_id in chunk.message_index_offsets.keys() {
                    message_index_chunks
                        .entry(StreamId(u32::from(*channel_id)))
                        .or_default()
                        .push(chunk_index);
                }
            }
        }
        let mut source = Self {
            backing,
            summary,
            catalog,
            range,
            cache: vec![],
            chunk: None,
            position: 0,
            selected_streams: None,
            message_index_chunks,
        };
        if !source.has_chunks() {
            source.load_linear()?;
            if let (Some(first), Some(last)) = (source.cache.first(), source.cache.last()) {
                source.range = (first.arrival, last.arrival);
                source.catalog.time_range = last
                    .arrival
                    .0
                    .checked_add(1)
                    .and_then(|end| RecordingTimeRange::new(first.arrival, ArrivalTime(end)));
            }
        }
        Ok(source)
    }

    pub fn catalog(&self) -> &SourceCatalog {
        &self.catalog
    }
    pub fn time_range(&self) -> (ArrivalTime, ArrivalTime) {
        self.range
    }

    pub fn select_streams(&mut self, streams: impl IntoIterator<Item = StreamId>) {
        let selected = streams.into_iter().collect::<HashSet<_>>();
        if self.has_chunks() {
            self.cache.clear();
            self.chunk = None;
        } else {
            // Summary-less/unchunked sources are loaded once during construction. Keep that
            // bounded fallback buffer and prune it in place instead of clearing the only copy.
            self.cache
                .retain(|message| selected.contains(&message.stream_id));
        }
        self.selected_streams = Some(selected);
        self.position = 0;
    }

    pub fn seek(&mut self, cursor: ArrivalTime) -> Result<(), McapOpenError> {
        if self.has_chunks() {
            let summary = self.summary.as_ref().expect("chunk summary checked");
            let partition = summary
                .chunk_indexes
                .partition_point(|index| to_arrival_lossy(index.message_end_time) < cursor);
            let selected = partition.min(summary.chunk_indexes.len() - 1);
            let cache = self.read_chunk(selected)?;
            let position = cache.partition_point(|message| message.arrival < cursor);
            self.cache = cache;
            self.chunk = Some(selected);
            self.position = position;
        } else {
            self.load_linear()?;
            self.position = self
                .cache
                .partition_point(|message| message.arrival < cursor);
        }
        Ok(())
    }

    pub fn read_until(&mut self, cursor: ArrivalTime) -> Result<Vec<RawMessage>, McapOpenError> {
        if self.has_chunks() && self.chunk.is_none() {
            self.load_chunk(0)?;
        }
        let mut output = vec![];
        loop {
            while let Some(message) = self.cache.get(self.position) {
                if message.arrival > cursor {
                    return Ok(output);
                }
                output.push(RawMessage {
                    stream_id: message.stream_id,
                    arrival_time: message.arrival,
                    payload: message.payload.clone(),
                });
                self.position += 1;
            }
            let Some(next) = self.chunk.map(|value| value + 1) else {
                return Ok(output);
            };
            let count = self
                .summary
                .as_ref()
                .map_or(0, |summary| summary.chunk_indexes.len());
            if next >= count {
                return Ok(output);
            }
            self.load_chunk(next)?;
        }
    }

    /// Executes a bounded exact-message query without mutating this source's playback cursor.
    pub fn query_range(&self, query: &RangeQuery) -> Result<RangeQueryResult, RangeQueryError> {
        if query.streams.is_empty() {
            return Err(RangeQueryError::Invalid(
                "range query requires at least one stream".into(),
            ));
        }
        if query.limits.max_messages == 0 || query.limits.max_payload_bytes == 0 {
            return Err(RangeQueryError::Invalid(
                "range query limits must be non-zero".into(),
            ));
        }
        if let Some(unknown) = query
            .streams
            .iter()
            .find(|id| self.catalog.by_id(**id).is_none())
        {
            return Err(RangeQueryError::Invalid(format!(
                "range query references unknown stream {}",
                unknown.0
            )));
        }

        let mut source = McapSource::new(self.backing.as_ref())?;
        source.select_streams(query.streams.iter().copied());
        source.seek(query.range.start)?;
        let mut messages = Vec::new();
        let mut payload_bytes = 0_usize;
        loop {
            while let Some(message) = source.cache.get(source.position) {
                if message.arrival >= query.range.end_exclusive {
                    return Ok(RangeQueryResult {
                        messages,
                        payload_bytes,
                        complete: true,
                    });
                }
                let next_bytes = payload_bytes
                    .checked_add(message.payload.len())
                    .ok_or_else(|| {
                        RangeQueryError::Invalid("range query payload size overflow".into())
                    })?;
                if messages.len() == query.limits.max_messages
                    || next_bytes > query.limits.max_payload_bytes
                {
                    return Ok(RangeQueryResult {
                        messages,
                        payload_bytes,
                        complete: false,
                    });
                }
                messages.push(RawMessage {
                    stream_id: message.stream_id,
                    arrival_time: message.arrival,
                    payload: message.payload.clone(),
                });
                payload_bytes = next_bytes;
                source.position += 1;
            }
            let Some(next) = source.chunk.map(|value| value + 1) else {
                return Ok(RangeQueryResult {
                    messages,
                    payload_bytes,
                    complete: true,
                });
            };
            let count = source
                .summary
                .as_ref()
                .map_or(0, |summary| summary.chunk_indexes.len());
            if next >= count {
                return Ok(RangeQueryResult {
                    messages,
                    payload_bytes,
                    complete: true,
                });
            }
            source.load_chunk(next)?;
        }
    }

    /// Finds one exact predecessor per stream using MCAP Message Index records.
    ///
    /// Candidate entries are selected without decompressing chunks. Candidates are then grouped
    /// by chunk so a compressed chunk is streamed at most once for the whole multi-stream lookup.
    pub fn latest_before(
        &self,
        streams: &[StreamId],
        target: ArrivalTime,
    ) -> Result<IndexedMessages, McapOpenError> {
        let summary = self.indexed_summary(streams)?;
        let target = u64::try_from(target.0).map_err(|_| McapOpenError::TimestampOverflow)?;
        let mut index_cache =
            HashMap::<usize, HashMap<u16, Vec<mcap::records::MessageIndexEntry>>>::new();
        let mut candidates = BTreeMap::<usize, Vec<(StreamId, u64)>>::new();

        for stream in streams.iter().copied() {
            let Some(chunks) = self.message_index_chunks.get(&stream) else {
                if self
                    .catalog
                    .by_id(stream)
                    .and_then(|descriptor| descriptor.timing.message_count)
                    == Some(0)
                {
                    continue;
                }
                return Err(McapOpenError::MessageIndexRequired(stream));
            };
            for chunk_index in chunks.iter().copied().rev() {
                let chunk = &summary.chunk_indexes[chunk_index];
                if chunk.message_start_time > target {
                    continue;
                }
                let indexes = if let Some(indexes) = index_cache.get(&chunk_index) {
                    indexes
                } else {
                    let parsed = summary
                        .read_message_indexes(self.backing.as_ref(), chunk)?
                        .into_iter()
                        .map(|(channel, entries)| (channel.id, entries))
                        .collect();
                    index_cache.insert(chunk_index, parsed);
                    index_cache
                        .get(&chunk_index)
                        .expect("inserted Message Index cache entry")
                };
                let channel_id = u16::try_from(stream.0).map_err(|_| mcap::McapError::BadIndex)?;
                let Some(entries) = indexes.get(&channel_id) else {
                    continue;
                };
                let position = entries.partition_point(|entry| entry.log_time <= target);
                let Some(entry) = position.checked_sub(1).and_then(|index| entries.get(index))
                else {
                    continue;
                };
                candidates
                    .entry(chunk_index)
                    .or_default()
                    .push((stream, entry.log_time));
                break;
            }
        }

        let mut selected = HashMap::<StreamId, RawMessage>::new();
        for (chunk_index, wanted) in &candidates {
            let chunk = &summary.chunk_indexes[*chunk_index];
            for message in summary.stream_chunk(self.backing.as_ref(), chunk)? {
                let message = message?;
                let stream = StreamId(u32::from(message.channel.id));
                if wanted.iter().any(|(wanted_stream, time)| {
                    *wanted_stream == stream && *time == message.log_time
                }) {
                    selected.insert(stream, raw_message(message)?);
                }
            }
        }
        let mut messages = selected.into_values().collect::<Vec<_>>();
        messages.sort_by_key(|message| (message.arrival_time, message.stream_id.0));
        Ok(IndexedMessages {
            messages,
            diagnostics: IndexedReadDiagnostics {
                chunks_streamed: candidates.len(),
            },
        })
    }

    /// Reads an indexed, bounded history without changing sequential playback state.
    pub fn indexed_range(
        &self,
        streams: &[StreamId],
        range: crate::DataWindowTimeRange,
    ) -> Result<IndexedMessages, McapOpenError> {
        let summary = self.indexed_summary(streams)?;
        let mut chunks = HashSet::<usize>::new();
        for stream in streams.iter().copied() {
            let Some(indexes) = self.message_index_chunks.get(&stream) else {
                if self
                    .catalog
                    .by_id(stream)
                    .and_then(|value| value.timing.message_count)
                    == Some(0)
                {
                    continue;
                }
                return Err(McapOpenError::MessageIndexRequired(stream));
            };
            chunks.extend(indexes.iter().copied().filter(|index| {
                let chunk = &summary.chunk_indexes[*index];
                to_arrival_lossy(chunk.message_end_time) >= range.start
                    && to_arrival_lossy(chunk.message_start_time) < range.end_exclusive
            }));
        }
        let mut chunks = chunks.into_iter().collect::<Vec<_>>();
        chunks.sort_unstable();
        let selected = streams.iter().copied().collect::<HashSet<_>>();
        let mut messages = Vec::new();
        for chunk_index in &chunks {
            let chunk = &summary.chunk_indexes[*chunk_index];
            for message in summary.stream_chunk(self.backing.as_ref(), chunk)? {
                let message = message?;
                let stream = StreamId(u32::from(message.channel.id));
                let arrival = to_arrival(message.log_time)?;
                if selected.contains(&stream) && range.contains(arrival) {
                    messages.push(raw_message(message)?);
                }
            }
        }
        messages.sort_by_key(|message| (message.arrival_time, message.stream_id.0));
        Ok(IndexedMessages {
            messages,
            diagnostics: IndexedReadDiagnostics {
                chunks_streamed: chunks.len(),
            },
        })
    }

    /// Bootstraps explicitly persistent streams once per session.
    pub fn indexed_streams(&self, streams: &[StreamId]) -> Result<IndexedMessages, McapOpenError> {
        let recording = self.catalog.time_range.ok_or_else(|| {
            streams.first().copied().map_or(
                McapOpenError::Mcap(mcap::McapError::BadIndex),
                McapOpenError::MessageIndexRequired,
            )
        })?;
        self.indexed_range(
            streams,
            crate::DataWindowTimeRange::new(recording.start, recording.end_exclusive)
                .expect("catalog recording range is ordered"),
        )
    }

    fn indexed_summary(&self, streams: &[StreamId]) -> Result<&Summary, McapOpenError> {
        for stream in streams {
            if self.catalog.by_id(*stream).is_none() {
                return Err(McapOpenError::Mcap(mcap::McapError::BadIndex));
            }
        }
        self.summary.as_ref().ok_or_else(|| {
            streams.first().copied().map_or(
                McapOpenError::Mcap(mcap::McapError::BadIndex),
                McapOpenError::MessageIndexRequired,
            )
        })
    }

    fn load_chunk(&mut self, index: usize) -> Result<(), McapOpenError> {
        let cache = self.read_chunk(index)?;
        self.cache = cache;
        self.chunk = Some(index);
        self.position = 0;
        Ok(())
    }

    fn read_chunk(&self, index: usize) -> Result<Vec<CachedMessage>, McapOpenError> {
        let summary = self
            .summary
            .as_ref()
            .expect("chunk loading requires a summary");
        let chunk = summary
            .chunk_indexes
            .get(index)
            .ok_or(mcap::McapError::BadIndex)?;
        let mut cache = vec![];
        for message in summary.stream_chunk(self.backing.as_ref(), chunk)? {
            let message = message?;
            let stream_id = StreamId(u32::from(message.channel.id));
            if self
                .selected_streams
                .as_ref()
                .is_some_and(|streams| !streams.contains(&stream_id))
            {
                continue;
            }
            cache.push(CachedMessage {
                arrival: to_arrival(message.log_time)?,
                stream_id,
                payload: Bytes::from(message.data.into_owned()),
            });
        }
        cache.sort_by_key(|message| (message.arrival, message.stream_id.0));
        Ok(cache)
    }

    fn load_linear(&mut self) -> Result<(), McapOpenError> {
        if !self.cache.is_empty() {
            return Ok(());
        }
        let mut cache = vec![];
        for message in MessageStream::new(self.backing.as_ref())? {
            let message = message?;
            let stream_id = StreamId(u32::from(message.channel.id));
            if !self
                .catalog
                .streams
                .iter()
                .any(|stream| stream.id == StreamId(u32::from(message.channel.id)))
            {
                self.catalog.streams.push(StreamDescriptor {
                    id: StreamId(u32::from(message.channel.id)),
                    topic: message.channel.topic.clone(),
                    schema: message
                        .channel
                        .schema
                        .as_ref()
                        .map(|schema| schema.name.clone())
                        .unwrap_or_default(),
                    message_encoding: message.channel.message_encoding.clone(),
                    timing: StreamTimingSummary::default(),
                });
            }
            if self
                .selected_streams
                .as_ref()
                .is_some_and(|streams| !streams.contains(&stream_id))
            {
                continue;
            }
            cache.push(CachedMessage {
                arrival: to_arrival(message.log_time)?,
                stream_id,
                payload: Bytes::from(message.data.into_owned()),
            });
        }
        cache.sort_by_key(|message| (message.arrival, message.stream_id.0));
        self.cache = cache;
        let mut counts = std::collections::HashMap::<StreamId, u64>::new();
        for message in &self.cache {
            let count = counts.entry(message.stream_id).or_default();
            *count = count.saturating_add(1);
        }
        for stream in &mut self.catalog.streams {
            stream.timing.message_count = counts.get(&stream.id).copied();
        }
        Ok(())
    }

    fn has_chunks(&self) -> bool {
        self.summary
            .as_ref()
            .is_some_and(|summary| !summary.chunk_indexes.is_empty())
    }
}

fn to_arrival(value: u64) -> Result<ArrivalTime, McapOpenError> {
    i64::try_from(value)
        .map(ArrivalTime)
        .map_err(|_| McapOpenError::TimestampOverflow)
}
fn to_arrival_lossy(value: u64) -> ArrivalTime {
    ArrivalTime(i64::try_from(value).unwrap_or(i64::MAX))
}

fn raw_message(message: mcap::Message<'_>) -> Result<RawMessage, McapOpenError> {
    Ok(RawMessage {
        stream_id: StreamId(u32::from(message.channel.id)),
        arrival_time: to_arrival(message.log_time)?,
        payload: Bytes::from(message.data.into_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcap::{WriteOptions, Writer, records::MessageHeader};
    use std::{collections::BTreeMap, io::Cursor};

    fn indexed_fixture(chunk_size: u64, emit_message_indexes: bool) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let options = WriteOptions::new()
                .use_chunks(true)
                .chunk_size(Some(chunk_size))
                .emit_message_indexes(emit_message_indexes);
            let mut writer = Writer::with_options(&mut bytes, options).unwrap();
            let schema = writer
                .add_schema("example/msg/Value", "ros2msg", b"uint8 value\n")
                .unwrap();
            let a = writer
                .add_channel(schema, "/a", "cdr", &BTreeMap::new())
                .unwrap();
            let b = writer
                .add_channel(schema, "/b", "cdr", &BTreeMap::new())
                .unwrap();
            for (sequence, channel, time, marker) in [
                (0, a, 0, 1),
                (0, b, 0, 2),
                (1, a, 1_000_000_000, 3),
                (1, b, 80_000_000_000, 4),
                (2, a, 100_000_000_000, 5),
            ] {
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence,
                            log_time: time,
                            publish_time: time,
                        },
                        &[marker],
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        bytes.into_inner()
    }

    fn sparse_restore_fixture() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = Writer::with_options(
                &mut bytes,
                WriteOptions::new()
                    .use_chunks(true)
                    .chunk_size(Some(96))
                    .emit_message_indexes(true),
            )
            .unwrap();
            let schema = writer
                .add_schema("example/msg/Value", "ros2msg", b"uint8 value\n")
                .unwrap();
            let camera = writer
                .add_channel(schema, "/camera", "cdr", &BTreeMap::new())
                .unwrap();
            let path = writer
                .add_channel(schema, "/path", "cdr", &BTreeMap::new())
                .unwrap();
            let odometry = writer
                .add_channel(schema, "/odom", "cdr", &BTreeMap::new())
                .unwrap();
            let late = writer
                .add_channel(schema, "/late", "cdr", &BTreeMap::new())
                .unwrap();
            let second = 1_000_000_000_u64;
            let mut messages = vec![
                (camera, 0, 1),
                (camera, second, 2),
                (camera, 100 * second, 3),
                (path, 0, 10),
                (path, 80 * second, 11),
                (odometry, 0, 20),
                (odometry, 10 * second, 21),
                (odometry, 90 * second, 22),
                (late, 10 * second, 30),
            ];
            messages.sort_by_key(|(_, time, _)| *time);
            for (sequence, (channel, time, marker)) in messages.into_iter().enumerate() {
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence: sequence as u32,
                            log_time: time,
                            publish_time: time,
                        },
                        &[marker],
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        bytes.into_inner()
    }

    #[test]
    fn multi_stream_latest_before_streams_a_shared_chunk_once() {
        let source = McapSource::new(indexed_fixture(1_000_000, true)).unwrap();
        let a = source.catalog().by_topic("/a").unwrap().id;
        let b = source.catalog().by_topic("/b").unwrap().id;
        let result = source
            .latest_before(&[a, b], ArrivalTime(85_000_000_000))
            .unwrap();
        assert_eq!(result.diagnostics.chunks_streamed, 1);
        assert_eq!(
            result
                .messages
                .iter()
                .map(|message| (message.stream_id, message.arrival_time, message.payload[0]))
                .collect::<Vec<_>>(),
            vec![
                (a, ArrivalTime(1_000_000_000), 3),
                (b, ArrivalTime(80_000_000_000), 4),
            ]
        );
    }

    #[test]
    fn latest_before_is_inclusive_and_can_find_a_previous_chunk() {
        let bytes = indexed_fixture(40, true);
        let summary = Summary::read(&bytes).unwrap().unwrap();
        assert!(summary.chunk_indexes.len() > 1);
        let source = McapSource::new(bytes).unwrap();
        let a = source.catalog().by_topic("/a").unwrap().id;
        assert_eq!(
            source
                .latest_before(&[a], ArrivalTime(1_000_000_000))
                .unwrap()
                .messages[0]
                .payload[0],
            3
        );
        assert_eq!(
            source
                .latest_before(&[a], ArrivalTime(50_000_000_000))
                .unwrap()
                .messages[0]
                .payload[0],
            3
        );
        assert!(
            source
                .latest_before(&[a], ArrivalTime(-1))
                .unwrap_err()
                .to_string()
                .contains("timestamp")
        );
    }

    #[test]
    fn sparse_multi_feature_predecessors_are_independent_of_seek_history() {
        let source = McapSource::new(sparse_restore_fixture()).unwrap();
        let streams =
            ["/camera", "/path", "/odom"].map(|topic| source.catalog().by_topic(topic).unwrap().id);
        let second = 1_000_000_000_i64;
        for (target, expected) in [
            (50 * second, [2, 10, 21]),
            (85 * second, [2, 11, 21]),
            (99 * second, [2, 11, 22]),
        ] {
            let messages = source
                .latest_before(&streams, ArrivalTime(target))
                .unwrap()
                .messages;
            assert_eq!(
                streams
                    .iter()
                    .map(|stream| {
                        messages
                            .iter()
                            .find(|message| message.stream_id == *stream)
                            .unwrap()
                            .payload[0]
                    })
                    .collect::<Vec<_>>(),
                expected
            );
        }

        let late = source.catalog().by_topic("/late").unwrap().id;
        assert!(
            source
                .latest_before(&[late], ArrivalTime(5 * second))
                .unwrap()
                .messages
                .is_empty(),
            "a stream with no predecessor at T is explicitly unavailable"
        );
    }

    #[test]
    fn restore_reports_missing_message_indexes_instead_of_scanning() {
        let source = McapSource::new(indexed_fixture(128, false)).unwrap();
        let stream = source.catalog().by_topic("/a").unwrap().id;
        assert!(matches!(
            source.latest_before(&[stream], ArrivalTime(1_000_000_000)),
            Err(McapOpenError::MessageIndexRequired(found)) if found == stream
        ));
    }

    #[test]
    fn reads_summary_bearing_mcap_without_chunks_linearly() {
        let mut bytes = Cursor::new(Vec::new());
        {
            let options = WriteOptions::new().use_chunks(false);
            let mut writer = Writer::with_options(&mut bytes, options).unwrap();
            let schema = writer
                .add_schema("example/msg/Value", "ros2msg", b"uint8 value\n")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/value", "cdr", &BTreeMap::new())
                .unwrap();
            writer
                .write_to_known_channel(
                    &MessageHeader {
                        channel_id: channel,
                        sequence: 0,
                        log_time: 20,
                        publish_time: 10,
                    },
                    &[1, 2, 3],
                )
                .unwrap();
            writer
                .write_to_known_channel(
                    &MessageHeader {
                        channel_id: channel,
                        sequence: 1,
                        log_time: 21,
                        publish_time: 11,
                    },
                    &[4, 5],
                )
                .unwrap();
            writer.finish().unwrap();
        }

        let mut source = McapSource::new(bytes.into_inner()).unwrap();
        assert_eq!(source.time_range(), (ArrivalTime(20), ArrivalTime(21)));
        let stream = source.catalog().by_topic("/value").unwrap().id;
        let query = RangeQuery {
            streams: vec![stream],
            range: crate::DataWindowTimeRange::new(ArrivalTime(20), ArrivalTime(21)).unwrap(),
            limits: crate::QueryLimits::new(1, 3).unwrap(),
        };
        let result = source.query_range(&query).unwrap();
        assert!(result.complete);
        assert_eq!(result.payload_bytes, 3);
        assert_eq!(result.messages[0].payload, vec![1, 2, 3]);

        let message_limited = source
            .query_range(&RangeQuery {
                streams: vec![stream],
                range: crate::DataWindowTimeRange::new(ArrivalTime(20), ArrivalTime(22)).unwrap(),
                limits: crate::QueryLimits::new(1, 16).unwrap(),
            })
            .unwrap();
        assert!(!message_limited.complete);
        assert_eq!(message_limited.messages.len(), 1);

        let too_small = source
            .query_range(&RangeQuery {
                limits: crate::QueryLimits::new(1, 2).unwrap(),
                ..query
            })
            .unwrap();
        assert!(!too_small.complete);
        assert!(too_small.messages.is_empty());

        // The independent bounded query must not move sequential playback state.
        let messages = source.read_until(ArrivalTime(20)).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].payload, vec![1, 2, 3]);
    }
}
