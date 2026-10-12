//! Work reserved for one exact latest-before demand, never for a reader lifetime.
use super::McapOpenError;

pub(crate) const MAX_LATEST_INDEX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct LatestQueryLimits {
    pub max_streams: usize,
    pub max_candidates: usize,
    pub max_index_reads: usize,
    pub max_index_bytes: usize,
    pub max_chunk_bytes: usize,
}
impl Default for LatestQueryLimits {
    fn default() -> Self {
        Self {
            max_streams: 64,
            max_candidates: 16_384,
            max_index_reads: 4096,
            max_index_bytes: MAX_LATEST_INDEX_BYTES,
            max_chunk_bytes: 256 * 1024 * 1024,
        }
    }
}

pub struct LatestQueryBudget {
    limits: LatestQueryLimits,
    streams: usize,
    candidates: usize,
    index_reads: usize,
    index_bytes: usize,
    chunk_bytes: usize,
}
impl Default for LatestQueryBudget {
    fn default() -> Self {
        Self::new(LatestQueryLimits::default())
    }
}
impl LatestQueryBudget {
    pub fn new(limits: LatestQueryLimits) -> Self {
        Self {
            limits,
            streams: 0,
            candidates: 0,
            index_reads: 0,
            index_bytes: 0,
            chunk_bytes: 0,
        }
    }
    fn reserve(
        used: &mut usize,
        amount: usize,
        maximum: usize,
        resource: &'static str,
    ) -> Result<(), McapOpenError> {
        if amount > maximum.saturating_sub(*used) {
            return Err(McapOpenError::QueryLimit { resource, maximum });
        }
        *used += amount;
        Ok(())
    }
    pub(crate) fn streams(&mut self, count: usize) -> Result<(), McapOpenError> {
        Self::reserve(
            &mut self.streams,
            count,
            self.limits.max_streams,
            "latest streams",
        )
    }
    pub(crate) fn candidate(&mut self) -> Result<(), McapOpenError> {
        Self::reserve(
            &mut self.candidates,
            1,
            self.limits.max_candidates,
            "latest chunk candidates",
        )
    }
    pub(crate) fn index(&mut self, bytes: usize) -> Result<(), McapOpenError> {
        if bytes > MAX_LATEST_INDEX_BYTES {
            return Err(McapOpenError::QueryLimit {
                resource: "latest index span bytes",
                maximum: MAX_LATEST_INDEX_BYTES,
            });
        }
        if self.index_reads >= self.limits.max_index_reads {
            return Err(McapOpenError::QueryLimit {
                resource: "latest index reads",
                maximum: self.limits.max_index_reads,
            });
        }
        Self::reserve(
            &mut self.index_bytes,
            bytes,
            self.limits.max_index_bytes,
            "latest index bytes",
        )?;
        self.index_reads += 1;
        Ok(())
    }
    pub(crate) fn chunk(
        &mut self,
        compressed: u64,
        uncompressed: u64,
    ) -> Result<(), McapOpenError> {
        let bytes = compressed
            .checked_add(uncompressed)
            .and_then(|sum| usize::try_from(sum).ok())
            .ok_or(McapOpenError::ReadOverflow)?;
        Self::reserve(
            &mut self.chunk_bytes,
            bytes,
            self.limits.max_chunk_bytes,
            "latest chunk bytes",
        )
    }
}
