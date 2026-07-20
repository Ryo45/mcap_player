//! Platform-neutral playback, MCAP indexing and camera-domain contracts.

mod bev;
mod camera;
mod cdr;
mod clock;
mod domain;
mod mcap_source;
mod pipeline;
mod playback;
mod point_cloud;
mod telemetry;
mod time;
mod transform;

pub use bev::{BevPathFrame, BevState};
pub use camera::{CameraFrame, CameraState, CameraStatus};
pub use cdr::{
    CompressedImage, DecodeError, LaserScan, Odometry, PathMessage, TransformStamped,
    decode_compressed_image, decode_laser_scan, decode_odometry, decode_path, decode_tf_message,
    encode_compressed_image_cdr,
};
pub use clock::{PlaybackClock, PlaybackSpeed};
pub use domain::DomainState;
pub use mcap_source::{McapOpenError, McapSource, StreamCatalog};
pub use pipeline::{
    DomainUpdate, ODOM_TOPIC, PATH_TOPIC, PipelineCounters, PipelineSet, RawMessage, SCAN_TOPIC,
    StreamBinding, StreamDescriptor, StreamId, StreamPipeline, TF_STATIC_TOPIC, TF_TOPIC,
    standard_bindings,
};
pub use playback::{McapPlayback, McapPlaybackError};
pub use point_cloud::{PointCloudFrame, PointCloudState};
pub use telemetry::{TelemetryFrame, TelemetryState};
pub use time::{ArrivalTime, CameraId, MeasurementTime};
pub use transform::{TransformBatch, TransformState};
