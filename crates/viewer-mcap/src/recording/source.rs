//! Native indexed MCAP source: metadata and forward traversal.
mod index;
mod latest;
mod range;
#[cfg(test)]
use index::IndexReadMetrics;
pub use index::IndexedMessagePosition;
use index::{LatestIndexRead, StreamChunks};
pub use latest::{LatestMessageReader, LatestQueryDiagnostics};
pub use range::{QueryLimits, RangeQuery, RangeQueryError, RangeQueryResult};

use crate::{IndexedChunkFact, McapSourceError, ensure_indexed, history_candidate_chunks};

use bytes::Bytes;
use mcap::{Summary, records::op};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use viewer_core::{
    ArrivalTime, RawMessage, RecordingTimeRange, SourceCapabilities, SourceCatalog,
    StreamDescriptor, StreamId, StreamTimingSummary,
};

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
    forward: McapSourcePosition,
    all_chunks: std::sync::Arc<[usize]>,
    all_prefix_max_end: std::sync::Arc<[ArrivalTime]>,
    forward_chunks: std::sync::Arc<[usize]>,
    forward_prefix_max_end: std::sync::Arc<[ArrivalTime]>,
    selected_streams: Option<HashSet<StreamId>>,
    index_facts: std::sync::Arc<[IndexedChunkFact]>,
    stream_chunks: std::sync::Arc<HashMap<StreamId, StreamChunks>>,
    read_metrics: std::sync::Arc<ChunkReadMetrics>,
    #[cfg(test)]
    index_metrics: std::sync::Arc<IndexReadMetrics>,
}

#[derive(Default)]
struct ChunkReadMetrics {
    chunks: std::sync::atomic::AtomicU64,
    expanded_bytes: std::sync::atomic::AtomicU64,
    decompressed_bytes: std::sync::atomic::AtomicU64,
}

/// Cumulative work shared by file/source forks. File bytes count successful positional
/// reads, including metadata and rereads, not physical disk traffic or OS cache misses.
/// Chunk counters count successful expansion (including uncompressed chunks).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SourceReadDiagnostics {
    pub file_read_calls: u64,
    pub file_read_bytes: u64,
    pub chunks_expanded: u64,
    pub expanded_bytes: u64,
    pub decompressed_bytes: u64,
    /// Sum of active overlapping sequential chunk backings, excluding feature/output memory.
    pub sequential_cache_backing_bytes: u64,
}

#[derive(Clone, Default)]
pub(crate) struct McapSourcePosition {
    next: usize,
    active: Vec<ActiveChunk>,
    lower: Option<ArrivalTime>,
}
#[derive(Clone)]
struct ActiveChunk {
    index: usize,
    messages: std::sync::Arc<[CachedMessage]>,
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
    pub fn copy_from_slice(backing: &[u8]) -> Result<Self, McapSourceError> {
        Self::from_bytes(Bytes::copy_from_slice(backing))
    }
    pub fn from_bytes(backing: Bytes) -> Result<Self, McapSourceError> {
        Self::from_backing(super::backing::Backing::Bytes(backing))
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_file(path: &std::path::Path) -> Result<Self, McapSourceError> {
        if path.is_dir() {
            return Self::open_files(&super::segments::directory_paths(path)?);
        }
        Self::from_backing(super::backing::Backing::File(std::sync::Arc::new(
            super::backing::FileRanges::open(path)?,
        )))
    }
    /// Open indexed parts as one log-time ordered recording. Payloads remain in their files.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_files(paths: &[std::path::PathBuf]) -> Result<Self, McapSourceError> {
        if paths.len() == 1 {
            return Self::from_backing(super::backing::Backing::File(std::sync::Arc::new(
                super::backing::FileRanges::open(&paths[0])?,
            )));
        }
        let parts = super::segments::SegmentedBacking::open(paths)?;
        Self::from_backing(super::backing::Backing::Segments(std::sync::Arc::new(
            parts,
        )))
    }
    fn from_backing(backing: super::backing::Backing) -> Result<Self, McapSourceError> {
        let summary = backing.summary()?;
        if summary.chunk_indexes.is_empty() {
            return Err(McapSourceError::ChunkIndexRequired);
        }
        let schema_definitions = summary
            .schemas
            .iter()
            .map(|(id, schema)| (*id, crate::schema_definition_from_mcap(schema)))
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
            .ok_or(McapSourceError::TimestampOverflow)?;
        let range = RecordingTimeRange::new(range.0, ArrivalTime(end_exclusive))
            .ok_or(McapSourceError::TimestampOverflow)?;
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
        let mut all_chunks = (0..summary.chunk_indexes.len()).collect::<Vec<_>>();
        all_chunks.sort_by_key(|&i| (summary.chunk_indexes[i].message_start_time, i));
        let all_prefix_max_end: std::sync::Arc<[ArrivalTime]> =
            forward_prefix_max_end(&summary, &all_chunks).into();
        let all_chunks: std::sync::Arc<[usize]> = all_chunks.into();
        Ok(Self {
            forward_chunks: all_chunks.clone(),
            forward_prefix_max_end: all_prefix_max_end.clone(),
            all_chunks,
            all_prefix_max_end,
            forward: Default::default(),
            backing,
            summary,
            catalog: std::sync::Arc::new(catalog),
            range,

            selected_streams: None,
            index_facts: std::sync::Arc::from(index_facts),
            stream_chunks: std::sync::Arc::new(stream_chunks),
            read_metrics: Default::default(),
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
            forward: Default::default(),
            all_chunks: self.all_chunks.clone(),
            forward_chunks: self.all_chunks.clone(),
            all_prefix_max_end: self.all_prefix_max_end.clone(),
            forward_prefix_max_end: self.all_prefix_max_end.clone(),
            selected_streams: None,
            index_facts: self.index_facts.clone(),
            stream_chunks: self.stream_chunks.clone(),
            read_metrics: self.read_metrics.clone(),
            #[cfg(test)]
            index_metrics: self.index_metrics.clone(),
        }
    }
    pub fn summary(&self) -> &Summary {
        &self.summary
    }
    pub fn summary_identity(&self) -> Result<crate::McapSummaryIdentity, McapSourceError> {
        let stats = self
            .summary
            .stats
            .as_ref()
            .ok_or(McapSourceError::SummaryRequired)?;
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

    pub fn read_diagnostics(&self) -> SourceReadDiagnostics {
        use std::sync::atomic::Ordering::Relaxed;
        let (file_read_calls, file_read_bytes) = self.backing.file_read_totals();
        SourceReadDiagnostics {
            file_read_calls,
            file_read_bytes,
            chunks_expanded: self.read_metrics.chunks.load(Relaxed),
            expanded_bytes: self.read_metrics.expanded_bytes.load(Relaxed),
            decompressed_bytes: self.read_metrics.decompressed_bytes.load(Relaxed),
            sequential_cache_backing_bytes: self
                .forward
                .active
                .iter()
                .map(|c| self.summary.chunk_indexes[c.index].uncompressed_size)
                .sum(),
        }
    }
    pub fn time_range(&self) -> RecordingTimeRange {
        self.range
    }

    pub fn select_streams(
        &mut self,
        streams: impl IntoIterator<Item = StreamId>,
    ) -> Result<(), McapSourceError> {
        let selected = streams.into_iter().collect::<HashSet<_>>();
        for stream in &selected {
            let descriptor = self
                .catalog
                .by_id(*stream)
                .ok_or(mcap::McapError::BadIndex)?;
            ensure_indexed(&self.index_facts, *stream, descriptor.timing.message_count)
                .map_err(McapSourceError::MessageIndexRequired)?;
        }
        self.forward_chunks = self
            .all_chunks
            .iter()
            .copied()
            .filter(|&i| {
                self.index_facts[i]
                    .indexed_streams
                    .iter()
                    .any(|s| selected.contains(s))
            })
            .collect::<Vec<_>>()
            .into();
        self.forward_prefix_max_end =
            forward_prefix_max_end(&self.summary, &self.forward_chunks).into();
        self.forward = Default::default();
        self.selected_streams = Some(selected);
        Ok(())
    }

    pub fn seek(&mut self, cursor: ArrivalTime) -> Result<(), McapSourceError> {
        let position = self.prepare_seek(cursor)?;
        self.commit_seek(position);
        Ok(())
    }

    pub(crate) fn prepare_seek(
        &self,
        cursor: ArrivalTime,
    ) -> Result<McapSourcePosition, McapSourceError> {
        self.backing.validate()?;
        let mut state = McapSourcePosition {
            lower: Some(cursor),
            next: self
                .forward_prefix_max_end
                .partition_point(|end| *end < cursor),
            ..Default::default()
        };
        // End times need not be monotonic. Stage every selected chunk spanning the target.
        while let Some(&index) = self.forward_chunks.get(state.next) {
            if self.index_facts[index].start > cursor {
                break;
            }
            state.next += 1;
            if self.index_facts[index].end_inclusive >= cursor {
                self.stage_forward_chunk(&mut state, index)?;
            }
        }
        self.backing.validate()?;
        Ok(state)
    }

    pub(crate) fn commit_seek(&mut self, position: McapSourcePosition) {
        self.forward = position;
    }

    fn stage_forward_chunk(
        &self,
        state: &mut McapSourcePosition,
        index: usize,
    ) -> Result<(), McapSourceError> {
        let size = self.summary.chunk_indexes[index].uncompressed_size;
        let retained = state
            .active
            .iter()
            .map(|c| self.summary.chunk_indexes[c.index].uncompressed_size)
            .sum::<u64>();
        if size.saturating_add(retained) > super::backing::MAX_SOURCE_RECORD_BYTES as u64 {
            return Err(McapSourceError::QueryLimit {
                resource: "forward overlapping chunk backing bytes",
                maximum: super::backing::MAX_SOURCE_RECORD_BYTES,
            });
        }
        let messages = self.read_chunk(index)?;
        let position = state
            .lower
            .map_or(0, |lower| messages.partition_point(|m| m.arrival < lower));
        if position < messages.len() {
            state.active.push(ActiveChunk {
                index,
                messages: messages.into(),
                position,
            });
        }
        Ok(())
    }

    fn next_forward(
        &self,
        state: &mut McapSourcePosition,
        cursor: ArrivalTime,
    ) -> Result<Option<RawMessage>, McapSourceError> {
        loop {
            let head = state.active.iter().enumerate().min_by_key(|(_, c)| {
                let m = &c.messages[c.position];
                (m.arrival, m.stream_id.0, c.index, c.position)
            });
            let horizon = head.map_or(cursor, |(_, c)| c.messages[c.position].arrival.min(cursor));
            if let Some(&index) = self.forward_chunks.get(state.next)
                && self.index_facts[index].start <= horizon
            {
                state.next += 1;
                self.stage_forward_chunk(state, index)?;
                continue;
            }
            let Some((slot, c)) = head else {
                return Ok(None);
            };
            let m = &c.messages[c.position];
            if m.arrival > cursor {
                return Ok(None);
            }
            let message = RawMessage {
                stream_id: m.stream_id,
                arrival_time: m.arrival,
                publish_time_ns: m.publish_time_ns,
                sequence: m.sequence,
                payload: m.payload.clone(),
            };
            let c = &mut state.active[slot];
            c.position += 1;
            if c.position == c.messages.len() {
                state.active.remove(slot);
            }
            return Ok(Some(message));
        }
    }

    /// Inclusive, chronological delivery; equal times use stream then physical record order.
    /// All fallible work is staged before committing traversal. Overlapping chunk backings are
    /// bounded to 256 MiB; consumed chunks are released instead of accumulating a history.
    pub fn read_through(
        &mut self,
        cursor: ArrivalTime,
    ) -> Result<Vec<RawMessage>, McapSourceError> {
        let mut state = self.forward.clone();
        let mut messages = Vec::new();
        while let Some(message) = self.next_forward(&mut state, cursor)? {
            messages.push(message);
        }
        self.forward = state;
        Ok(messages)
    }

    /// Concrete streaming I/O for numeric reduction, independent of playback selection/cursor.
    pub(crate) fn visit_stream_range(
        &self,
        stream: StreamId,
        range: viewer_core::DataWindowTimeRange,
        mut cancelled: impl FnMut() -> bool,
        mut visit: impl FnMut(&RawMessage),
    ) -> Result<usize, McapSourceError> {
        let descriptor = self
            .catalog
            .by_id(stream)
            .ok_or(mcap::McapError::BadIndex)?;
        ensure_indexed(&self.index_facts, stream, descriptor.timing.message_count)
            .map_err(McapSourceError::MessageIndexRequired)?;
        let mut chunks_read = 0;
        // Inspect index facts before touching compressed payloads. No unrelated chunk scan.
        for (index, fact) in self.index_facts.iter().enumerate() {
            if cancelled() {
                return Err(McapSourceError::QueryCancelled);
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
                    return Err(McapSourceError::QueryCancelled);
                }
                visit(&message);
            }
        }
        Ok(chunks_read)
    }

    /// Finds one exact predecessor per stream using MCAP Message Index records.
    ///
    /// Candidate entries are selected without decompressing chunks. Candidates are then grouped
    /// by chunk so a compressed chunk is streamed at most once for the whole multi-stream lookup.
    pub fn latest_before(
        &self,
        streams: &[StreamId],
        target: ArrivalTime,
    ) -> Result<IndexedMessages, McapSourceError> {
        self.latest_before_with_limits(streams, target, crate::LatestQueryLimits::default())
    }

    pub fn latest_before_with_limits(
        &self,
        streams: &[StreamId],
        target: ArrivalTime,
        limits: crate::LatestQueryLimits,
    ) -> Result<IndexedMessages, McapSourceError> {
        let mut budget = crate::LatestQueryBudget::new(limits);
        budget.streams(streams.len())?;
        self.backing.validate()?;
        self.indexed_summary(streams)?;
        let target = u64::try_from(target.0).map_err(|_| McapSourceError::TimestampOverflow)?;
        let mut index_cache = HashMap::new();
        let mut candidates = BTreeMap::<usize, Vec<(StreamId, u64)>>::new();
        let mut reads = 0;
        let mut cancelled = || false;
        for stream in streams.iter().copied() {
            if let Some(position) = self.latest_position(
                stream,
                target,
                &mut LatestIndexRead {
                    cache: &mut index_cache,
                    reads: &mut reads,
                    budget: &mut budget,
                    cancelled: &mut cancelled,
                },
            )? {
                candidates
                    .entry(position.chunk)
                    .or_default()
                    .push((stream, position.record_offset));
            }
        }

        let mut selected = HashMap::<StreamId, RawMessage>::new();
        for (chunk_index, wanted) in &candidates {
            let chunk = &self.summary.chunk_indexes[*chunk_index];
            budget.chunk(chunk.compressed_size, chunk.uncompressed_size)?;
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
        self.backing.validate()?;
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
        range: viewer_core::DataWindowTimeRange,
    ) -> Result<IndexedMessages, McapSourceError> {
        self.indexed_summary(streams)?;
        let mut chunks = HashSet::<usize>::new();
        for stream in streams.iter().copied() {
            let count = self
                .catalog
                .by_id(stream)
                .and_then(|value| value.timing.message_count);
            chunks.extend(
                history_candidate_chunks(&self.index_facts, stream, count, range)
                    .map_err(McapSourceError::MessageIndexRequired)?,
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
    pub fn indexed_streams(
        &self,
        streams: &[StreamId],
    ) -> Result<IndexedMessages, McapSourceError> {
        let recording = self.range;
        self.indexed_range(
            streams,
            viewer_core::DataWindowTimeRange::new(recording.start, recording.end_exclusive)
                .expect("catalog recording range is ordered"),
        )
    }

    pub(crate) fn validate_unchanged(&self) -> Result<(), McapSourceError> {
        self.backing.validate()
    }

    fn indexed_summary(&self, streams: &[StreamId]) -> Result<&Summary, McapSourceError> {
        for stream in streams {
            if self.catalog.by_id(*stream).is_none() {
                return Err(McapSourceError::Mcap(mcap::McapError::BadIndex));
            }
        }
        Ok(&self.summary)
    }

    fn read_chunk(&self, index: usize) -> Result<Vec<CachedMessage>, McapSourceError> {
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

    fn parse_chunk(&self, index: usize) -> Result<Vec<ParsedChunkMessage>, McapSourceError> {
        self.parse_chunk_cancellable(index, &mut || false)
    }

    fn parse_chunk_cancellable(
        &self,
        index: usize,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<Vec<ParsedChunkMessage>, McapSourceError> {
        if cancelled() {
            return Err(McapSourceError::QueryCancelled);
        }
        let chunk = self
            .summary
            .chunk_indexes
            .get(index)
            .ok_or(mcap::McapError::BadIndex)?;
        let uncompressed_crc = self.chunk_uncompressed_crc(chunk)?;
        let compressed = self.backing.read(
            chunk.compressed_data_offset()?,
            usize::try_from(chunk.compressed_size).map_err(|_| McapSourceError::ReadOverflow)?,
        )?;
        let expected_size = usize::try_from(chunk.uncompressed_size)
            .map_err(|_| McapSourceError::InvalidChunk("uncompressed size is too large".into()))?;
        if expected_size > super::backing::MAX_SOURCE_RECORD_BYTES {
            return Err(McapSourceError::ReadLimit {
                requested: expected_size,
                maximum: super::backing::MAX_SOURCE_RECORD_BYTES,
            });
        }
        let backing = super::decode_mcap_chunk(&chunk.compression, compressed, expected_size)?;
        use std::sync::atomic::Ordering::Relaxed;
        self.read_metrics.chunks.fetch_add(1, Relaxed);
        self.read_metrics
            .expanded_bytes
            .fetch_add(backing.len() as u64, Relaxed);
        if !chunk.compression.is_empty() {
            self.read_metrics
                .decompressed_bytes
                .fetch_add(backing.len() as u64, Relaxed);
        }
        if cancelled() {
            return Err(McapSourceError::QueryCancelled);
        }
        if uncompressed_crc != 0 && crc32fast::hash(&backing) != uncompressed_crc {
            return Err(McapSourceError::InvalidChunk(
                "uncompressed CRC does not match Chunk header".into(),
            ));
        }
        let mut messages = parse_chunk_backing(backing, cancelled)?;
        for parsed in &mut messages {
            let local =
                u16::try_from(parsed.message.stream_id.0).map_err(|_| mcap::McapError::BadIndex)?;
            parsed.message.stream_id = StreamId(u32::from(self.backing.channel_id(index, local)?));
        }
        Ok(messages)
    }

    fn chunk_uncompressed_crc(
        &self,
        chunk: &mcap::records::ChunkIndex,
    ) -> Result<u32, McapSourceError> {
        let length = 49usize
            .checked_add(chunk.compression.len())
            .ok_or(McapSourceError::ReadOverflow)?;
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

struct ParsedChunkMessage {
    offset: u64,
    message: RawMessage,
}

fn parse_chunk_backing(
    backing: Bytes,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<Vec<ParsedChunkMessage>, McapSourceError> {
    let mut messages = Vec::new();
    super::visit_mcap_chunk_records(&backing, |offset, message| {
        if cancelled() {
            return Err(McapSourceError::QueryCancelled);
        }
        if let Some(message) = message {
            messages.push(ParsedChunkMessage { offset, message });
        }
        Ok(())
    })?;
    Ok(messages)
}

fn forward_prefix_max_end(summary: &Summary, chunks: &[usize]) -> Vec<ArrivalTime> {
    let mut max = ArrivalTime(i64::MIN);
    chunks
        .iter()
        .map(|&i| {
            max = max.max(to_arrival_lossy(summary.chunk_indexes[i].message_end_time));
            max
        })
        .collect()
}

fn to_arrival(value: u64) -> Result<ArrivalTime, McapSourceError> {
    i64::try_from(value)
        .map(ArrivalTime)
        .map_err(|_| McapSourceError::TimestampOverflow)
}
fn to_arrival_lossy(value: u64) -> ArrivalTime {
    ArrivalTime(i64::try_from(value).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcap::{Compression, WriteOptions, Writer, records::MessageHeader};
    use std::{collections::BTreeMap, io::Cursor};

    #[test]
    fn forward_overlap_budget_rejects_before_reading_and_preserves_position() {
        let mut source = no_predecessor_fixture(2);
        // Model a fully occupied active backing without allocating 256 MiB in a unit test.
        std::sync::Arc::make_mut(&mut source.summary).chunk_indexes[0].uncompressed_size =
            super::super::backing::MAX_SOURCE_RECORD_BYTES as u64;
        source.forward.next = 1;
        source.forward.active.push(ActiveChunk {
            index: 0,
            messages: vec![CachedMessage {
                publish_time_ns: None,
                sequence: None,
                arrival: ArrivalTime(200),
                stream_id: StreamId(1),
                payload: Bytes::from_static(&[1]),
            }]
            .into(),
            position: 0,
        });
        assert!(matches!(
            source.read_through(ArrivalTime(200)),
            Err(McapSourceError::QueryLimit {
                resource: "forward overlapping chunk backing bytes",
                ..
            })
        ));
        assert_eq!(source.forward.next, 1);
        assert_eq!(source.forward.active[0].position, 0);
        assert_eq!(source.read_diagnostics().chunks_expanded, 0);
    }

    #[test]
    fn read_diagnostics_count_reexpansion_and_share_work_across_forks() {
        let source = no_predecessor_fixture(2);
        assert_eq!(source.read_diagnostics().chunks_expanded, 0);
        let streams = source
            .catalog()
            .streams
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>();
        let fork = source.fork_reader();
        let before = source.read_diagnostics();
        let result = fork.latest_before(&streams, ArrivalTime(101)).unwrap();
        let after = source.read_diagnostics();
        assert_eq!(
            after.chunks_expanded,
            result.diagnostics.chunks_streamed as u64
        );
        assert!(after.expanded_bytes > before.expanded_bytes);
        assert_eq!(after.decompressed_bytes, 0);
        assert_eq!(after.file_read_bytes, 0); // Owned bytes are not filesystem reads.
        source.latest_before(&streams, ArrivalTime(101)).unwrap();
        assert_eq!(
            source.read_diagnostics().expanded_bytes,
            2 * after.expanded_bytes
        );
    }

    fn no_predecessor_fixture(chunks: u64) -> McapSource {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = Writer::with_options(
                &mut output,
                WriteOptions::new().compression(None).chunk_size(None),
            )
            .unwrap();
            let schema = writer
                .add_schema("demo/msg/Sample", "ros2msg", b"uint8 value")
                .unwrap();
            let future = writer
                .add_channel(schema, "/future", "cdr", &BTreeMap::new())
                .unwrap();
            let past = writer
                .add_channel(schema, "/past", "cdr", &BTreeMap::new())
                .unwrap();
            for chunk in 0..chunks {
                for (channel, time) in [(future, 100 + chunk), (past, 0)] {
                    writer
                        .write_to_known_channel(
                            &MessageHeader {
                                channel_id: channel,
                                sequence: chunk as u32,
                                log_time: time,
                                publish_time: 0,
                            },
                            &[1],
                        )
                        .unwrap();
                }
                writer.flush().unwrap();
            }
            writer.finish().unwrap();
        }
        McapSource::from_bytes(output.into_inner().into()).unwrap()
    }

    #[test]
    fn cancellation_after_first_index_read_stops_even_without_a_predecessor() {
        use std::sync::atomic::Ordering::Relaxed;
        let source = no_predecessor_fixture(64);
        let stream = source.catalog().by_topic("/future").unwrap().id;
        let metrics = source.index_metrics.clone();
        let mut reader = LatestMessageReader::new(source);
        assert!(matches!(
            reader.read_at_cancellable(stream, ArrivalTime(0), || metrics.reads.load(Relaxed) > 0),
            Err(McapSourceError::QueryCancelled)
        ));
        assert_eq!(metrics.reads.load(Relaxed), 1);
        assert_eq!(metrics.resident_spans.load(Relaxed), 0);
    }

    #[test]
    fn generation_change_during_lookup_wakes_and_joins_after_one_read() {
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering::Relaxed},
            mpsc,
        };
        let source = no_predecessor_fixture(64);
        let stream = source.catalog().by_topic("/future").unwrap().id;
        let metrics = source.index_metrics.clone();
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = generation.clone();
        let (entered, wait_entered) = mpsc::channel();
        let (resume, wait_resume) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut reader = LatestMessageReader::new(source);
            let mut announced = false;
            let result = reader.read_at_cancellable(stream, ArrivalTime(0), || {
                if metrics.reads.load(Relaxed) > 0 && !announced {
                    announced = true;
                    entered.send(()).unwrap();
                    wait_resume.recv().unwrap();
                }
                worker_generation.load(Relaxed) != 0
            });
            (result, reader.diagnostics())
        });
        wait_entered
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        generation.store(1, Relaxed);
        resume.send(()).unwrap();
        let (result, metrics) = worker.join().unwrap();
        assert!(matches!(result, Err(McapSourceError::QueryCancelled)));
        assert_eq!(metrics.index_reads, 1);
    }

    #[test]
    fn latest_demand_reserves_reads_bytes_and_candidates_before_exceeding_them() {
        use std::sync::atomic::Ordering::Relaxed;
        for (limits, resource, expected_reads) in [
            (
                crate::LatestQueryLimits {
                    max_index_reads: 1,
                    ..Default::default()
                },
                "latest index reads",
                1,
            ),
            (
                crate::LatestQueryLimits {
                    max_index_bytes: 1,
                    ..Default::default()
                },
                "latest index bytes",
                0,
            ),
            (
                crate::LatestQueryLimits {
                    max_candidates: 1,
                    ..Default::default()
                },
                "latest chunk candidates",
                1,
            ),
        ] {
            let source = no_predecessor_fixture(3);
            let stream = source.catalog().by_topic("/future").unwrap().id;
            let metrics = source.index_metrics.clone();
            let mut reader = LatestMessageReader::new(source);
            let mut budget = crate::LatestQueryBudget::new(limits);
            assert!(
                matches!(reader.read_at_with_budget_cancellable(stream, ArrivalTime(0), &mut budget, || false), Err(McapSourceError::QueryLimit { resource: found, .. }) if found == resource)
            );
            assert_eq!(metrics.reads.load(Relaxed), expected_reads);
        }
    }

    #[test]
    fn parsed_restore_indexes_remain_one_span_as_recording_duration_grows() {
        use std::sync::atomic::Ordering::Relaxed;
        let mut peaks = Vec::new();
        for chunks in [3, 128] {
            let source = no_predecessor_fixture(chunks);
            let stream = source.catalog().by_topic("/future").unwrap().id;
            assert!(
                source
                    .latest_before(&[stream], ArrivalTime(0))
                    .unwrap()
                    .messages
                    .is_empty()
            );
            let metrics = source.index_read_metrics();
            assert_eq!(metrics.peak_resident_spans.load(Relaxed), 1);
            assert_eq!(metrics.resident_spans.load(Relaxed), 0);
            peaks.push(metrics.peak_resident_entry_bytes.load(Relaxed));
        }
        assert_eq!(peaks[0], peaks[1]);
        assert!(peaks[0] <= super::super::latest_limits::MAX_LATEST_INDEX_BYTES);
    }

    #[test]
    fn fresh_budgets_reuse_cached_indexes_and_cached_results_still_check_cancellation() {
        let source = no_predecessor_fixture(1);
        let stream = source.catalog().by_topic("/past").unwrap().id;
        let mut reader = LatestMessageReader::new(source);
        for _ in 0..16 {
            let mut budget = crate::LatestQueryBudget::new(crate::LatestQueryLimits {
                max_streams: 1,
                max_index_reads: 1,
                ..Default::default()
            });
            assert!(
                reader
                    .read_at_with_budget_cancellable(stream, ArrivalTime(0), &mut budget, || false)
                    .unwrap()
                    .is_some()
            );
        }
        assert_eq!(reader.diagnostics().index_reads, 1);
        let mut checks = 0;
        assert!(matches!(
            reader.read_at_cancellable(stream, ArrivalTime(0), || {
                checks += 1;
                checks > 2
            }),
            Err(McapSourceError::QueryCancelled)
        ));
        let mut budget = crate::LatestQueryBudget::new(crate::LatestQueryLimits {
            max_streams: 1,
            ..Default::default()
        });
        reader
            .read_at_with_budget_cancellable(stream, ArrivalTime(0), &mut budget, || false)
            .unwrap();
        assert!(matches!(
            reader.read_at_with_budget_cancellable(stream, ArrivalTime(0), &mut budget, || false),
            Err(McapSourceError::QueryLimit {
                resource: "latest streams",
                ..
            })
        ));
    }

    #[test]
    fn failed_index_parse_spends_the_attempt_budget_and_chunk_limits_fail_before_decode() {
        use std::sync::atomic::Ordering::Relaxed;
        let mut source = no_predecessor_fixture(1);
        let stream = source.catalog().by_topic("/past").unwrap().id;
        let summary = source.summary.clone();
        let index = *summary.chunk_indexes[0]
            .message_index_offsets
            .values()
            .min()
            .unwrap() as usize;
        let super::super::backing::Backing::Bytes(bytes) = &source.backing else {
            unreachable!()
        };
        let mut corrupt = bytes.to_vec();
        corrupt[index] = 0;
        source.backing = super::super::backing::Backing::Bytes(corrupt.into());
        let metrics = source.index_metrics.clone();
        let mut reader = LatestMessageReader::new(source);
        let mut budget = crate::LatestQueryBudget::new(crate::LatestQueryLimits {
            max_index_reads: 1,
            ..Default::default()
        });
        assert!(
            reader
                .read_at_with_budget_cancellable(stream, ArrivalTime(0), &mut budget, || false)
                .is_err()
        );
        assert!(matches!(
            reader.read_at_with_budget_cancellable(stream, ArrivalTime(0), &mut budget, || false),
            Err(McapSourceError::QueryLimit {
                resource: "latest index reads",
                ..
            })
        ));
        assert_eq!(metrics.reads.load(Relaxed), 1);
        let source = no_predecessor_fixture(1);
        let stream = source.catalog().by_topic("/past").unwrap().id;
        let mut reader = LatestMessageReader::new(source);
        let mut budget = crate::LatestQueryBudget::new(crate::LatestQueryLimits {
            max_chunk_bytes: 1,
            ..Default::default()
        });
        assert!(matches!(
            reader.read_at_with_budget_cancellable(stream, ArrivalTime(0), &mut budget, || false),
            Err(McapSourceError::QueryLimit {
                resource: "latest chunk bytes",
                ..
            })
        ));
        assert_eq!(reader.diagnostics().chunks_read, 0);
    }

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

    #[cfg(feature = "mcap-lz4")]
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
            Err(McapSourceError::MessageIndexRequired(found)) if found.stream == stream
        ));
        assert!(matches!(
            source.latest_before(&[stream], ArrivalTime(1_000_000_000)),
            Err(McapSourceError::MessageIndexRequired(found)) if found.stream == stream
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
            Err(McapSourceError::ChunkIndexRequired)
        ));
    }
}
