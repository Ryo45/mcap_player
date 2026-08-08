//! Platform-neutral playback, MCAP indexing and camera-domain contracts.

mod bev;
mod bookmark;
mod camera;
mod camera_projection;
mod cdr;
mod clock;
mod domain;
mod frame_builder;
mod mcap_source;
mod performance;
mod pipeline;
mod playback;
mod playback_core;
mod plot;
mod point_cloud;
mod presentation;
mod preview;
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
pub use domain::DomainState;
pub use frame_builder::{
    BevFrameBuilder, BevSnapshot, SceneDiagnostics, SceneFrameBuilder, SceneSnapshot, SceneTfError,
};
pub use mcap_source::{McapOpenError, McapSource, StreamCatalog};
pub use performance::{PresentationMetrics, PresentationSnapshot};
pub use pipeline::{
    DomainUpdate, ODOM_TOPIC, PATH_TOPIC, PipelineCounters, PipelineSet, RawMessage, SCAN_TOPIC,
    StreamBinding, StreamDescriptor, StreamId, StreamPipeline, TF_STATIC_TOPIC, TF_TOPIC,
    camera_topics, standard_bindings,
};
pub use playback::{McapPlayback, McapPlaybackError, PlaybackEffect};
pub use playback_core::{PlaybackCore, PlaybackCoreError, PlaybackPerformance, StageTiming};
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
pub use source_identity::{
    MCAP_SUMMARY_IDENTITY_ALGORITHM, McapSummaryIdentity, mcap_summary_fingerprint,
};
pub use telemetry::{TelemetryFrame, TelemetryState};
pub use time::{ArrivalTime, CameraId, MeasurementTime};
pub use transform::{TransformBatch, TransformState};
