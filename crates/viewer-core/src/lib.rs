//! Platform-neutral playback, MCAP indexing and camera-domain contracts.

mod bev;
mod bookmark;
mod camera;
mod camera_projection;
mod cdr;
mod clock;
pub mod data_window;
mod domain;
mod domain_runtime;
mod frame_builder;
mod mcap_source;
mod performance;
mod pipeline;
mod playback;
mod plot;
mod point_cloud;
mod presentation;
mod preview;
mod session_plan;
mod source_identity;
mod telemetry;
mod time;
mod transform;

pub use bev::{BevPathFrame, BevState};
pub use bookmark::{
    Bookmark, BookmarkDocument, BookmarkId, BookmarkValidationError,
    CURRENT_BOOKMARK_SCHEMA_VERSION, PreviewBuildInfo, SourceFingerprint,
};
pub use camera::{CameraFrame, CameraState, CameraStatus};
pub use camera_projection::{
    CalibrationError, CameraCalibration, CameraCalibrationSet, ProjectedPlan, ProjectionError,
};
pub use cdr::{
    CompressedImage, DecodeError, DecodedCompressedImage, LaserScan, Odometry, PathMessage,
    TransformStamped, decode_compressed_image, decode_compressed_image_bytes, decode_laser_scan,
    decode_odometry, decode_path, decode_tf_message, encode_compressed_image_cdr,
    encode_tf_message_cdr,
};
pub use clock::{
    PlaybackClock, PlaybackCommand, PlaybackLoadState, PlaybackSpeed, PlaybackView, SeekFidelity,
    SeekRequest,
};
pub use data_window::{
    DataWindowError, FetchDemand, FetchIntent, FetchPlanner, FetchProfile, MemoryWindowStore,
    SerializedWindow, TimeRange as DataWindowTimeRange,
};
pub use domain::DomainState;
pub use domain_runtime::{DomainPerformance, DomainRuntime, StageTiming};
pub use frame_builder::{
    BevFrameBuilder, BevSnapshot, SceneDiagnostics, SceneFrameBuilder, SceneSnapshot, SceneTfError,
};
pub use mcap_source::{McapOpenError, McapSource, StreamCatalog};
pub use performance::{PlaybackPerformance, PresentationMetrics, PresentationSnapshot};
pub use pipeline::{
    DomainPipeline, DomainPipelineError, DomainPipelineSet, DomainUpdate, PipelineCounters,
    RawMessage, StreamDescriptor, StreamId,
};
pub use playback::{McapPlayback, McapPlaybackError, PlaybackEffect};
pub use plot::{
    LoadedSignal, PlotMode, PlotPanelState, PlotSeries, PlotViewport, SignalId, SignalSample,
    arrival_time_from_plot_x, cursor_seconds, downsample_min_max, followed_viewport,
    load_speed_signal, sample_at_or_before, should_shift_viewport,
};
pub use point_cloud::{PointCloudFrame, PointCloudState};
pub use presentation::{
    CameraPresentation, DiagnosticsPresentation, OverlayStatus, TelemetryPresentation,
    ViewerPresentation,
};
pub use preview::{
    CURRENT_PREVIEW_SCHEMA_VERSION, CameraPreviewFrame, DataFidelity, PreviewBudget,
    PreviewImageEncoding, PreviewRequest, PreviewSnapshot, PreviewValidationError, SignalBucket,
    SignalFidelity, SignalOverview, TimeRange, TimedPosition2, merge_signal_buckets,
};
pub use session_plan::{
    DomainRoute, DomainTarget, ODOM_TOPIC, PATH_TOPIC, SCAN_TOPIC, SessionPlan, SessionPlanError,
    TF_STATIC_TOPIC, TF_TOPIC,
};
pub use source_identity::{
    MCAP_SUMMARY_IDENTITY_ALGORITHM, McapSummaryIdentity, mcap_summary_fingerprint,
};
pub use telemetry::{TelemetryFrame, TelemetryState};
pub use time::{ArrivalTime, CameraId, MeasurementTime};
pub use transform::{TransformBatch, TransformState};
