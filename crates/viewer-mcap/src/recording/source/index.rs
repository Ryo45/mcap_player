//! Physical Message Index spans, chunk timelines and exact predecessor lookup.
use super::McapSource;
use crate::{ArrivalTime, McapSourceError, StreamId};
use mcap::records::op;
use std::collections::HashMap;
pub(super) struct LatestIndexRead<'a> {
    pub(super) cache: &'a mut HashMap<usize, ChunkMessageIndexes>,
    pub(super) reads: &'a mut u64,
    pub(super) budget: &'a mut crate::LatestQueryBudget,
    pub(super) cancelled: &'a mut dyn FnMut() -> bool,
}
impl LatestIndexRead<'_> {
    fn check(&mut self) -> Result<(), McapSourceError> {
        if (self.cancelled)() {
            return Err(McapSourceError::QueryCancelled);
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct StreamChunks {
    pub(super) chunks: Vec<usize>,
    pub(super) prefix_max_end: Vec<ArrivalTime>,
    pub(super) prefix_max_chunk: Vec<usize>,
}

/// Source-local exact record identity, for query view reuse. Not a persistent/global message ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndexedMessagePosition {
    pub chunk: usize,
    pub record_offset: u64,
}
pub(crate) struct StreamChunkTimeline<'a> {
    pub(crate) chunks: &'a [usize],
    pub(crate) prefix_max_end: &'a [ArrivalTime],
}

pub(crate) struct ChunkMessageIndexes {
    pub(crate) channels: HashMap<u16, Box<[mcap::records::MessageIndexEntry]>>,
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
        channels: &HashMap<u16, Box<[mcap::records::MessageIndexEntry]>>,
    ) -> Self {
        use std::sync::atomic::Ordering::Relaxed;
        let entry_bytes = channels
            .values()
            .map(|entries| entries.len() * std::mem::size_of::<mcap::records::MessageIndexEntry>())
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

impl McapSource {
    pub(super) fn latest_position(
        &self,
        stream: StreamId,
        target: u64,
        read: &mut LatestIndexRead<'_>,
    ) -> Result<Option<IndexedMessagePosition>, McapSourceError> {
        self.indexed_summary(&[stream])?;
        let count = self
            .catalog
            .by_id(stream)
            .and_then(|s| s.timing.message_count);
        let Some(sparse) = self.stream_chunks.get(&stream) else {
            return if count == Some(0) {
                Ok(None)
            } else {
                Err(McapSourceError::MessageIndexRequired(
                    crate::IndexedPlanError { stream },
                ))
            };
        };
        let target_time =
            ArrivalTime(i64::try_from(target).map_err(|_| McapSourceError::TimestampOverflow)?);
        let end = sparse
            .chunks
            .partition_point(|chunk| self.index_facts[*chunk].start <= target_time);
        let mut search = crate::MessageIndexPredecessor::new(target);
        for cursor in (0..end).rev() {
            read.check()?;
            read.budget.candidate()?;
            if !search.can_improve(
                sparse.prefix_max_end[cursor].0 as u64,
                sparse.prefix_max_chunk[cursor],
            ) {
                break;
            }
            let chunk = sparse.chunks[cursor];
            if !read.cache.contains_key(&chunk) {
                let length =
                    usize::try_from(self.summary.chunk_indexes[chunk].message_index_length)
                        .map_err(|_| McapSourceError::ReadOverflow)?;
                read.budget.index(length)?;
                // One physical accelerator span, including multi-stream restore. Release the
                // previous parsed entries before allocating the next span.
                read.cache.clear();
                *read.reads = read.reads.saturating_add(1);
                let parsed = self.read_chunk_message_indexes_bounded(
                    chunk,
                    &mut *read.cancelled,
                    super::super::latest_limits::MAX_LATEST_INDEX_BYTES,
                )?;
                read.cache.insert(chunk, parsed);
            }
            let channel = u16::try_from(stream.0).map_err(|_| mcap::McapError::BadIndex)?;
            if let Some(entries) = read.cache[&chunk].channels.get(&channel) {
                search.consider_sorted(chunk, entries);
            }
        }
        read.check()?;
        Ok(search.result().map(|(_, position)| position))
    }
    /// Physical Message Index span access. No traversal, cache or query-budget policy.
    pub(crate) fn read_chunk_message_indexes(
        &self,
        chunk_index: usize,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<ChunkMessageIndexes, McapSourceError> {
        self.read_chunk_message_indexes_bounded(
            chunk_index,
            &mut cancelled,
            super::super::backing::MAX_SOURCE_RECORD_BYTES,
        )
    }
    pub(super) fn read_chunk_message_indexes_bounded(
        &self,
        chunk_index: usize,
        mut cancelled: impl FnMut() -> bool,
        maximum_entry_bytes: usize,
    ) -> Result<ChunkMessageIndexes, McapSourceError> {
        if cancelled() {
            return Err(McapSourceError::QueryCancelled);
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
        let length = usize::try_from(chunk.message_index_length)
            .map_err(|_| McapSourceError::ReadOverflow)?;
        // Message Index records are contiguous after their chunk. Read that bounded span once.
        #[cfg(test)]
        self.index_metrics
            .reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let bytes = self.backing.read(start, length)?;
        let mut entry_bytes = 0usize;
        for (channel, offset) in &chunk.message_index_offsets {
            if cancelled() {
                return Err(McapSourceError::QueryCancelled);
            }
            let relative = usize::try_from(
                offset
                    .checked_sub(start)
                    .ok_or(McapSourceError::ReadOverflow)?,
            )
            .map_err(|_| McapSourceError::ReadOverflow)?;
            let header_end = relative
                .checked_add(9)
                .ok_or(McapSourceError::ReadOverflow)?;
            let head = bytes
                .get(relative..header_end)
                .ok_or(mcap::McapError::BadIndex)?;
            if head[0] != op::MESSAGE_INDEX {
                return Err(mcap::McapError::BadIndex.into());
            }
            let length = usize::try_from(u64::from_le_bytes(head[1..9].try_into().unwrap()))
                .map_err(|_| McapSourceError::ReadOverflow)?;
            let body_end = header_end
                .checked_add(length)
                .ok_or(McapSourceError::ReadOverflow)?;
            let body = bytes
                .get(header_end..body_end)
                .ok_or(mcap::McapError::BadIndex)?;
            // Validate the declared vector size before the library parser allocates entries.
            let entry_length = body.get(2..6).ok_or(mcap::McapError::BadIndex)?;
            let entry_length = u32::from_le_bytes(entry_length.try_into().unwrap()) as usize;
            if !entry_length.is_multiple_of(16) || entry_length > body.len().saturating_sub(6) {
                return Err(mcap::McapError::BadIndex.into());
            }
            if entry_length > maximum_entry_bytes.saturating_sub(entry_bytes) {
                return Err(McapSourceError::QueryLimit {
                    resource: "parsed index entry bytes",
                    maximum: maximum_entry_bytes,
                });
            }
            entry_bytes += entry_length;
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
            indexes.insert(*channel, records.into_boxed_slice());
        }
        if cancelled() {
            return Err(McapSourceError::QueryCancelled);
        }
        Ok(ChunkMessageIndexes {
            #[cfg(test)]
            _resident: ResidentIndexSpan::new(self.index_metrics.clone(), &indexes),
            channels: indexes,
        })
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
}
