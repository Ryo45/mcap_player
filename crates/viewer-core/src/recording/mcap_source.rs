//! Native indexed MCAP access and bounded chunk ownership.

use crate::{
    ArrivalTime, IndexedChunkFact, RangeQuery, RangeQueryError, RangeQueryResult, RawMessage,
    RecordingTimeRange, SourceCapabilities, SourceCatalog, StreamDescriptor, StreamId,
    StreamTimingSummary, ensure_indexed, history_candidate_chunks,
};
use bytes::Bytes;
use mcap::{Summary, records::op};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt,
};

#[derive(Debug)]
pub enum McapOpenError {
    Mcap(mcap::McapError),
    SummaryRequired,
    ChunkIndexRequired,
    MessageIndexRequired(crate::IndexedPlanError),
    Decompression {
        format: &'static str,
        source: std::io::Error,
    },
    InvalidChunk(String),
    UnsupportedCompression(String),
    TimestampOverflow,
    QueryCancelled,
    Io(std::io::Error),
    SourceChanged,
    NotRegularFile,
    ReadLockPoisoned,
    ReadOverflow,
    ReadOutOfBounds {
        offset: u64,
        length: usize,
        file_size: u64,
    },
    ReadLimit {
        requested: usize,
        maximum: usize,
    },
}

impl fmt::Display for McapOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mcap(error) => write!(f, "MCAP error: {error}"),
            Self::SummaryRequired => write!(f, "MCAP Summary is required for indexed playback"),
            Self::ChunkIndexRequired => {
                write!(f, "MCAP Chunk Index is required for indexed playback")
            }
            Self::Io(error) => write!(f, "source I/O: {error}"),
            Self::SourceChanged => write!(f, "recording changed after open"),
            Self::NotRegularFile => write!(f, "recording is not a regular file"),
            Self::ReadLockPoisoned => write!(f, "source read lock was poisoned"),
            Self::ReadOverflow => write!(f, "source range arithmetic overflow"),
            Self::ReadOutOfBounds {
                offset,
                length,
                file_size,
            } => write!(
                f,
                "source range {offset}+{length} exceeds {file_size} bytes"
            ),
            Self::ReadLimit { requested, maximum } => write!(
                f,
                "source record of {requested} bytes exceeds {maximum} byte limit"
            ),
            Self::QueryCancelled => write!(f, "recording query cancelled"),
            Self::TimestampOverflow => write!(f, "MCAP timestamp exceeds signed nanosecond range"),
            Self::Decompression { format, source } => write!(f, "{format} decompression: {source}"),
            Self::MessageIndexRequired(error) => write!(
                f,
                "MCAP Message Index is required to restore stream {}",
                error.stream.0
            ),
            Self::InvalidChunk(reason) => write!(f, "invalid MCAP Chunk: {reason}"),
            Self::UnsupportedCompression(compression) => {
                write!(f, "unsupported MCAP Chunk compression: {compression}")
            }
        }
    }
}

impl std::error::Error for McapOpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mcap(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Decompression { source, .. } => Some(source),
            Self::MessageIndexRequired(error) => Some(error),
            _ => None,
        }
    }
}
impl From<super::McapChunkError> for McapOpenError {
    fn from(error: super::McapChunkError) -> Self {
        match error {
            super::McapChunkError::Mcap(error) => Self::Mcap(error),
            super::McapChunkError::InvalidRecord(reason) => Self::InvalidChunk(reason.into()),
            super::McapChunkError::TimestampOverflow => Self::TimestampOverflow,
            super::McapChunkError::UnsupportedCompression(format) => {
                Self::UnsupportedCompression(format)
            }
            super::McapChunkError::Decompression { format, source } => {
                Self::Decompression { format, source }
            }
        }
    }
}
impl From<mcap::McapError> for McapOpenError {
    fn from(value: mcap::McapError) -> Self {
        Self::Mcap(value)
    }
}

#[derive(Clone)]
struct CachedMessage {
    publish_time_ns: Option<u64>,
    sequence: Option<u32>,
    arrival: ArrivalTime,
    stream_id: StreamId,
    payload: Bytes,
}

pub struct McapSource {
    backing: super::backing::Backing,
    summary: std::sync::Arc<Summary>,
    catalog: std::sync::Arc<SourceCatalog>,
    range: RecordingTimeRange,
    cache: Vec<CachedMessage>,
    chunk: Option<usize>,
    position: usize,
    selected_streams: Option<HashSet<StreamId>>,
    index_facts: std::sync::Arc<[IndexedChunkFact]>,
    stream_chunks: std::sync::Arc<HashMap<StreamId, StreamChunks>>,
    #[cfg(test)]
    index_metrics: std::sync::Arc<IndexReadMetrics>,
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
    /// Explicitly copies the supplied bytes once. Prefer `from_bytes` for an existing owner.
    pub fn copy_from_slice(backing: &[u8]) -> Result<Self, McapOpenError> {
        Self::from_bytes(Bytes::copy_from_slice(backing))
    }
    pub fn from_bytes(backing: Bytes) -> Result<Self, McapOpenError> {
        Self::from_backing(super::backing::Backing::Bytes(backing))
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_file(path: &std::path::Path) -> Result<Self, McapOpenError> {
        Self::from_backing(super::backing::Backing::File(std::sync::Arc::new(
            super::backing::FileRanges::open(path)?,
        )))
    }
    fn from_backing(backing: super::backing::Backing) -> Result<Self, McapOpenError> {
        let summary = backing.summary()?;
        if summary.chunk_indexes.is_empty() {
            return Err(McapOpenError::ChunkIndexRequired);
        }
        let schema_definitions = summary
            .schemas
            .iter()
            .map(|(id, schema)| (*id, crate::SchemaDefinition::from_mcap(schema)))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut catalog = SourceCatalog {
            capabilities: SourceCapabilities::INDEXED_RECORDING,
            ..SourceCatalog::default()
        };
        {
            let value = &summary;
            for channel in value.channels.values() {
                catalog.streams.push(StreamDescriptor {
                    schema_definition: channel
                        .schema
                        .as_ref()
                        .and_then(|schema| schema_definitions.get(&schema.id).cloned()),
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
        let end_exclusive = range
            .1
            .0
            .checked_add(1)
            .ok_or(McapOpenError::TimestampOverflow)?;
        let range = RecordingTimeRange::new(range.0, ArrivalTime(end_exclusive))
            .ok_or(McapOpenError::TimestampOverflow)?;
        catalog.time_range = Some(range);
        let index_facts: Vec<_> = summary
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
        let mut stream_chunks = HashMap::<StreamId, StreamChunks>::new();
        for (chunk, fact) in index_facts.iter().enumerate() {
            for stream in &fact.indexed_streams {
                stream_chunks.entry(*stream).or_default().chunks.push(chunk);
            }
        }
        for index in stream_chunks.values_mut() {
            index
                .chunks
                .sort_by_key(|chunk| (index_facts[*chunk].start, *chunk));
            let mut max = ArrivalTime(i64::MIN);
            let mut max_chunk = 0;
            for chunk in &index.chunks {
                max = max.max(index_facts[*chunk].end_inclusive);
                index.prefix_max_end.push(max);
                max_chunk = max_chunk.max(*chunk);
                index.prefix_max_chunk.push(max_chunk);
            }
        }
        Ok(Self {
            backing,
            summary: std::sync::Arc::new(summary),
            catalog: std::sync::Arc::new(catalog),
            range,
            cache: vec![],
            chunk: None,
            position: 0,
            selected_streams: None,
            index_facts: std::sync::Arc::from(index_facts),
            stream_chunks: std::sync::Arc::new(stream_chunks),
            #[cfg(test)]
            index_metrics: Default::default(),
        })
    }

    /// Independent traversal over the same bytes/file handle and summary facts.
    pub fn fork_reader(&self) -> Self {
        Self {
            backing: self.backing.clone(),
            summary: self.summary.clone(),
            catalog: self.catalog.clone(),
            range: self.range,
            cache: vec![],
            chunk: None,
            position: 0,
            selected_streams: None,
            index_facts: self.index_facts.clone(),
            stream_chunks: self.stream_chunks.clone(),
            #[cfg(test)]
            index_metrics: self.index_metrics.clone(),
        }
    }
    pub fn summary(&self) -> &Summary {
        &self.summary
    }
    pub fn summary_identity(&self) -> Result<crate::McapSummaryIdentity, McapOpenError> {
        let stats = self
            .summary
            .stats
            .as_ref()
            .ok_or(McapOpenError::SummaryRequired)?;
        Ok(crate::McapSummaryIdentity {
            file_size: self.backing.len(),
            summary_crc: self.backing.summary_crc()?,
            message_start_time: stats.message_start_time,
            message_end_time: stats.message_end_time,
            message_count: stats.message_count,
            schema_count: stats.schema_count,
            channel_count: stats.channel_count,
            chunk_count: self.summary.chunk_indexes.len(),
        })
    }
    pub fn catalog(&self) -> &SourceCatalog {
        &self.catalog
    }
    pub fn time_range(&self) -> RecordingTimeRange {
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
                .map_err(McapOpenError::MessageIndexRequired)?;
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
        if self
            .selected_streams
            .as_ref()
            .is_some_and(HashSet::is_empty)
        {
            return Ok(McapSourcePosition {
                cache: Vec::new(),
                chunk: None,
                position: 0,
            });
        }
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

    /// Inclusive delivery, committed only after every required physical read succeeds.
    pub fn read_through(&mut self, cursor: ArrivalTime) -> Result<Vec<RawMessage>, McapOpenError> {
        if self
            .selected_streams
            .as_ref()
            .is_some_and(HashSet::is_empty)
        {
            return Ok(Vec::new());
        }
        let mut chunk = self.chunk;
        let mut position = self.position;
        let mut candidate_cache = None;
        if chunk.is_none() {
            candidate_cache = Some(self.read_chunk(0)?);
            chunk = Some(0);
            position = 0;
        }
        let mut output = Vec::new();
        loop {
            let cache = candidate_cache.as_ref().unwrap_or(&self.cache);
            while let Some(message) = cache.get(position) {
                if message.arrival > cursor {
                    break;
                }
                output.push(RawMessage {
                    stream_id: message.stream_id,
                    arrival_time: message.arrival,
                    publish_time_ns: message.publish_time_ns,
                    sequence: message.sequence,
                    payload: message.payload.clone(),
                });
                position += 1;
            }
            if cache.get(position).is_some() {
                break;
            }
            let next = chunk.expect("initial chunk staged") + 1;
            if next >= self.summary.chunk_indexes.len()
                || to_arrival_lossy(self.summary.chunk_indexes[next].message_start_time) > cursor
            {
                break;
            }
            candidate_cache = Some(self.read_chunk(next)?);
            chunk = Some(next);
            position = 0;
        }
        if let Some(cache) = candidate_cache {
            self.cache = cache;
        }
        self.chunk = chunk;
        self.position = position;
        Ok(output)
    }

    /// Concrete streaming I/O for numeric reduction, independent of playback selection/cursor.
    pub(crate) fn visit_stream_range(
        &self,
        stream: StreamId,
        range: crate::DataWindowTimeRange,
        mut cancelled: impl FnMut() -> bool,
        mut visit: impl FnMut(&RawMessage),
    ) -> Result<usize, McapOpenError> {
        let descriptor = self
            .catalog
            .by_id(stream)
            .ok_or(mcap::McapError::BadIndex)?;
        ensure_indexed(&self.index_facts, stream, descriptor.timing.message_count)
            .map_err(McapOpenError::MessageIndexRequired)?;
        let mut chunks_read = 0;
        // Inspect index facts before touching compressed payloads. No unrelated chunk scan.
        for (index, fact) in self.index_facts.iter().enumerate() {
            if cancelled() {
                return Err(McapOpenError::QueryCancelled);
            }
            if !fact.indexed_streams.contains(&stream)
                || fact.end_inclusive < range.start
                || fact.start >= range.end_exclusive
            {
                continue;
            }
            let parsed_chunk = self.parse_chunk_cancellable(index, &mut cancelled)?;
            chunks_read += 1;
            for parsed in parsed_chunk {
                let message = parsed.message;
                if message.stream_id != stream || !range.contains(message.arrival_time) {
                    continue;
                }
                if cancelled() {
                    return Err(McapOpenError::QueryCancelled);
                }
                visit(&message);
            }
        }
        Ok(chunks_read)
    }

    /// Executes a bounded exact-message query without mutating this source's playback cursor.
    pub fn query_range(&self, query: &RangeQuery) -> Result<RangeQueryResult, RangeQueryError> {
        if query.range.start >= query.range.end_exclusive {
            return Err(RangeQueryError::InvalidRange(query.range));
        }
        if query.streams.is_empty() {
            return Err(RangeQueryError::EmptySelection);
        }
        if query.limits.max_messages == 0 || query.limits.max_payload_bytes == 0 {
            return Err(RangeQueryError::ZeroLimits {
                max_messages: query.limits.max_messages,
                max_payload_bytes: query.limits.max_payload_bytes,
            });
        }
        if let Some(unknown) = query
            .streams
            .iter()
            .find(|id| self.catalog.by_id(**id).is_none())
        {
            return Err(RangeQueryError::UnknownStream(*unknown));
        }

        let mut source = self.fork_reader();
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
                    .ok_or_else(|| RangeQueryError::PayloadBytesOverflow)?;
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
                    publish_time_ns: message.publish_time_ns,
                    sequence: message.sequence,
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
        self.indexed_summary(streams)?;
        let target = u64::try_from(target.0).map_err(|_| McapOpenError::TimestampOverflow)?;
        let mut index_cache = HashMap::new();
        let mut candidates = BTreeMap::<usize, Vec<(StreamId, u64)>>::new();
        for stream in streams.iter().copied() {
            if let Some(position) =
                self.latest_position(stream, target, &mut index_cache, true, &mut 0)?
            {
                candidates
                    .entry(position.chunk)
                    .or_default()
                    .push((stream, position.record_offset));
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
                    .map_err(McapOpenError::MessageIndexRequired)?,
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
        let recording = self.range;
        self.indexed_range(
            streams,
            crate::DataWindowTimeRange::new(recording.start, recording.end_exclusive)
                .expect("catalog recording range is ordered"),
        )
    }

    fn latest_position(
        &self,
        stream: StreamId,
        target: u64,
        index_cache: &mut HashMap<usize, ChunkMessageIndexes>,
        retain_restore_indexes: bool,
        index_reads: &mut u64,
    ) -> Result<Option<IndexedMessagePosition>, McapOpenError> {
        self.indexed_summary(&[stream])?;
        let count = self
            .catalog
            .by_id(stream)
            .and_then(|s| s.timing.message_count);
        let Some(sparse) = self.stream_chunks.get(&stream) else {
            return if count == Some(0) {
                Ok(None)
            } else {
                Err(McapOpenError::MessageIndexRequired(
                    crate::IndexedPlanError { stream },
                ))
            };
        };
        let target_time =
            ArrivalTime(i64::try_from(target).map_err(|_| McapOpenError::TimestampOverflow)?);
        let end = sparse
            .chunks
            .partition_point(|chunk| self.index_facts[*chunk].start <= target_time);
        let mut search = super::MessageIndexPredecessor::new(target);
        for cursor in (0..end).rev() {
            if !search.can_improve(
                sparse.prefix_max_end[cursor].0 as u64,
                sparse.prefix_max_chunk[cursor],
            ) {
                break;
            }
            let chunk = sparse.chunks[cursor];
            if !index_cache.contains_key(&chunk) {
                if !retain_restore_indexes {
                    index_cache.clear();
                }
                let parsed = self.read_chunk_message_indexes(chunk, || false)?;
                index_cache.insert(chunk, parsed);
                *index_reads = index_reads.saturating_add(1);
            }
            let channel = u16::try_from(stream.0).map_err(|_| mcap::McapError::BadIndex)?;
            if let Some(entries) = index_cache[&chunk].channels.get(&channel) {
                search.consider_sorted(chunk, entries);
            }
        }
        Ok(search.result().map(|(_, position)| position))
    }

    /// Physical Message Index span access. No traversal, cache or query-budget policy.
    pub(crate) fn read_chunk_message_indexes(
        &self,
        chunk_index: usize,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<ChunkMessageIndexes, McapOpenError> {
        if cancelled() {
            return Err(McapOpenError::QueryCancelled);
        }
        let chunk = self
            .summary
            .chunk_indexes
            .get(chunk_index)
            .ok_or(mcap::McapError::BadIndex)?;
        let mut indexes = HashMap::new();
        let Some(start) = chunk.message_index_offsets.values().copied().min() else {
            return Err(mcap::McapError::BadIndex.into());
        };
        let length =
            usize::try_from(chunk.message_index_length).map_err(|_| McapOpenError::ReadOverflow)?;
        // Message Index records are contiguous after their chunk. Read that bounded span once.
        #[cfg(test)]
        self.index_metrics
            .reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let bytes = self.backing.read(start, length)?;
        for (channel, offset) in &chunk.message_index_offsets {
            if cancelled() {
                return Err(McapOpenError::QueryCancelled);
            }
            let relative = usize::try_from(
                offset
                    .checked_sub(start)
                    .ok_or(McapOpenError::ReadOverflow)?,
            )
            .map_err(|_| McapOpenError::ReadOverflow)?;
            let header_end = relative.checked_add(9).ok_or(McapOpenError::ReadOverflow)?;
            let head = bytes
                .get(relative..header_end)
                .ok_or(mcap::McapError::BadIndex)?;
            if head[0] != op::MESSAGE_INDEX {
                return Err(mcap::McapError::BadIndex.into());
            }
            let length = usize::try_from(u64::from_le_bytes(head[1..9].try_into().unwrap()))
                .map_err(|_| McapOpenError::ReadOverflow)?;
            let body_end = header_end
                .checked_add(length)
                .ok_or(McapOpenError::ReadOverflow)?;
            let body = bytes
                .get(header_end..body_end)
                .ok_or(mcap::McapError::BadIndex)?;
            let mcap::records::Record::MessageIndex(index) =
                mcap::parse_record(op::MESSAGE_INDEX, body)?
            else {
                return Err(mcap::McapError::BadIndex.into());
            };
            if index.channel_id != *channel {
                return Err(mcap::McapError::BadIndex.into());
            }
            let mut records = index.records;
            records.sort_by_key(|entry| (entry.log_time, entry.offset));
            indexes.insert(*channel, records);
        }
        if cancelled() {
            return Err(McapOpenError::QueryCancelled);
        }
        Ok(ChunkMessageIndexes {
            #[cfg(test)]
            _resident: ResidentIndexSpan::new(self.index_metrics.clone(), &indexes),
            channels: indexes,
        })
    }

    pub(crate) fn validate_unchanged(&self) -> Result<(), McapOpenError> {
        self.backing.validate()
    }

    /// Borrowed index topology, ordered by (chunk start time, chunk ID).
    pub(crate) fn stream_chunk_timeline(
        &self,
        stream: StreamId,
    ) -> Option<StreamChunkTimeline<'_>> {
        self.stream_chunks
            .get(&stream)
            .map(|timeline| StreamChunkTimeline {
                chunks: &timeline.chunks,
                prefix_max_end: &timeline.prefix_max_end,
            })
    }

    #[cfg(test)]
    pub(crate) fn index_read_metrics(&self) -> &IndexReadMetrics {
        &self.index_metrics
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
                publish_time_ns: message.message.publish_time_ns,
                sequence: message.message.sequence,
                arrival: message.message.arrival_time,
                stream_id,
                payload: message.message.payload,
            });
        }
        cache.sort_by_key(|message| (message.arrival, message.stream_id.0));
        Ok(cache)
    }

    fn parse_chunk(&self, index: usize) -> Result<Vec<ParsedChunkMessage>, McapOpenError> {
        self.parse_chunk_cancellable(index, &mut || false)
    }

    fn parse_chunk_cancellable(
        &self,
        index: usize,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<Vec<ParsedChunkMessage>, McapOpenError> {
        if cancelled() {
            return Err(McapOpenError::QueryCancelled);
        }
        let chunk = self
            .summary
            .chunk_indexes
            .get(index)
            .ok_or(mcap::McapError::BadIndex)?;
        let uncompressed_crc = self.chunk_uncompressed_crc(chunk)?;
        let compressed = self.backing.read(
            chunk.compressed_data_offset()?,
            usize::try_from(chunk.compressed_size).map_err(|_| McapOpenError::ReadOverflow)?,
        )?;
        let expected_size = usize::try_from(chunk.uncompressed_size)
            .map_err(|_| McapOpenError::InvalidChunk("uncompressed size is too large".into()))?;
        if expected_size > super::backing::MAX_SOURCE_RECORD_BYTES {
            return Err(McapOpenError::ReadLimit {
                requested: expected_size,
                maximum: super::backing::MAX_SOURCE_RECORD_BYTES,
            });
        }
        let backing = super::decode_mcap_chunk(&chunk.compression, compressed, expected_size)?;
        if cancelled() {
            return Err(McapOpenError::QueryCancelled);
        }
        if uncompressed_crc != 0 && crc32fast::hash(&backing) != uncompressed_crc {
            return Err(McapOpenError::InvalidChunk(
                "uncompressed CRC does not match Chunk header".into(),
            ));
        }
        parse_chunk_backing(backing, cancelled)
    }

    fn chunk_uncompressed_crc(
        &self,
        chunk: &mcap::records::ChunkIndex,
    ) -> Result<u32, McapOpenError> {
        let length = 49usize
            .checked_add(chunk.compression.len())
            .ok_or(McapOpenError::ReadOverflow)?;
        let bytes = self.backing.read(chunk.chunk_start_offset, length)?;
        let u64_at = |offset| {
            u64::from_le_bytes(
                bytes[offset..offset + 8]
                    .try_into()
                    .expect("bounded header"),
            )
        };
        if bytes[0] != op::CHUNK
            || u64_at(1).checked_add(9) != Some(chunk.chunk_length)
            || u64_at(9) != chunk.message_start_time
            || u64_at(17) != chunk.message_end_time
            || u64_at(25) != chunk.uncompressed_size
            || u32::from_le_bytes(bytes[37..41].try_into().unwrap()) as usize
                != chunk.compression.len()
            || &bytes[41..41 + chunk.compression.len()] != chunk.compression.as_bytes()
            || u64_at(41 + chunk.compression.len()) != chunk.compressed_size
        {
            return Err(mcap::McapError::BadIndex.into());
        }
        Ok(u32::from_le_bytes(bytes[33..37].try_into().unwrap()))
    }
}

#[derive(Default)]
struct StreamChunks {
    chunks: Vec<usize>,
    prefix_max_end: Vec<ArrivalTime>,
    prefix_max_chunk: Vec<usize>,
}

/// Source-local exact record identity, for query view reuse. Not a persistent/global message ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndexedMessagePosition {
    pub chunk: usize,
    pub record_offset: u64,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LatestQueryDiagnostics {
    pub index_reads: u64,
    pub chunks_read: u64,
}
pub(crate) struct StreamChunkTimeline<'a> {
    pub(crate) chunks: &'a [usize],
    pub(crate) prefix_max_end: &'a [ArrivalTime],
}

pub(crate) struct ChunkMessageIndexes {
    pub(crate) channels: HashMap<u16, Vec<mcap::records::MessageIndexEntry>>,
    #[cfg(test)]
    _resident: ResidentIndexSpan,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct IndexReadMetrics {
    pub(crate) reads: std::sync::atomic::AtomicUsize,
    pub(crate) resident_spans: std::sync::atomic::AtomicUsize,
    pub(crate) peak_resident_spans: std::sync::atomic::AtomicUsize,
    pub(crate) resident_entry_bytes: std::sync::atomic::AtomicUsize,
    pub(crate) peak_resident_entry_bytes: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
struct ResidentIndexSpan {
    metrics: std::sync::Arc<IndexReadMetrics>,
    entry_bytes: usize,
}
#[cfg(test)]
impl ResidentIndexSpan {
    fn new(
        metrics: std::sync::Arc<IndexReadMetrics>,
        channels: &HashMap<u16, Vec<mcap::records::MessageIndexEntry>>,
    ) -> Self {
        use std::sync::atomic::Ordering::Relaxed;
        let entry_bytes = channels
            .values()
            .map(|entries| {
                entries.capacity() * std::mem::size_of::<mcap::records::MessageIndexEntry>()
            })
            .sum();
        let spans = metrics.resident_spans.fetch_add(1, Relaxed) + 1;
        metrics.peak_resident_spans.fetch_max(spans, Relaxed);
        let bytes = metrics.resident_entry_bytes.fetch_add(entry_bytes, Relaxed) + entry_bytes;
        metrics.peak_resident_entry_bytes.fetch_max(bytes, Relaxed);
        Self {
            metrics,
            entry_bytes,
        }
    }
}
#[cfg(test)]
impl Drop for ResidentIndexSpan {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        self.metrics.resident_spans.fetch_sub(1, Relaxed);
        self.metrics
            .resident_entry_bytes
            .fetch_sub(self.entry_bytes, Relaxed);
    }
}

/// Concrete cursor query accelerator: at most one Message Index span and one serialized chunk.
/// Decoded Inspector view models are owned by the caller, never by this physical cache.
pub struct LatestMessageReader {
    source: McapSource,
    indexes: HashMap<usize, ChunkMessageIndexes>,
    chunk: Option<(usize, Vec<ParsedChunkMessage>)>,
    diagnostics: LatestQueryDiagnostics,
}
impl LatestMessageReader {
    pub fn new(source: McapSource) -> Self {
        Self {
            source,
            indexes: HashMap::new(),
            chunk: None,
            diagnostics: LatestQueryDiagnostics::default(),
        }
    }
    pub fn catalog(&self) -> &SourceCatalog {
        self.source.catalog()
    }
    pub fn diagnostics(&self) -> LatestQueryDiagnostics {
        self.diagnostics
    }
    pub fn read_at(
        &mut self,
        stream: StreamId,
        target: ArrivalTime,
    ) -> Result<Option<(IndexedMessagePosition, RawMessage)>, McapOpenError> {
        self.read_at_cancellable(stream, target, || false)
    }
    pub fn read_at_cancellable(
        &mut self,
        stream: StreamId,
        target: ArrivalTime,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Option<(IndexedMessagePosition, RawMessage)>, McapOpenError> {
        if cancelled() {
            return Err(McapOpenError::QueryCancelled);
        }
        self.source.backing.validate()?;
        if target < self.source.range.start {
            return Ok(None);
        }
        let target = u64::try_from(target.0).map_err(|_| McapOpenError::TimestampOverflow)?;
        let Some(position) = self.source.latest_position(
            stream,
            target,
            &mut self.indexes,
            false,
            &mut self.diagnostics.index_reads,
        )?
        else {
            return Ok(None);
        };
        if self
            .chunk
            .as_ref()
            .is_none_or(|(chunk, _)| *chunk != position.chunk)
        {
            self.chunk = None;
            let parsed = self
                .source
                .parse_chunk_cancellable(position.chunk, &mut cancelled)?;
            self.chunk = Some((position.chunk, parsed));
            self.diagnostics.chunks_read = self.diagnostics.chunks_read.saturating_add(1);
        }
        let messages = &self.chunk.as_ref().expect("loaded chunk").1;
        let index = messages.partition_point(|m| m.offset < position.record_offset);
        let message = messages
            .get(index)
            .filter(|m| {
                m.offset == position.record_offset
                    && m.message.stream_id == stream
                    && m.message.arrival_time.0 as u64 <= target
            })
            .ok_or(mcap::McapError::BadIndex)?;
        Ok(Some((position, message.message.clone())))
    }
}

struct ParsedChunkMessage {
    offset: u64,
    message: RawMessage,
}

fn parse_chunk_backing(
    backing: Bytes,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<Vec<ParsedChunkMessage>, McapOpenError> {
    let mut messages = Vec::new();
    super::visit_mcap_chunk_records(&backing, |offset, message| {
        if cancelled() {
            return Err(McapOpenError::QueryCancelled);
        }
        if let Some(message) = message {
            messages.push(ParsedChunkMessage { offset, message });
        }
        Ok(())
    })?;
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
        let source =
            McapSource::copy_from_slice((indexed_fixture(1_000_000, true)).as_ref()).unwrap();
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
        let messages = source
            .read_through(source.time_range().end_inclusive())
            .unwrap();

        assert_eq!(messages.len(), 3);
        for message in &messages {
            let start = message.payload.as_ptr() as usize;
            let end = start + message.payload.len();
            assert!(source_start <= start && end <= source_end);
        }
    }

    #[test]
    fn native_zstd_messages_share_one_decompressed_chunk_backing() {
        let mut source =
            McapSource::from_bytes(Bytes::from(indexed_fixture(1_000_000, true))).unwrap();
        let streams = ["/a", "/b"].map(|topic| source.catalog().by_topic(topic).unwrap().id);
        source.select_streams(streams).unwrap();
        let messages = source
            .read_through(source.time_range().end_inclusive())
            .unwrap();
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
        let mut source =
            McapSource::from_bytes(Bytes::from(payload_fixture(Some(Compression::Lz4)))).unwrap();
        let camera = source.catalog().by_topic("/camera").unwrap().id;
        source.select_streams([camera]).unwrap();
        let messages = source
            .read_through(source.time_range().end_inclusive())
            .unwrap();
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
        let source = McapSource::copy_from_slice((bytes).as_ref()).unwrap();
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
        let source = McapSource::copy_from_slice((sparse_restore_fixture()).as_ref()).unwrap();
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
        let mut source =
            McapSource::copy_from_slice((indexed_fixture(128, false)).as_ref()).unwrap();
        let stream = source.catalog().by_topic("/a").unwrap().id;
        assert!(matches!(
            source.select_streams([stream]),
            Err(McapOpenError::MessageIndexRequired(found)) if found.stream == stream
        ));
        assert!(matches!(
            source.latest_before(&[stream], ArrivalTime(1_000_000_000)),
            Err(McapOpenError::MessageIndexRequired(found)) if found.stream == stream
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
            McapSource::copy_from_slice((bytes.into_inner()).as_ref()),
            Err(McapOpenError::ChunkIndexRequired)
        ));
    }
}
