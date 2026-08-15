use crate::{
    ArrivalTime, RangeQuery, RangeQueryError, RangeQueryResult, RawMessage, RecordingTimeRange,
    SourceCatalog, StreamDescriptor, StreamId, StreamTimingSummary,
};
use bytes::Bytes;
use mcap::{MessageStream, Summary};
use std::{collections::HashSet, fmt};

const LINEAR_FALLBACK_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum McapOpenError {
    Mcap(mcap::McapError),
    SummaryRequired(usize),
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
            let start = value
                .chunk_indexes
                .iter()
                .map(|chunk| chunk.message_start_time)
                .min()
                .unwrap_or(0);
            let end = value
                .chunk_indexes
                .iter()
                .map(|chunk| chunk.message_end_time)
                .max()
                .unwrap_or(start);
            (to_arrival(start)?, to_arrival(end)?)
        } else {
            (ArrivalTime(0), ArrivalTime(0))
        };
        catalog.time_range = range
            .1
            .0
            .checked_add(1)
            .and_then(|end| RecordingTimeRange::new(range.0, ArrivalTime(end)));
        let mut source = Self {
            backing,
            summary,
            catalog,
            range,
            cache: vec![],
            chunk: None,
            position: 0,
            selected_streams: None,
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

    pub(crate) fn backing_bytes(&self) -> &[u8] {
        self.backing.as_ref()
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
        self.cache.clear();
        self.position = 0;
        if self.has_chunks() {
            let summary = self.summary.as_ref().expect("chunk summary checked");
            let partition = summary
                .chunk_indexes
                .partition_point(|index| to_arrival_lossy(index.message_end_time) < cursor);
            let selected = partition.min(summary.chunk_indexes.len() - 1);
            self.load_chunk(selected)?;
            self.position = self
                .cache
                .partition_point(|message| message.arrival < cursor);
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

    fn load_chunk(&mut self, index: usize) -> Result<(), McapOpenError> {
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
        self.cache = cache;
        self.chunk = Some(index);
        self.position = 0;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use mcap::{WriteOptions, Writer, records::MessageHeader};
    use std::{collections::BTreeMap, io::Cursor};

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
