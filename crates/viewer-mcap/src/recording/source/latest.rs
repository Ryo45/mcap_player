//! Independent exact latest-before reader with bounded serialized/index caches.
use super::{
    McapSource, ParsedChunkMessage,
    index::{ChunkMessageIndexes, IndexedMessagePosition, LatestIndexRead},
};
use crate::McapSourceError;
use std::collections::HashMap;
use viewer_core::{ArrivalTime, RawMessage, SourceCatalog, StreamId};
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LatestQueryDiagnostics {
    pub index_reads: u64,
    pub chunks_read: u64,
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
    ) -> Result<Option<(IndexedMessagePosition, RawMessage)>, McapSourceError> {
        self.read_at_cancellable(stream, target, || false)
    }
    pub fn read_at_cancellable(
        &mut self,
        stream: StreamId,
        target: ArrivalTime,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Option<(IndexedMessagePosition, RawMessage)>, McapSourceError> {
        self.read_at_with_budget_cancellable(
            stream,
            target,
            &mut crate::LatestQueryBudget::default(),
            &mut cancelled,
        )
    }

    pub fn read_at_with_budget_cancellable(
        &mut self,
        stream: StreamId,
        target: ArrivalTime,
        budget: &mut crate::LatestQueryBudget,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Option<(IndexedMessagePosition, RawMessage)>, McapSourceError> {
        if cancelled() {
            return Err(McapSourceError::QueryCancelled);
        }
        budget.streams(1)?;
        self.source.backing.validate()?;
        if target < self.source.range.start {
            return Ok(None);
        }
        let target = u64::try_from(target.0).map_err(|_| McapSourceError::TimestampOverflow)?;
        let position = self.source.latest_position(
            stream,
            target,
            &mut LatestIndexRead {
                cache: &mut self.indexes,
                reads: &mut self.diagnostics.index_reads,
                budget,
                cancelled: &mut cancelled,
            },
        )?;
        if cancelled() {
            return Err(McapSourceError::QueryCancelled);
        }
        self.source.backing.validate()?;
        let Some(position) = position else {
            return Ok(None);
        };
        if self
            .chunk
            .as_ref()
            .is_none_or(|(chunk, _)| *chunk != position.chunk)
        {
            let chunk = &self.source.summary.chunk_indexes[position.chunk];
            budget.chunk(chunk.compressed_size, chunk.uncompressed_size)?;
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
        if cancelled() {
            return Err(McapSourceError::QueryCancelled);
        }
        self.source.backing.validate()?;
        Ok(Some((position, message.message.clone())))
    }
}
