#![forbid(unsafe_code)]

//! MCAP recording I/O, byte processing, indexed queries and playback transport.
//! Viewer domain types, codecs, reducers and feature state live in `viewer-core`.

mod derived;
mod playback;
mod query;
mod recording;
mod schema;

pub use derived::mcap_summary_fingerprint;
pub use playback::{McapPlayback, McapPlaybackError, McapSeekError};
pub use query::{
    DEFAULT_QUERY_WINDOW_SECONDS, DiagnosticsRequirement, MAX_QUERY_WINDOW_SECONDS,
    TopicHealthRequirement, TopicHealthSnapshot, WindowQueryError, query_diagnostics,
    query_topic_health,
};
pub use query::{
    NumericPlotDetail, NumericPlotError, NumericPlotOverview, NumericPlotOverviewBatch,
    NumericSample, NumericSeriesRequest, load_odometry_signals,
    load_odometry_signals_for_topic_with_progress, load_speed_signal, load_yaw_rate_signal,
};
#[cfg(not(target_arch = "wasm32"))]
pub use recording::FileMessageStream;
pub use recording::{
    IndexedChunkFact, IndexedMessagePosition, IndexedMessages, IndexedPlanError,
    IndexedReadDiagnostics, LatestMessageReader, LatestQueryBudget, LatestQueryDiagnostics,
    LatestQueryLimits, MCAP_SUMMARY_IDENTITY_ALGORITHM, McapChunkError, McapChunkStream,
    McapSource, McapSourceError, McapSummaryIdentity, MessageIndexPredecessor, QueryLimits,
    RangeQuery, RangeQueryError, RangeQueryResult, SourceReadDiagnostics, decode_mcap_chunk,
    ensure_indexed, history_candidate_chunks, latest_candidate_chunks, persistent_candidate_chunks,
    visit_mcap_chunk_records,
};
pub use schema::schema_definition_from_mcap;
