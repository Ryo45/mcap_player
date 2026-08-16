//! Platform-neutral recording access, playback planning, and feature-controller contracts.

mod bev;
mod bookmark;
mod camera;
mod camera_projection;
mod cdr;
mod clock;
mod controllers;
pub mod data_window;
mod frame_builder;
mod mcap_source;
mod performance;
mod playback;
mod plot;
mod point_cloud;
mod presentation;
mod preview;
mod range_query;
mod raw_message;
mod restore;
mod session_plan;
mod source_identity;
mod stream;
mod telemetry;
mod time;
mod transform;

pub use bev::{BevPathFrame, BevState};
pub use bookmark::{
    Bookmark, BookmarkDocument, BookmarkId, BookmarkValidationError,
    CURRENT_BOOKMARK_SCHEMA_VERSION, PreviewBuildInfo, SourceFingerprint,
};
pub use camera::{CameraFrame, CameraId, CameraState, CameraStatus};
pub use camera_projection::{
    CalibrationError, CameraCalibration, CameraCalibrationSet, ProjectedPlan, ProjectionError,
};
pub use cdr::{
    CompressedImage, DecodeError, DecodedCompressedImage, LaserScan, Odometry, PathMessage,
    TransformStamped, decode_compressed_image, decode_compressed_image_bytes, decode_laser_scan,
    decode_odometry, decode_path, decode_tf_message, encode_compressed_image_cdr,
    encode_tf_message_cdr,
};
pub use clock::{PlaybackClock, PlaybackCommand, PlaybackLoadState, PlaybackSpeed, PlaybackView};
pub use controllers::{
    CameraController, OdometryController, PathController, ProcessingCounters, SceneController,
    TransformController,
};
pub use data_window::{
    DataWindowError, FetchDemand, FetchIntent, FetchPlanner, FetchProfile, MemoryWindowStore,
    SerializedWindow, TimeRange as DataWindowTimeRange,
};
pub use frame_builder::{
    BevFrameBuilder, BevSnapshot, SceneDiagnostics, SceneFrameBuilder, SceneSnapshot, SceneTfError,
};
pub use mcap_source::{IndexedMessages, IndexedReadDiagnostics, McapOpenError, McapSource};
pub use performance::{
    PlaybackPerformance, PresentationMetrics, PresentationSnapshot, StageTiming,
};
pub use playback::{McapPlayback, McapPlaybackError, PlaybackEffect};
pub use plot::{
    LoadedOdometrySignals, LoadedSignal, PlotMode, PlotPanelState, PlotSeries, PlotViewport,
    SignalId, SignalSample, arrival_time_from_plot_x, cursor_seconds, downsample_min_max,
    followed_viewport, load_odometry_signals, load_odometry_signals_for_topic_with_progress,
    load_speed_signal, load_yaw_rate_signal, sample_at_or_before, should_shift_viewport,
};
pub use point_cloud::{PointCloudFrame, PointCloudState};
pub use presentation::{
    CameraPresentation, DiagnosticsPresentation, OverlayStatus, TelemetryPresentation,
    ViewerPresentation, ViewerPresentationInput,
};
pub use preview::{
    CURRENT_PREVIEW_SCHEMA_VERSION, CameraPreviewFrame, DataFidelity, PreviewBudget,
    PreviewImageEncoding, PreviewRequest, PreviewSnapshot, PreviewValidationError, SignalBucket,
    SignalFidelity, SignalOverview, TimeRange, TimedPosition2, merge_signal_buckets,
};
pub use range_query::{QueryLimits, RangeQuery, RangeQueryError, RangeQueryResult};
pub use raw_message::RawMessage;
pub use restore::{
    RestoreInput, RestorePlan, RestorePlanError, RestorePlanner, RestoreRead, RestoreSemantics,
};
pub use session_plan::{
    CameraRoute, PlaybackRequirements, SessionPlan, SessionPlanError, WorkspaceBindings,
};
pub use source_identity::{
    MCAP_SUMMARY_IDENTITY_ALGORITHM, McapSummaryIdentity, mcap_summary_fingerprint,
};
pub use stream::{
    RecordingTimeRange, SourceCatalog, StreamDescriptor, StreamId, StreamTimingSummary,
};
pub use telemetry::{TelemetryFrame, TelemetryState};
pub use time::{ArrivalTime, MeasurementTime};
pub use transform::{DYNAMIC_TF_HISTORY, TransformBatch, TransformState};
