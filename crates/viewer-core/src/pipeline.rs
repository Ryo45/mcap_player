use crate::{
    ArrivalTime, BevPathFrame, CameraFrame, CameraId, DecodeError, PointCloudFrame, TelemetryFrame,
    TransformBatch, decode_compressed_image, decode_laser_scan, decode_odometry, decode_path,
    decode_tf_message,
};
use std::collections::HashMap;

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
            match binding {
                StreamBinding::Camera(camera_id)
                    if descriptor.schema == "sensor_msgs/msg/CompressedImage" =>
                {
                    pipelines.insert(
                        *id,
                        Box::new(CompressedImagePipeline {
                            camera_id: *camera_id,
                        }),
                    );
                }
                StreamBinding::Camera(_) => {}
                StreamBinding::Path if descriptor.schema == "nav_msgs/msg/Path" => {
                    pipelines.insert(*id, Box::new(PathPipeline));
                }
                StreamBinding::Path => {}
                StreamBinding::Odometry if descriptor.schema == "nav_msgs/msg/Odometry" => {
                    pipelines.insert(*id, Box::new(OdometryPipeline));
                }
                StreamBinding::Odometry => {}
                StreamBinding::LaserScan if descriptor.schema == "sensor_msgs/msg/LaserScan" => {
                    pipelines.insert(*id, Box::new(LaserScanPipeline));
                }
                StreamBinding::LaserScan => {}
                StreamBinding::Transforms { is_static }
                    if descriptor.schema == "tf2_msgs/msg/TFMessage" =>
                {
                    pipelines.insert(
                        *id,
                        Box::new(TransformPipeline {
                            is_static: *is_static,
                        }),
                    );
                }
                StreamBinding::Transforms { .. } => {}
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
}
