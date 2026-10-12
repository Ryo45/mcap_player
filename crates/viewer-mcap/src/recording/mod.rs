//! Concrete MCAP access and format-specific indexed planning.
mod backing;
mod chunk;
mod error;
#[cfg(not(target_arch = "wasm32"))]
mod file_messages;
mod latest_limits;
mod message_index;
mod source;
mod source_identity;
pub use chunk::{McapChunkError, decode_mcap_chunk, visit_mcap_chunk_records};
#[cfg(not(target_arch = "wasm32"))]
pub use file_messages::FileMessageStream;
pub use latest_limits::{LatestQueryBudget, LatestQueryLimits};
pub use message_index::{
    IndexedChunkFact, IndexedPlanError, MessageIndexPredecessor, ensure_indexed,
    history_candidate_chunks, latest_candidate_chunks, persistent_candidate_chunks,
};
pub use source::{
    IndexedMessagePosition, IndexedMessages, IndexedReadDiagnostics, LatestMessageReader,
    LatestQueryDiagnostics, McapSource,
};
pub use source::{QueryLimits, RangeQuery, RangeQueryError, RangeQueryResult};
pub use source_identity::{MCAP_SUMMARY_IDENTITY_ALGORITHM, McapSummaryIdentity};

pub use error::McapSourceError;
