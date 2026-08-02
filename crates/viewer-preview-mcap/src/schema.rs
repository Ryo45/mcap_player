use serde::{Deserialize, Serialize};
use viewer_core::{
    ArrivalTime, CameraId, MeasurementTime, PreviewBuildInfo, SignalBucket, SignalId,
    SourceFingerprint, TimedPosition2,
};

pub const BUILD_INFO_TOPIC: &str = "/preview/build_info";
pub const TRAJECTORY_TOPIC: &str = "/preview/trajectory";
pub const CAMERA_TOPIC_PREFIX: &str = "/preview/camera/";
pub const SIGNAL_TOPIC_PREFIX: &str = "/preview/signal/";
pub const WIRE_SCHEMA_VERSION: u32 = 1;

pub fn camera_topic(camera_id: CameraId) -> String {
    format!("{CAMERA_TOPIC_PREFIX}{}", camera_id.0)
}

pub fn parse_camera_topic(topic: &str) -> Option<CameraId> {
    topic
        .strip_prefix(CAMERA_TOPIC_PREFIX)?
        .parse::<u16>()
        .ok()
        .map(CameraId)
}

pub fn signal_topic(signal_id: SignalId) -> String {
    match signal_id {
        SignalId::Speed => format!("{SIGNAL_TOPIC_PREFIX}speed"),
    }
}

pub fn parse_signal_topic(topic: &str) -> Option<SignalId> {
    match topic.strip_prefix(SIGNAL_TOPIC_PREFIX)? {
        "speed" => Some(SignalId::Speed),
        _ => None,
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BuildInfoWire {
    pub preview_schema_version: u32,
    pub generator_name: String,
    pub generator_version: String,
    pub source_fingerprint: SourceFingerprint,
}

impl From<&PreviewBuildInfo> for BuildInfoWire {
    fn from(info: &PreviewBuildInfo) -> Self {
        Self {
            preview_schema_version: info.schema_version(),
            generator_name: info.generator_name().to_owned(),
            generator_version: info.generator_version().to_owned(),
            source_fingerprint: info.source().clone(),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CameraMetadataWire {
    pub schema_version: u32,
    pub camera_id: CameraId,
    pub measurement_time: Option<MeasurementTime>,
    pub arrival_time: ArrivalTime,
    pub frame_id: String,
    pub encoding: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignalBucketWire {
    pub schema_version: u32,
    pub signal_id: SignalId,
    pub bucket_start: ArrivalTime,
    pub bucket_end: ArrivalTime,
    pub first: f64,
    pub last: f64,
    pub min: f64,
    pub max: f64,
    pub count: u32,
    pub bucket_ns: i64,
}

impl SignalBucketWire {
    pub(crate) fn from_bucket(signal_id: SignalId, bucket_ns: i64, bucket: SignalBucket) -> Self {
        Self {
            schema_version: WIRE_SCHEMA_VERSION,
            signal_id,
            bucket_start: bucket.start_time(),
            bucket_end: bucket.end_time(),
            first: bucket.first(),
            last: bucket.last(),
            min: bucket.min(),
            max: bucket.max(),
            count: bucket.count(),
            bucket_ns,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrajectoryWire {
    pub schema_version: u32,
    pub time: ArrivalTime,
    pub position: [f32; 2],
}

impl From<TimedPosition2> for TrajectoryWire {
    fn from(point: TimedPosition2) -> Self {
        Self {
            schema_version: WIRE_SCHEMA_VERSION,
            time: point.time(),
            position: point.position(),
        }
    }
}
