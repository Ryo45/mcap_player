//! Recording facts, requirements, indexed planning, and concrete MCAP access.

mod indexed_plan;
mod mcap_source;
mod range_query;
mod raw_message;
mod restore;
mod session_plan;
mod source_identity;
mod stream;

pub use indexed_plan::{
    IndexedChunkFact, IndexedPlanError, ensure_indexed, history_candidate_chunks,
    latest_candidate_chunks, persistent_candidate_chunks,
};
pub use mcap_source::{IndexedMessages, IndexedReadDiagnostics, McapOpenError, McapSource};
pub use range_query::{QueryLimits, RangeQuery, RangeQueryError, RangeQueryResult};
pub use raw_message::RawMessage;
pub use restore::{
    RestoreInput, RestorePlan, RestorePlanError, RestorePlanner, RestoreRead, RestoreSemantics,
};
pub use session_plan::{
    CameraRoute, PlaybackRequirements, SessionPlan, SessionPlanError, WorkspaceBindings,
};
pub use source_identity::{MCAP_SUMMARY_IDENTITY_ALGORITHM, McapSummaryIdentity};
pub use stream::{
    RecordingTimeRange, SourceCapabilities, SourceCatalog, StreamDescriptor, StreamId,
    StreamTimingSummary,
};
