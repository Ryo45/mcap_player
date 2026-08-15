use crate::{
    BevPathFrame, CameraFrame, CameraId, DecodeError, DomainRoute, DomainTarget, PointCloudFrame,
    RawMessage, SessionPlan, StreamId, TelemetryFrame, TransformBatch,
    decode_compressed_image_bytes, decode_laser_scan, decode_odometry, decode_path,
    decode_tf_message,
};
use std::{collections::HashMap, fmt};

#[derive(Clone, Debug, PartialEq)]
pub enum DomainUpdate {
    Camera(CameraFrame),
    Path(BevPathFrame),
    Telemetry(TelemetryFrame),
    PointCloud(PointCloudFrame),
    Transforms(TransformBatch),
}

struct PathPipeline;

impl DomainPipeline for PathPipeline {
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

impl DomainPipeline for OdometryPipeline {
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

impl DomainPipeline for LaserScanPipeline {
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

impl DomainPipeline for TransformPipeline {
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

pub trait DomainPipeline {
    fn decode(
        &mut self,
        message: RawMessage,
        output: &mut Vec<DomainUpdate>,
    ) -> Result<(), DecodeError>;
}

struct CompressedImagePipeline {
    camera_id: CameraId,
}

impl DomainPipeline for CompressedImagePipeline {
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

pub struct DomainPipelineSet {
    pipelines: HashMap<StreamId, Box<dyn DomainPipeline>>,
    counters: PipelineCounters,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainPipelineError(String);

impl fmt::Display for DomainPipelineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DomainPipelineError {}

impl DomainPipelineSet {
    pub fn new(plan: &SessionPlan) -> Self {
        Self::from_routes(plan.domain_routes())
            .expect("SessionPlan routes must satisfy DomainTarget requirements")
    }

    pub fn from_routes(routes: &[DomainRoute]) -> Result<Self, DomainPipelineError> {
        let mut pipelines: HashMap<StreamId, Box<dyn DomainPipeline>> = HashMap::new();
        for route in routes {
            let pipeline = build_pipeline(route)?;
            pipelines.insert(route.stream.id, pipeline);
        }
        Ok(Self {
            pipelines,
            counters: PipelineCounters::default(),
        })
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

fn build_pipeline(route: &DomainRoute) -> Result<Box<dyn DomainPipeline>, DomainPipelineError> {
    if !route.target.accepts_stream(&route.stream) {
        return Err(DomainPipelineError(format!(
            "stream {} ({}) cannot target {:?}: expected cdr {}",
            route.stream.topic,
            route.stream.schema,
            route.target,
            route.target.expected_schema(),
        )));
    }
    let pipeline: Box<dyn DomainPipeline> = match route.target {
        DomainTarget::Camera(camera_id) => Box::new(CompressedImagePipeline { camera_id }),
        DomainTarget::Path => Box::new(PathPipeline),
        DomainTarget::Telemetry => Box::new(OdometryPipeline),
        DomainTarget::PointCloud => Box::new(LaserScanPipeline),
        DomainTarget::Transforms { is_static } => Box::new(TransformPipeline { is_static }),
    };
    Ok(pipeline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ArrivalTime, CameraId, CompressedImage, MeasurementTime, StreamDescriptor,
        encode_compressed_image_cdr,
    };
    use bytes::Bytes;

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
        let mut pipelines = DomainPipelineSet::from_routes(&[DomainRoute {
            stream: StreamDescriptor {
                id: stream_id,
                topic: "/camera".into(),
                schema: "sensor_msgs/msg/CompressedImage".into(),
                message_encoding: "cdr".into(),
                timing: crate::StreamTimingSummary::default(),
            },
            target: DomainTarget::Camera(CameraId(0)),
        }])
        .unwrap();
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

    #[test]
    fn invalid_domain_route_is_rejected_instead_of_silently_skipped() {
        let error = DomainPipelineSet::from_routes(&[DomainRoute {
            stream: StreamDescriptor {
                id: StreamId(1),
                topic: crate::ODOM_TOPIC.into(),
                schema: "example_msgs/msg/Foo".into(),
                message_encoding: "cdr".into(),
                timing: crate::StreamTimingSummary::default(),
            },
            target: DomainTarget::Telemetry,
        }])
        .err()
        .expect("wrong-schema route must fail");

        assert!(error.to_string().contains("nav_msgs/msg/Odometry"));
    }
}
