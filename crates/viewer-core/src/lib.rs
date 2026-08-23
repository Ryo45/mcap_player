//! Platform-neutral recording access, playback planning, and feature-controller contracts.
//!
//! Internal modules follow the data flow instead of exposing one flat implementation namespace:
//!
//! - `message`: time primitives and concrete ROS CDR codecs;
//! - `recording`: catalogs, requirements, indexed plans, serialized records, and MCAP access;
//! - `playback`: clocks, bounded windows, native playback orchestration, and stage timing;
//! - `feature`: concrete continuous state, reducers, and transactional runtime ownership;
//! - `derived`: bounded Plot/Preview products and bookmark artifacts;
//! - `presentation`: CPU-visible presentation state and frontend-facing snapshots.
//!
//! Public consumers continue to use the explicit root exports below. The internal modules stay
//! private so their layout describes ownership without becoming a second supported API surface.

mod derived;
mod feature;
mod message;
mod playback;
mod presentation;
mod recording;

pub use derived::{
    Bookmark, BookmarkDocument, BookmarkId, BookmarkValidationError,
    CURRENT_BOOKMARK_SCHEMA_VERSION, CURRENT_PREVIEW_SCHEMA_VERSION, CameraPreviewFrame,
    DataFidelity, LoadedOdometrySignals, LoadedSignal, PlotSeries, PreviewBudget, PreviewBuildInfo,
    PreviewImageEncoding, PreviewRequest, PreviewSnapshot, PreviewValidationError, SignalBucket,
    SignalFidelity, SignalId, SignalOverview, SignalOverviewReducer, SignalSample,
    SourceFingerprint, TimeRange, TimedPosition2, arrival_time_from_plot_x, cursor_seconds,
    load_odometry_signals, load_odometry_signals_for_topic_with_progress, load_speed_signal,
    load_yaw_rate_signal, mcap_summary_fingerprint, merge_signal_buckets,
};
pub use feature::{
    BevPathFrame, BevState, CameraController, CameraFrame, CameraId, CameraState, CameraStatus,
    DYNAMIC_TF_HISTORY, FeatureRestoreError, FeatureRestoreErrorKind, FeatureRuntime,
    OdometryController, PathController, PlaybackPerformance, PointCloudFrame, PointCloudState,
    ProcessingCounters, SceneController, TelemetryFrame, TelemetryState, TransformBatch,
    TransformController, TransformState,
};
pub use message::{
    ArrivalTime, CompressedImage, DecodeError, DecodedCompressedImage, LaserScan, MeasurementTime,
    Odometry, PathMessage, TransformStamped, decode_compressed_image,
    decode_compressed_image_bytes, decode_laser_scan, decode_odometry, decode_path,
    decode_tf_message, encode_compressed_image_cdr, encode_tf_message_cdr,
};
pub use playback::{
    DataWindowError, DataWindowTimeRange, FetchDemand, FetchIntent, FetchPlanner, FetchProfile,
    McapPlayback, McapPlaybackError, McapSeekError, MemoryWindowStore, PlaybackClock,
    PlaybackCommand, PlaybackEffect, PlaybackLoadState, PlaybackSpeed, PlaybackView,
    SerializedWindow, StageTiming,
};
pub use presentation::{
    BevFrameBuilder, BevSnapshot, CalibrationError, CameraCalibration, CameraCalibrationSet,
    CameraPresentation, DiagnosticsPresentation, OverlayStatus, PresentationMetrics,
    PresentationSnapshot, ProjectedPlan, ProjectionError, SceneDiagnostics, ScenePresentationState,
    SceneSnapshot, SceneTfError, TelemetryPresentation, ViewerPresentation,
    ViewerPresentationInput,
};
pub use recording::{
    CameraRoute, IndexedChunkFact, IndexedMessages, IndexedPlanError, IndexedReadDiagnostics,
    MCAP_SUMMARY_IDENTITY_ALGORITHM, McapOpenError, McapSource, McapSummaryIdentity,
    PlaybackRequirements, QueryLimits, RangeQuery, RangeQueryError, RangeQueryResult, RawMessage,
    RecordingTimeRange, RestoreInput, RestorePlan, RestorePlanError, RestorePlanner, RestoreRead,
    RestoreSemantics, SessionPlan, SessionPlanError, SourceCapabilities, SourceCatalog,
    StreamDescriptor, StreamId, StreamTimingSummary, WorkspaceBindings, ensure_indexed,
    history_candidate_chunks, latest_candidate_chunks, persistent_candidate_chunks,
};
