use crate::{
    ArrivalTime, BevPathFrame, CameraFrame, CameraId, DecodeError, PointCloudFrame, TelemetryFrame,
    TransformBatch, decode_compressed_image, decode_laser_scan, decode_odometry, decode_path,
    decode_tf_message,
};
use std::collections::HashMap;

pub const PATH_TOPIC: &str = "/planning/path";
pub const ODOM_TOPIC: &str = "/odom";
pub const SCAN_TOPIC: &str = "/scan";
pub const TF_TOPIC: &str = "/tf";
pub const TF_STATIC_TOPIC: &str = "/tf_static";

const OPTIONAL_BINDINGS: &[(&str, StreamBinding)] = &[
    (PATH_TOPIC, StreamBinding::Path),
    (ODOM_TOPIC, StreamBinding::Odometry),
    (SCAN_TOPIC, StreamBinding::LaserScan),
    (TF_TOPIC, StreamBinding::Transforms { is_static: false }),
    (
        TF_STATIC_TOPIC,
        StreamBinding::Transforms { is_static: true },
    ),
];

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StreamId(pub u32);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawMessage {
    pub stream_id: StreamId,
    pub arrival_time: ArrivalTime,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamDescriptor {
    pub id: StreamId,
    pub topic: String,
    pub schema: String,
    pub message_encoding: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamBinding {
    Camera(CameraId),
    Path,
    Odometry,
    LaserScan,
    Transforms { is_static: bool },
}

pub fn standard_bindings(
    catalog: &crate::StreamCatalog,
    camera_topic: &str,
) -> Result<Vec<(StreamId, StreamBinding)>, String> {
    let cameras = camera_topics(catalog, camera_topic)?;
    let mut bindings = cameras
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (*id, StreamBinding::Camera(CameraId(index as u16))))
        .collect::<Vec<_>>();
    bindings.extend(OPTIONAL_BINDINGS.iter().filter_map(|(topic, binding)| {
        catalog
            .by_topic(topic)
            .map(|descriptor| (descriptor.id, *binding))
    }));
    Ok(bindings)
}

pub fn camera_topics(
    catalog: &crate::StreamCatalog,
    primary_topic: &str,
) -> Result<Vec<(StreamId, String)>, String> {
    let mut cameras = catalog
        .streams
        .iter()
        .filter(|stream| stream.schema == "sensor_msgs/msg/CompressedImage")
        .map(|stream| (stream.id, stream.topic.clone()))
        .collect::<Vec<_>>();
    let Some(primary_index) = cameras.iter().position(|(_, topic)| topic == primary_topic) else {
        return Err(format!("topic {primary_topic} is not present"));
    };
    cameras.swap(0, primary_index);
    Ok(cameras)
}

#[derive(Clone, Debug, PartialEq)]
pub enum DomainUpdate {
    Camera(CameraFrame),
    Path(BevPathFrame),
    Telemetry(TelemetryFrame),
    PointCloud(PointCloudFrame),
    Transforms(TransformBatch),
}

struct PathPipeline;

impl StreamPipeline for PathPipeline {
    fn decode(
        &mut self,
        message: RawMessage,
        output: &mut Vec<DomainUpdate>,
    ) -> Result<(), DecodeError> {
        let path = decode_path(&message.payload)?;
        let points = path
            .points
            .into_iter()
            .map(|[forward, left]| [-left as f32, forward as f32])
            .collect();
        output.push(DomainUpdate::Path(BevPathFrame {
            measurement_time: path.measurement_time,
            arrival_time: message.arrival_time,
            frame_id: path.frame_id,
            points,
        }));
        Ok(())
    }
}

struct OdometryPipeline;

impl StreamPipeline for OdometryPipeline {
    fn decode(
        &mut self,
        message: RawMessage,
        output: &mut Vec<DomainUpdate>,
    ) -> Result<(), DecodeError> {
        let odometry = decode_odometry(&message.payload)?;
        let [qx, qy, qz, qw] = odometry.orientation;
        let sin_yaw = 2.0 * (qw * qz + qx * qy);
        let cos_yaw = 1.0 - 2.0 * (qy * qy + qz * qz);
        let [vx, vy, _] = odometry.linear_velocity;
        output.push(DomainUpdate::Telemetry(TelemetryFrame {
            measurement_time: odometry.measurement_time,
            arrival_time: message.arrival_time,
            frame_id: odometry.frame_id,
            child_frame_id: odometry.child_frame_id,
            position_x: odometry.position[0],
            position_y: odometry.position[1],
            yaw_radians: sin_yaw.atan2(cos_yaw),
            forward_velocity: vx,
            speed: vx.hypot(vy),
            yaw_rate: odometry.angular_velocity[2],
        }));
        Ok(())
    }
}

struct LaserScanPipeline;

impl StreamPipeline for LaserScanPipeline {
    fn decode(
        &mut self,
        message: RawMessage,
        output: &mut Vec<DomainUpdate>,
    ) -> Result<(), DecodeError> {
        let scan = decode_laser_scan(&message.payload)?;
        let mut points = Vec::with_capacity(scan.ranges.len());
        for (index, range) in scan.ranges.iter().copied().enumerate() {
            if !range.is_finite() || range < scan.range_min || range > scan.range_max {
                continue;
            }
            let angle = scan.angle_min + index as f32 * scan.angle_increment;
            points.push([range * angle.cos(), range * angle.sin(), 0.0]);
        }
        output.push(DomainUpdate::PointCloud(PointCloudFrame {
            measurement_time: scan.measurement_time,
            arrival_time: message.arrival_time,
            frame_id: scan.frame_id,
            points,
        }));
        Ok(())
    }
}

struct TransformPipeline {
    is_static: bool,
}

impl StreamPipeline for TransformPipeline {
    fn decode(
        &mut self,
        message: RawMessage,
        output: &mut Vec<DomainUpdate>,
    ) -> Result<(), DecodeError> {
        output.push(DomainUpdate::Transforms(TransformBatch {
            arrival_time: message.arrival_time,
            is_static: self.is_static,
            transforms: decode_tf_message(&message.payload)?,
        }));
        Ok(())
    }
}

pub trait StreamPipeline {
    fn decode(
        &mut self,
        message: RawMessage,
        output: &mut Vec<DomainUpdate>,
    ) -> Result<(), DecodeError>;
}

struct CompressedImagePipeline {
    camera_id: CameraId,
}

impl StreamPipeline for CompressedImagePipeline {
    fn decode(
        &mut self,
        message: RawMessage,
        output: &mut Vec<DomainUpdate>,
    ) -> Result<(), DecodeError> {
        let image = decode_compressed_image(&message.payload)?;
        output.push(DomainUpdate::Camera(CameraFrame {
            camera_id: self.camera_id,
            measurement_time: image.measurement_time,
            arrival_time: message.arrival_time,
            frame_id: image.frame_id,
            jpeg: image.jpeg,
        }));
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PipelineCounters {
    pub decoded: u64,
    pub errors: u64,
    pub unknown_streams: u64,
    /// Camera updates coalesced before presentation.
    pub dropped: u64,
}

pub struct PipelineSet {
    pipelines: HashMap<StreamId, Box<dyn StreamPipeline>>,
    counters: PipelineCounters,
}

impl PipelineSet {
    pub fn new(descriptors: &[StreamDescriptor], bindings: &[(StreamId, StreamBinding)]) -> Self {
        let mut pipelines: HashMap<StreamId, Box<dyn StreamPipeline>> = HashMap::new();
        for (id, binding) in bindings {
            let Some(descriptor) = descriptors.iter().find(|item| item.id == *id) else {
                continue;
            };
            if let Some(pipeline) = build_pipeline(descriptor, *binding) {
                pipelines.insert(*id, pipeline);
            }
        }
        Self {
            pipelines,
            counters: PipelineCounters::default(),
        }
    }

    pub fn decode(&mut self, message: RawMessage, output: &mut Vec<DomainUpdate>) {
        let Some(pipeline) = self.pipelines.get_mut(&message.stream_id) else {
            self.counters.unknown_streams += 1;
            return;
        };
        match pipeline.decode(message, output) {
            Ok(()) => self.counters.decoded += 1,
            Err(_) => self.counters.errors += 1,
        }
    }

    pub fn counters(&self) -> PipelineCounters {
        self.counters
    }

    pub(crate) fn add_counters(&mut self, counters: PipelineCounters) {
        self.counters.decoded = self.counters.decoded.saturating_add(counters.decoded);
        self.counters.errors = self.counters.errors.saturating_add(counters.errors);
        self.counters.unknown_streams = self
            .counters
            .unknown_streams
            .saturating_add(counters.unknown_streams);
    }
}

fn build_pipeline(
    descriptor: &StreamDescriptor,
    binding: StreamBinding,
) -> Option<Box<dyn StreamPipeline>> {
    let pipeline: Box<dyn StreamPipeline> = match binding {
        StreamBinding::Camera(camera_id)
            if descriptor.schema == "sensor_msgs/msg/CompressedImage" =>
        {
            Box::new(CompressedImagePipeline { camera_id })
        }
        StreamBinding::Path if descriptor.schema == "nav_msgs/msg/Path" => Box::new(PathPipeline),
        StreamBinding::Odometry if descriptor.schema == "nav_msgs/msg/Odometry" => {
            Box::new(OdometryPipeline)
        }
        StreamBinding::LaserScan if descriptor.schema == "sensor_msgs/msg/LaserScan" => {
            Box::new(LaserScanPipeline)
        }
        StreamBinding::Transforms { is_static }
            if descriptor.schema == "tf2_msgs/msg/TFMessage" =>
        {
            Box::new(TransformPipeline { is_static })
        }
        _ => return None,
    };
    Some(pipeline)
}
