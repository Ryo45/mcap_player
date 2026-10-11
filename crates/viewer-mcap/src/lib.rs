#![forbid(unsafe_code)]

//! MCAP recording I/O, byte processing, indexed queries and playback transport.
//! Viewer domain types, codecs, reducers and feature state live in `viewer-core`.

// Internal modules use the domain vocabulary without re-exporting it as a facade.
pub(crate) use viewer_core::*;

mod derived;
mod playback;
mod query;
mod recording;
mod schema;

pub use derived::{
    NumericPlotDetail, NumericPlotError, NumericPlotOverview, NumericPlotOverviewBatch,
    NumericSample, NumericSeriesRequest, load_odometry_signals,
    load_odometry_signals_for_topic_with_progress, load_speed_signal, load_yaw_rate_signal,
    mcap_summary_fingerprint,
};
pub use playback::{McapPlayback, McapPlaybackError, McapSeekError};
pub use query::{
    DEFAULT_QUERY_WINDOW_SECONDS, DiagnosticEvent, DiagnosticsRequirement, DiagnosticsSnapshot,
    MAX_QUERY_WINDOW_SECONDS, TopicHealthRequirement, TopicHealthSnapshot, WindowQueryError,
    query_diagnostics, query_topic_health,
};
#[cfg(not(target_arch = "wasm32"))]
pub use recording::FileMessageStream;
pub use recording::{
    IndexedChunkFact, IndexedMessagePosition, IndexedMessages, IndexedPlanError,
    IndexedReadDiagnostics, LatestMessageReader, LatestQueryDiagnostics,
    MCAP_SUMMARY_IDENTITY_ALGORITHM, McapChunkError, McapOpenError, McapSource,
    McapSummaryIdentity, MessageIndexPredecessor, QueryLimits, RangeQuery, RangeQueryError,
    RangeQueryResult, decode_mcap_chunk, ensure_indexed, history_candidate_chunks,
    latest_candidate_chunks, persistent_candidate_chunks, visit_mcap_chunk_records,
};
pub use schema::schema_definition_from_mcap;
