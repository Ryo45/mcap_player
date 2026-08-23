//! Native indexed MCAP access and bounded chunk ownership.

use crate::{
    ArrivalTime, IndexedChunkFact, RangeQuery, RangeQueryError, RangeQueryResult, RawMessage,
    RecordingTimeRange, SourceCapabilities, SourceCatalog, StreamDescriptor, StreamId,
    StreamTimingSummary, ensure_indexed, history_candidate_chunks, latest_candidate_chunks,
};
use bytes::Bytes;
use mcap::{Summary, records::op};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt,
    io::Read,
};

#[derive(Debug)]
pub enum McapOpenError {
    Mcap(mcap::McapError),
    SummaryRequired,
    ChunkIndexRequired,
    MessageIndexRequired(StreamId),
    InvalidChunk(String),
    UnsupportedCompression(String),
    TimestampOverflow,
}

impl fmt::Display for McapOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mcap(error) => write!(f, "MCAP error: {error}"),
            Self::SummaryRequired => write!(f, "MCAP Summary is required for indexed playback"),
            Self::ChunkIndexRequired => {
                write!(f, "MCAP Chunk Index is required for indexed playback")
            }
            Self::TimestampOverflow => write!(f, "MCAP timestamp exceeds signed nanosecond range"),
            Self::MessageIndexRequired(stream) => write!(
                f,
                "MCAP Message Index is required to restore stream {}",
                stream.0
            ),
            Self::InvalidChunk(reason) => write!(f, "invalid MCAP Chunk: {reason}"),
            Self::UnsupportedCompression(compression) => {
                write!(f, "unsupported MCAP Chunk compression: {compression}")
            }
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

pub struct McapSource {
    backing: Bytes,
    summary: Summary,
    catalog: SourceCatalog,
    range: (ArrivalTime, ArrivalTime),
    cache: Vec<CachedMessage>,
    chunk: Option<usize>,
    position: usize,
    selected_streams: Option<HashSet<StreamId>>,
    index_facts: Vec<IndexedChunkFact>,
}

pub(crate) struct McapSourcePosition {
    cache: Vec<CachedMessage>,
    chunk: Option<usize>,
    position: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexedReadDiagnostics {
    pub chunks_streamed: usize,
    /// Payload bytes copied after decompression while constructing individual RawMessages.
    pub per_message_copied_bytes: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndexedMessages {
    pub messages: Vec<RawMessage>,
    pub diagnostics: IndexedReadDiagnostics,
}

impl McapSource {
    /// Copies an arbitrary borrowed input once into source-owned backing.
    ///
    /// Native mmap callers should use [`Self::from_owner`] so the file mapping itself becomes the
    /// shared backing and this construction copy is avoided.
    pub fn new(backing: impl AsRef<[u8]>) -> Result<Self, McapOpenError> {
        Self::from_bytes(Bytes::copy_from_slice(backing.as_ref()))
    }

    pub fn from_owner<B>(backing: B) -> Result<Self, McapOpenError>
    where
        B: AsRef<[u8]> + Send + 'static,
    {
        Self::from_bytes(Bytes::from_owner(backing))
    }

    pub fn from_bytes(backing: Bytes) -> Result<Self, McapOpenError> {
        let bytes = backing.as_ref();
        let summary = Summary::read(bytes)?.ok_or(McapOpenError::SummaryRequired)?;
        if summary.chunk_indexes.is_empty() {
            return Err(McapOpenError::ChunkIndexRequired);
        }
        let mut catalog = SourceCatalog {
            capabilities: SourceCapabilities::INDEXED_RECORDING,
            ..SourceCatalog::default()
        };
        {
            let value = &summary;
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
        let range = {
            let value = &summary;
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
        };
        catalog.time_range = range
            .1
            .0
            .checked_add(1)
            .and_then(|end| RecordingTimeRange::new(range.0, ArrivalTime(end)));
        let index_facts = summary
            .chunk_indexes
            .iter()
            .map(|chunk| IndexedChunkFact {
                start: to_arrival_lossy(chunk.message_start_time),
                end_inclusive: to_arrival_lossy(chunk.message_end_time),
                indexed_streams: chunk
                    .message_index_offsets
                    .keys()
                    .map(|channel| StreamId(u32::from(*channel)))
                    .collect::<BTreeSet<_>>(),
            })
            .collect();
        Ok(Self {
            backing,
            summary,
            catalog,
            range,
            cache: vec![],
            chunk: None,
            position: 0,
            selected_streams: None,
            index_facts,
        })
    }

    pub fn catalog(&self) -> &SourceCatalog {
        &self.catalog
    }
    pub fn time_range(&self) -> (ArrivalTime, ArrivalTime) {
        self.range
    }

    pub fn select_streams(
        &mut self,
        streams: impl IntoIterator<Item = StreamId>,
    ) -> Result<(), McapOpenError> {
        let selected = streams.into_iter().collect::<HashSet<_>>();
        for stream in &selected {
            let descriptor = self
                .catalog
                .by_id(*stream)
                .ok_or(mcap::McapError::BadIndex)?;
            ensure_indexed(&self.index_facts, *stream, descriptor.timing.message_count)
                .map_err(|error| McapOpenError::MessageIndexRequired(error.stream))?;
        }
        self.cache.clear();
        self.chunk = None;
        self.selected_streams = Some(selected);
        self.position = 0;
        Ok(())
    }

    pub fn seek(&mut self, cursor: ArrivalTime) -> Result<(), McapOpenError> {
        let position = self.prepare_seek(cursor)?;
        self.commit_seek(position);
        Ok(())
    }

    pub(crate) fn prepare_seek(
        &self,
        cursor: ArrivalTime,
    ) -> Result<McapSourcePosition, McapOpenError> {
        let partition = self
            .summary
            .chunk_indexes
            .partition_point(|index| to_arrival_lossy(index.message_end_time) < cursor);
        let selected = partition.min(self.summary.chunk_indexes.len() - 1);
        let cache = self.read_chunk(selected)?;
        let position = cache.partition_point(|message| message.arrival < cursor);
        Ok(McapSourcePosition {
            cache,
            chunk: Some(selected),
            position,
        })
    }

    pub(crate) fn commit_seek(&mut self, position: McapSourcePosition) {
        self.cache = position.cache;
        self.chunk = position.chunk;
        self.position = position.position;
    }

    pub fn read_until(&mut self, cursor: ArrivalTime) -> Result<Vec<RawMessage>, McapOpenError> {
        if self.chunk.is_none() {
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
            let count = self.summary.chunk_indexes.len();
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

        let mut source = McapSource::from_bytes(self.backing.clone())?;
        source.select_streams(query.streams.iter().copied())?;
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
            let count = source.summary.chunk_indexes.len();
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
            let count = self
                .catalog
                .by_id(stream)
                .and_then(|descriptor| descriptor.timing.message_count);
            for chunk_index in latest_candidate_chunks(
                &self.index_facts,
                stream,
                count,
                ArrivalTime(i64::try_from(target).unwrap_or(i64::MAX)),
            )
            .map_err(|error| McapOpenError::MessageIndexRequired(error.stream))?
            {
                let chunk = &summary.chunk_indexes[chunk_index];
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
                    .push((stream, entry.offset));
                break;
            }
        }

        let mut selected = HashMap::<StreamId, RawMessage>::new();
        for (chunk_index, wanted) in &candidates {
            for message in self.parse_chunk(*chunk_index)? {
                if wanted.iter().any(|(wanted_stream, offset)| {
                    *wanted_stream == message.message.stream_id && *offset == message.offset
                }) {
                    selected.insert(message.message.stream_id, message.message);
                }
            }
        }
        let mut messages = selected.into_values().collect::<Vec<_>>();
        messages.sort_by_key(|message| (message.arrival_time, message.stream_id.0));
        Ok(IndexedMessages {
            messages,
            diagnostics: IndexedReadDiagnostics {
                chunks_streamed: candidates.len(),
                per_message_copied_bytes: 0,
            },
        })
    }

    /// Reads an indexed, bounded history without changing sequential playback state.
    pub fn indexed_range(
        &self,
        streams: &[StreamId],
        range: crate::DataWindowTimeRange,
    ) -> Result<IndexedMessages, McapOpenError> {
        self.indexed_summary(streams)?;
        let mut chunks = HashSet::<usize>::new();
        for stream in streams.iter().copied() {
            let count = self
                .catalog
                .by_id(stream)
                .and_then(|value| value.timing.message_count);
            chunks.extend(
                history_candidate_chunks(&self.index_facts, stream, count, range)
                    .map_err(|error| McapOpenError::MessageIndexRequired(error.stream))?,
            );
        }
        let mut chunks = chunks.into_iter().collect::<Vec<_>>();
        chunks.sort_unstable();
        let selected = streams.iter().copied().collect::<HashSet<_>>();
        let mut messages = Vec::new();
        for chunk_index in &chunks {
            for message in self.parse_chunk(*chunk_index)? {
                if selected.contains(&message.message.stream_id)
                    && range.contains(message.message.arrival_time)
                {
                    messages.push(message.message);
                }
            }
        }
        messages.sort_by_key(|message| (message.arrival_time, message.stream_id.0));
        Ok(IndexedMessages {
            messages,
            diagnostics: IndexedReadDiagnostics {
                chunks_streamed: chunks.len(),
                per_message_copied_bytes: 0,
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
        Ok(&self.summary)
    }

    fn load_chunk(&mut self, index: usize) -> Result<(), McapOpenError> {
        let cache = self.read_chunk(index)?;
        self.cache = cache;
        self.chunk = Some(index);
        self.position = 0;
        Ok(())
    }

    fn read_chunk(&self, index: usize) -> Result<Vec<CachedMessage>, McapOpenError> {
        let mut cache = Vec::new();
        for message in self.parse_chunk(index)? {
            let stream_id = message.message.stream_id;
            if self
                .selected_streams
                .as_ref()
                .is_some_and(|streams| !streams.contains(&stream_id))
            {
                continue;
            }
            cache.push(CachedMessage {
                arrival: message.message.arrival_time,
                stream_id,
                payload: message.message.payload,
            });
        }
        cache.sort_by_key(|message| (message.arrival, message.stream_id.0));
        Ok(cache)
    }

    fn parse_chunk(&self, index: usize) -> Result<Vec<ParsedChunkMessage>, McapOpenError> {
        let chunk = self
            .summary
            .chunk_indexes
            .get(index)
            .ok_or(mcap::McapError::BadIndex)?;
        let compressed_offset = usize::try_from(chunk.compressed_data_offset()?)
            .map_err(|_| McapOpenError::InvalidChunk("data offset is too large".into()))?;
        let compressed_size = usize::try_from(chunk.compressed_size)
            .map_err(|_| McapOpenError::InvalidChunk("compressed size is too large".into()))?;
        let compressed_end = compressed_offset
            .checked_add(compressed_size)
            .ok_or_else(|| McapOpenError::InvalidChunk("data range overflow".into()))?;
        if compressed_end > self.backing.len() {
            return Err(McapOpenError::InvalidChunk(
                "compressed data range exceeds source backing".into(),
            ));
        }
        let compressed = self.backing.slice(compressed_offset..compressed_end);
        let expected_size = usize::try_from(chunk.uncompressed_size)
            .map_err(|_| McapOpenError::InvalidChunk("uncompressed size is too large".into()))?;
        let backing = decode_chunk_backing(&chunk.compression, compressed, expected_size)?;
        let uncompressed_crc = self.chunk_uncompressed_crc(chunk)?;
        if uncompressed_crc != 0 && crc32fast::hash(&backing) != uncompressed_crc {
            return Err(McapOpenError::InvalidChunk(
                "uncompressed CRC does not match Chunk header".into(),
            ));
        }
        parse_chunk_backing(backing)
    }

    fn chunk_uncompressed_crc(
        &self,
        chunk: &mcap::records::ChunkIndex,
    ) -> Result<u32, McapOpenError> {
        let offset = usize::try_from(chunk.chunk_start_offset)
            .map_err(|_| McapOpenError::InvalidChunk("Chunk record offset is too large".into()))?;
        let header_end = offset
            .checked_add(9)
            .ok_or_else(|| McapOpenError::InvalidChunk("Chunk record header overflow".into()))?;
        if header_end > self.backing.len() || self.backing[offset] != op::CHUNK {
            return Err(McapOpenError::InvalidChunk(
                "Chunk Index does not point to a Chunk record".into(),
            ));
        }
        let body_length = usize::try_from(u64::from_le_bytes(
            self.backing[offset + 1..header_end]
                .try_into()
                .expect("Chunk record length has eight bytes"),
        ))
        .map_err(|_| McapOpenError::InvalidChunk("Chunk record is too large".into()))?;
        let body_end = header_end
            .checked_add(body_length)
            .ok_or_else(|| McapOpenError::InvalidChunk("Chunk record overflow".into()))?;
        if body_end > self.backing.len() {
            return Err(McapOpenError::InvalidChunk("truncated Chunk record".into()));
        }
        let record = mcap::parse_record(op::CHUNK, &self.backing[header_end..body_end])?;
        let mcap::records::Record::Chunk { header, .. } = record else {
            return Err(McapOpenError::InvalidChunk(
                "Chunk opcode parsed as another record".into(),
            ));
        };
        Ok(header.uncompressed_crc)
    }
}

struct ParsedChunkMessage {
    offset: u64,
    message: RawMessage,
}

fn decode_chunk_backing(
    compression: &str,
    compressed: Bytes,
    expected_size: usize,
) -> Result<Bytes, McapOpenError> {
    let decompressed = match compression {
        "" => {
            if compressed.len() != expected_size {
                return Err(McapOpenError::InvalidChunk(
                    "uncompressed data size does not match Chunk Index".into(),
                ));
            }
            return Ok(compressed);
        }
        #[cfg(feature = "mcap-zstd")]
        "zstd" => zstd::bulk::decompress(&compressed, expected_size)
            .map_err(|error| McapOpenError::InvalidChunk(error.to_string()))?,
        #[cfg(feature = "mcap-lz4")]
        "lz4" => {
            let mut output = Vec::with_capacity(expected_size);
            lz4::Decoder::new(std::io::Cursor::new(compressed))
                .map_err(|error| McapOpenError::InvalidChunk(error.to_string()))?
                .read_to_end(&mut output)
                .map_err(|error| McapOpenError::InvalidChunk(error.to_string()))?;
            output
        }
        value => return Err(McapOpenError::UnsupportedCompression(value.into())),
    };
    if decompressed.len() != expected_size {
        return Err(McapOpenError::InvalidChunk(
            "decompressed data size does not match Chunk Index".into(),
        ));
    }
    Ok(Bytes::from(decompressed))
}

fn parse_chunk_backing(backing: Bytes) -> Result<Vec<ParsedChunkMessage>, McapOpenError> {
    let mut offset = 0_usize;
    let mut messages = Vec::new();
    while offset < backing.len() {
        let record_offset = u64::try_from(offset)
            .map_err(|_| McapOpenError::InvalidChunk("record offset overflow".into()))?;
        let header_end = offset
            .checked_add(9)
            .ok_or_else(|| McapOpenError::InvalidChunk("record header overflow".into()))?;
        if header_end > backing.len() {
            return Err(McapOpenError::InvalidChunk(
                "truncated record header".into(),
            ));
        }
        let opcode = backing[offset];
        let body_length = usize::try_from(u64::from_le_bytes(
            backing[offset + 1..header_end]
                .try_into()
                .expect("record length has eight bytes"),
        ))
        .map_err(|_| McapOpenError::InvalidChunk("record body is too large".into()))?;
        let body_end = header_end
            .checked_add(body_length)
            .ok_or_else(|| McapOpenError::InvalidChunk("record body overflow".into()))?;
        if body_end > backing.len() {
            return Err(McapOpenError::InvalidChunk("truncated record body".into()));
        }
        if opcode == op::MESSAGE {
            let record = mcap::parse_record(opcode, &backing[header_end..body_end])?;
            let mcap::records::Record::Message { header, data } = record else {
                return Err(McapOpenError::InvalidChunk(
                    "message opcode parsed as another record".into(),
                ));
            };
            let payload_start = body_end
                .checked_sub(data.len())
                .ok_or_else(|| McapOpenError::InvalidChunk("payload range underflow".into()))?;
            messages.push(ParsedChunkMessage {
                offset: record_offset,
                message: RawMessage {
                    stream_id: StreamId(u32::from(header.channel_id)),
                    arrival_time: to_arrival(header.log_time)?,
                    payload: backing.slice(payload_start..body_end),
                },
            });
        }
        offset = body_end;
    }
    Ok(messages)
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
    use mcap::{Compression, WriteOptions, Writer, records::MessageHeader};
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

    fn payload_fixture(compression: Option<Compression>) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = Writer::with_options(
                &mut bytes,
                WriteOptions::new()
                    .use_chunks(true)
                    .compression(compression)
                    .chunk_size(Some(1_000_000))
                    .emit_message_indexes(true),
            )
            .unwrap();
            let schema = writer
                .add_schema("example/msg/Value", "ros2msg", b"uint8 value\n")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/camera", "cdr", &BTreeMap::new())
                .unwrap();
            for sequence in 0..3 {
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence,
                            log_time: u64::from(sequence),
                            publish_time: u64::from(sequence),
                        },
                        &[sequence as u8; 128],
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
        assert_eq!(
            source.catalog().capabilities,
            SourceCapabilities::INDEXED_RECORDING
        );
        let a = source.catalog().by_topic("/a").unwrap().id;
        let b = source.catalog().by_topic("/b").unwrap().id;
        let result = source
            .latest_before(&[a, b], ArrivalTime(85_000_000_000))
            .unwrap();
        assert_eq!(result.diagnostics.chunks_streamed, 1);
        assert_eq!(result.diagnostics.per_message_copied_bytes, 0);
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
    fn native_uncompressed_raw_messages_slice_the_source_owner_without_payload_copies() {
        let owner = Bytes::from(payload_fixture(None));
        let source_start = owner.as_ptr() as usize;
        let source_end = source_start + owner.len();
        let mut source = McapSource::from_bytes(owner).unwrap();
        let camera = source.catalog().by_topic("/camera").unwrap().id;
        source.select_streams([camera]).unwrap();
        let messages = source.read_until(source.time_range().1).unwrap();

        assert_eq!(messages.len(), 3);
        for message in &messages {
            let start = message.payload.as_ptr() as usize;
            let end = start + message.payload.len();
            assert!(source_start <= start && end <= source_end);
        }
    }

    #[test]
    fn native_zstd_messages_share_one_decompressed_chunk_backing() {
        let mut source = McapSource::from_owner(indexed_fixture(1_000_000, true)).unwrap();
        let streams = ["/a", "/b"].map(|topic| source.catalog().by_topic(topic).unwrap().id);
        source.select_streams(streams).unwrap();
        let messages = source.read_until(source.time_range().1).unwrap();
        let chunk_size =
            usize::try_from(source.summary.chunk_indexes[0].uncompressed_size).unwrap();
        let first = messages
            .iter()
            .map(|message| message.payload.as_ptr() as usize)
            .min()
            .unwrap();
        let last = messages
            .iter()
            .map(|message| message.payload.as_ptr() as usize + message.payload.len())
            .max()
            .unwrap();

        assert!(messages.len() > 1);
        assert!(last - first <= chunk_size);
    }

    #[test]
    fn native_lz4_messages_share_one_decompressed_chunk_backing() {
        let mut source = McapSource::from_owner(payload_fixture(Some(Compression::Lz4))).unwrap();
        let camera = source.catalog().by_topic("/camera").unwrap().id;
        source.select_streams([camera]).unwrap();
        let messages = source.read_until(source.time_range().1).unwrap();
        let chunk_size =
            usize::try_from(source.summary.chunk_indexes[0].uncompressed_size).unwrap();
        let first = messages
            .iter()
            .map(|message| message.payload.as_ptr() as usize)
            .min()
            .unwrap();
        let last = messages
            .iter()
            .map(|message| message.payload.as_ptr() as usize + message.payload.len())
            .max()
            .unwrap();

        assert_eq!(messages.len(), 3);
        assert!(last - first <= chunk_size);
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
        let mut source = McapSource::new(indexed_fixture(128, false)).unwrap();
        let stream = source.catalog().by_topic("/a").unwrap().id;
        assert!(matches!(
            source.select_streams([stream]),
            Err(McapOpenError::MessageIndexRequired(found)) if found == stream
        ));
        assert!(matches!(
            source.latest_before(&[stream], ArrivalTime(1_000_000_000)),
            Err(McapOpenError::MessageIndexRequired(found)) if found == stream
        ));
    }

    #[test]
    fn rejects_summary_bearing_mcap_without_chunk_indexes_at_open() {
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

        assert!(matches!(
            McapSource::new(bytes.into_inner()),
            Err(McapOpenError::ChunkIndexRequired)
        ));
    }
}
