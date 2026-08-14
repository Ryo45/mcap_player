use crate::{
    ArrivalTime, BevPathFrame, CameraFrame, CameraId, DecodeError, PointCloudFrame, TelemetryFrame,
    TransformBatch, decode_compressed_image_bytes, decode_laser_scan, decode_odometry, decode_path,
    decode_tf_message,
};
use bytes::Bytes;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StreamId(pub u32);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawMessage {
    pub stream_id: StreamId,
    pub arrival_time: ArrivalTime,
    pub payload: Bytes,
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
    crate::SessionPlan::build(catalog, camera_topic)
        .map(|plan| plan.stream_bindings())
        .map_err(|error| error.to_string())
}

pub fn camera_topics(
    catalog: &crate::StreamCatalog,
    primary_topic: &str,
) -> Result<Vec<(StreamId, String)>, String> {
    crate::SessionPlan::build(catalog, primary_topic)
        .map(|plan| {
            plan.domain_routes()
                .iter()
                .filter_map(|route| match route.target {
                    crate::DomainTarget::Camera(_) => {
                        Some((route.stream.id, route.stream.topic.clone()))
                    }
                    _ => None,
                })
                .collect()
        })
        .map_err(|error| error.to_string())
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
        let image = decode_compressed_image_bytes(message.payload)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CompressedImage, MeasurementTime, ODOM_TOPIC, PATH_TOPIC, SCAN_TOPIC, StreamCatalog,
        TF_STATIC_TOPIC, TF_TOPIC, encode_compressed_image_cdr,
    };

    fn descriptor(id: u32, topic: &str, schema: &str) -> StreamDescriptor {
        StreamDescriptor {
            id: StreamId(id),
            topic: topic.into(),
            schema: schema.into(),
            message_encoding: "cdr".into(),
        }
    }

    #[test]
    fn standard_bindings_capture_current_session_routing_policy() {
        let catalog = StreamCatalog {
            streams: vec![
                descriptor(
                    9,
                    "/camera/rear/image/compressed",
                    "sensor_msgs/msg/CompressedImage",
                ),
                descriptor(20, PATH_TOPIC, "nav_msgs/msg/Path"),
                descriptor(
                    7,
                    "/camera/front/image/compressed",
                    "sensor_msgs/msg/CompressedImage",
                ),
                descriptor(21, ODOM_TOPIC, "nav_msgs/msg/Odometry"),
                descriptor(22, SCAN_TOPIC, "sensor_msgs/msg/LaserScan"),
                descriptor(23, TF_TOPIC, "tf2_msgs/msg/TFMessage"),
                descriptor(24, TF_STATIC_TOPIC, "tf2_msgs/msg/TFMessage"),
                descriptor(
                    11,
                    "/camera/left/image/compressed",
                    "sensor_msgs/msg/CompressedImage",
                ),
                descriptor(30, "/unrelated", "example_msgs/msg/Other"),
            ],
        };

        assert_eq!(
            camera_topics(&catalog, "/camera/front/image/compressed").unwrap(),
            vec![
                (StreamId(7), "/camera/front/image/compressed".into()),
                (StreamId(9), "/camera/rear/image/compressed".into()),
                (StreamId(11), "/camera/left/image/compressed".into()),
            ]
        );
        assert_eq!(
            standard_bindings(&catalog, "/camera/front/image/compressed").unwrap(),
            vec![
                (StreamId(7), StreamBinding::Camera(CameraId(0))),
                (StreamId(9), StreamBinding::Camera(CameraId(1))),
                (StreamId(11), StreamBinding::Camera(CameraId(2))),
                (StreamId(20), StreamBinding::Path),
                (StreamId(21), StreamBinding::Odometry),
                (StreamId(22), StreamBinding::LaserScan),
                (StreamId(23), StreamBinding::Transforms { is_static: false },),
                (StreamId(24), StreamBinding::Transforms { is_static: true },),
            ]
        );
    }

    #[test]
    fn camera_pipeline_retains_jpeg_in_raw_message_backing_allocation() {
        let stream_id = StreamId(7);
        let payload = Bytes::from(
            encode_compressed_image_cdr(&CompressedImage {
                measurement_time: MeasurementTime(42),
                frame_id: "camera".into(),
                format: "jpeg".into(),
                jpeg: vec![1, 2, 3, 4],
            })
            .unwrap(),
        );
        let payload_start = payload.as_ptr() as usize;
        let payload_end = payload_start + payload.len();
        let mut pipelines = PipelineSet::new(
            &[StreamDescriptor {
                id: stream_id,
                topic: "/camera".into(),
                schema: "sensor_msgs/msg/CompressedImage".into(),
                message_encoding: "cdr".into(),
            }],
            &[(stream_id, StreamBinding::Camera(CameraId(0)))],
        );
        let mut updates = Vec::new();

        pipelines.decode(
            RawMessage {
                stream_id,
                arrival_time: ArrivalTime(100),
                payload,
            },
            &mut updates,
        );

        let DomainUpdate::Camera(frame) = updates.pop().unwrap() else {
            panic!("camera pipeline must produce a camera update");
        };
        let jpeg_start = frame.jpeg.as_ptr() as usize;
        assert_eq!(frame.jpeg.as_ref(), [1, 2, 3, 4]);
        assert!(jpeg_start >= payload_start);
        assert!(jpeg_start + frame.jpeg.len() <= payload_end);
    }
}
