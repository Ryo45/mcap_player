//! Platform-neutral playback, MCAP indexing and camera-domain contracts.

mod bev;
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
mod point_cloud;
mod presentation;
mod telemetry;
mod time;
mod transform;

pub use bev::{BevPathFrame, BevState};
pub use camera::{CameraFrame, CameraState, CameraStatus};
pub use camera_projection::{
    CalibrationError, CameraCalibration, CameraCalibrationSet, ProjectedPlan, ProjectionError,
};
pub use cdr::{
    CompressedImage, DecodeError, LaserScan, Odometry, PathMessage, TransformStamped,
    decode_compressed_image, decode_laser_scan, decode_odometry, decode_path, decode_tf_message,
    encode_compressed_image_cdr, encode_tf_message_cdr,
};
pub use clock::{PlaybackClock, PlaybackSpeed};
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
pub use playback::{McapPlayback, McapPlaybackError, PlaybackPerformance, StageTiming};
pub use point_cloud::{PointCloudFrame, PointCloudState};
pub use presentation::{
    CameraPresentation, DiagnosticsPresentation, OverlayStatus, TelemetryPresentation,
    ViewerPresentation,
};
pub use telemetry::{TelemetryFrame, TelemetryState};
pub use time::{ArrivalTime, CameraId, MeasurementTime};
pub use transform::{TransformBatch, TransformState};
