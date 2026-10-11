//! Concrete MCAP access and format-specific indexed planning.
mod backing;
mod chunk;
#[cfg(not(target_arch = "wasm32"))]
mod file_messages;
mod indexed_plan;
mod mcap_source;
mod range_query;
mod source_identity;
pub use chunk::{McapChunkError, decode_mcap_chunk, visit_mcap_chunk_records};
#[cfg(not(target_arch = "wasm32"))]
pub use file_messages::FileMessageStream;
pub use indexed_plan::{
    IndexedChunkFact, IndexedPlanError, MessageIndexPredecessor, ensure_indexed,
    history_candidate_chunks, latest_candidate_chunks, persistent_candidate_chunks,
};
pub use mcap_source::{
    IndexedMessagePosition, IndexedMessages, IndexedReadDiagnostics, LatestMessageReader,
    LatestQueryDiagnostics, McapOpenError, McapSource,
};
pub use range_query::{QueryLimits, RangeQuery, RangeQueryError, RangeQueryResult};
pub use source_identity::{MCAP_SUMMARY_IDENTITY_ALGORITHM, McapSummaryIdentity};
