use crate::{
    BevPathFrame, BevState, CameraFrame, CameraId, CameraState, PointCloudFrame, PointCloudState,
    RawMessage, RestoreSemantics, SceneFrameBuilder, SceneSnapshot, SessionPlan, StreamId,
    TelemetryFrame, TelemetryState, TransformBatch, TransformState, decode_compressed_image_bytes,
    decode_laser_scan, decode_odometry, decode_path, decode_tf_message,
};
use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

const FOCUSED_CAMERA_INTERVAL: Duration = Duration::from_millis(100);
const BACKGROUND_CAMERA_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcessingCounters {
    pub decoded: u64,
    pub errors: u64,
    pub unknown_streams: u64,
    /// High-bandwidth inputs coalesced before an expensive decode.
    pub dropped: u64,
}

impl ProcessingCounters {
    pub fn merge(&mut self, other: Self) {
        self.decoded = self.decoded.saturating_add(other.decoded);
        self.errors = self.errors.saturating_add(other.errors);
        self.unknown_streams = self.unknown_streams.saturating_add(other.unknown_streams);
        self.dropped = self.dropped.saturating_add(other.dropped);
    }
}

/// Shared Camera capability used by all Camera panels for one session.
///
/// Admission/coalescing happens on serialized messages. JPEG decoding remains a presentation
/// concern and the retained JPEG is a `Bytes` slice of the admitted CDR payload.
#[derive(Clone)]
pub struct CameraController {
    stream_to_camera: HashMap<StreamId, CameraId>,
    topics: Vec<(CameraId, String)>,
    state: CameraState,
    focused_camera: Option<CameraId>,
    presentation_elapsed: Duration,
    next_presentation: BTreeMap<CameraId, Duration>,
    pending: BTreeMap<CameraId, RawMessage>,
    counters: ProcessingCounters,
    input_frames: u64,
    presented_frames: u64,
    presented_by_id: BTreeMap<CameraId, u64>,
}

impl CameraController {
    pub const fn restore_semantics() -> RestoreSemantics {
        RestoreSemantics::RecentSample
    }

    pub fn new(plan: &SessionPlan) -> Self {
        let stream_to_camera = plan
            .camera_routes()
            .iter()
            .map(|route| (route.stream.id, route.camera_id))
            .collect();
        let topics = plan.camera_topics();
        let mut controller = Self {
            stream_to_camera,
            topics,
            state: CameraState::default(),
            focused_camera: plan.primary_camera(),
            presentation_elapsed: Duration::ZERO,
            next_presentation: BTreeMap::new(),
            pending: BTreeMap::new(),
            counters: ProcessingCounters::default(),
            input_frames: 0,
            presented_frames: 0,
            presented_by_id: BTreeMap::new(),
        };
        controller.reset_schedule();
        controller
    }

    /// Routes and coalesces a serialized Camera message without decoding it.
    pub fn admit(&mut self, message: &RawMessage) -> bool {
        let Some(camera_id) = self.stream_to_camera.get(&message.stream_id).copied() else {
            return false;
        };
        self.input_frames = self.input_frames.saturating_add(1);
        if self.pending.insert(camera_id, message.clone()).is_some() {
            self.counters.dropped = self.counters.dropped.saturating_add(1);
        }
        true
    }

    pub fn advance(&mut self, elapsed: Duration) {
        self.presentation_elapsed = self.presentation_elapsed.saturating_add(elapsed);
        let due = self
            .pending
            .keys()
            .copied()
            .filter(|camera_id| self.is_due(*camera_id))
            .collect::<Vec<_>>();
        for camera_id in due {
            let message = self
                .pending
                .remove(&camera_id)
                .expect("due Camera came from the pending map");
            match decode_compressed_image_bytes(message.payload) {
                Ok(image) => {
                    self.state.apply(CameraFrame {
                        camera_id,
                        measurement_time: image.measurement_time,
                        arrival_time: message.arrival_time,
                        frame_id: image.frame_id,
                        jpeg: image.jpeg,
                    });
                    self.counters.decoded = self.counters.decoded.saturating_add(1);
                }
                Err(_) => self.counters.errors = self.counters.errors.saturating_add(1),
            }
            self.next_presentation.insert(
                camera_id,
                self.presentation_elapsed
                    .saturating_add(self.interval(camera_id)),
            );
            self.presented_frames = self.presented_frames.saturating_add(1);
            let count = self.presented_by_id.entry(camera_id).or_default();
            *count = count.saturating_add(1);
        }
    }

    pub fn reset_for_restore(&mut self) {
        self.state.cold_seek();
        self.presentation_elapsed = Duration::ZERO;
        self.pending.clear();
        self.reset_schedule();
    }

    pub fn set_focused_camera(&mut self, focused_camera: Option<CameraId>) {
        let focused_camera = focused_camera
            .filter(|camera_id| self.topics.iter().any(|(id, _)| id == camera_id))
            .or_else(|| self.topics.first().map(|(camera_id, _)| *camera_id));
        if self.focused_camera == focused_camera {
            return;
        }
        if let Some(previous) = self.focused_camera {
            self.next_presentation.insert(
                previous,
                self.presentation_elapsed
                    .saturating_add(BACKGROUND_CAMERA_INTERVAL),
            );
        }
        self.focused_camera = focused_camera;
        if let Some(camera_id) = focused_camera {
            self.next_presentation
                .insert(camera_id, self.presentation_elapsed);
        }
    }

    pub fn state(&self) -> &CameraState {
        &self.state
    }

    pub fn topics(&self) -> &[(CameraId, String)] {
        &self.topics
    }

    pub fn focused_camera(&self) -> Option<CameraId> {
        self.focused_camera
    }

    pub fn counters(&self) -> ProcessingCounters {
        self.counters
    }

    pub fn input_frames(&self) -> u64 {
        self.input_frames
    }

    pub fn presented_frames(&self) -> u64 {
        self.presented_frames
    }

    pub fn presented_by_id(&self) -> &BTreeMap<CameraId, u64> {
        &self.presented_by_id
    }

    pub fn focused_hz() -> f64 {
        1.0 / FOCUSED_CAMERA_INTERVAL.as_secs_f64()
    }

    pub fn background_hz() -> f64 {
        1.0 / BACKGROUND_CAMERA_INTERVAL.as_secs_f64()
    }

    fn is_due(&self, camera_id: CameraId) -> bool {
        self.next_presentation
            .get(&camera_id)
            .is_none_or(|deadline| self.presentation_elapsed >= *deadline)
    }

    fn interval(&self, camera_id: CameraId) -> Duration {
        if Some(camera_id) == self.focused_camera {
            FOCUSED_CAMERA_INTERVAL
        } else {
            BACKGROUND_CAMERA_INTERVAL
        }
    }

    fn reset_schedule(&mut self) {
        self.next_presentation.clear();
        let background = self
            .topics
            .iter()
            .map(|(camera_id, _)| *camera_id)
            .filter(|camera_id| Some(*camera_id) != self.focused_camera)
            .collect::<Vec<_>>();
        let background_count = background.len().max(1) as f64;
        for (index, camera_id) in background.into_iter().enumerate() {
            let phase = Duration::from_secs_f64(
                BACKGROUND_CAMERA_INTERVAL.as_secs_f64() * index as f64 / background_count,
            );
            self.next_presentation.insert(camera_id, phase);
        }
        if let Some(camera_id) = self.focused_camera {
            self.next_presentation.insert(camera_id, Duration::ZERO);
        }
    }
}

#[derive(Clone)]
pub struct PathController {
    stream: Option<StreamId>,
    state: BevState,
    counters: ProcessingCounters,
}

impl PathController {
    pub const fn restore_semantics() -> RestoreSemantics {
        RestoreSemantics::RecentSample
    }

    pub fn new(plan: &SessionPlan) -> Self {
        Self {
            stream: plan.path_stream().map(|stream| stream.id),
            state: BevState::default(),
            counters: ProcessingCounters::default(),
        }
    }

    pub fn process(&mut self, message: &RawMessage) -> bool {
        if Some(message.stream_id) != self.stream {
            return false;
        }
        match decode_path(&message.payload) {
            Ok(path) => {
                self.state.apply(BevPathFrame {
                    measurement_time: path.measurement_time,
                    arrival_time: message.arrival_time,
                    frame_id: path.frame_id,
                    points: path
                        .points
                        .into_iter()
                        .map(|[forward, left]| [-left as f32, forward as f32])
                        .collect(),
                });
                self.counters.decoded = self.counters.decoded.saturating_add(1);
            }
            Err(_) => self.counters.errors = self.counters.errors.saturating_add(1),
        }
        true
    }

    pub fn reset_for_restore(&mut self) {
        self.state.cold_seek();
    }

    pub fn state(&self) -> &BevState {
        &self.state
    }

    pub fn counters(&self) -> ProcessingCounters {
        self.counters
    }
}

#[derive(Clone)]
pub struct OdometryController {
    stream: Option<StreamId>,
    state: TelemetryState,
    counters: ProcessingCounters,
}

impl OdometryController {
    pub const fn restore_semantics() -> RestoreSemantics {
        RestoreSemantics::RecentSample
    }

    pub fn new(plan: &SessionPlan) -> Self {
        Self {
            stream: plan.odometry_stream().map(|stream| stream.id),
            state: TelemetryState::default(),
            counters: ProcessingCounters::default(),
        }
    }

    pub fn process(&mut self, message: &RawMessage) -> bool {
        if Some(message.stream_id) != self.stream {
            return false;
        }
        match decode_odometry(&message.payload) {
            Ok(odometry) => {
                let [qx, qy, qz, qw] = odometry.orientation;
                let sin_yaw = 2.0 * (qw * qz + qx * qy);
                let cos_yaw = 1.0 - 2.0 * (qy * qy + qz * qz);
                let [vx, vy, _] = odometry.linear_velocity;
                self.state.apply(TelemetryFrame {
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
                });
                self.counters.decoded = self.counters.decoded.saturating_add(1);
            }
            Err(_) => self.counters.errors = self.counters.errors.saturating_add(1),
        }
        true
    }

    pub fn reset_for_restore(&mut self) {
        self.state.cold_seek();
    }

    pub fn state(&self) -> &TelemetryState {
        &self.state
    }

    pub fn counters(&self) -> ProcessingCounters {
        self.counters
    }
}

#[derive(Clone)]
pub struct TransformController {
    dynamic_stream: Option<StreamId>,
    static_stream: Option<StreamId>,
    state: TransformState,
    persistent_messages: Vec<RawMessage>,
    counters: ProcessingCounters,
}

impl TransformController {
    pub const fn dynamic_restore_semantics() -> RestoreSemantics {
        RestoreSemantics::History(Duration::from_secs(1))
    }

    pub const fn static_restore_semantics() -> RestoreSemantics {
        RestoreSemantics::Persistent
    }

    pub fn new(plan: &SessionPlan) -> Self {
        Self {
            dynamic_stream: plan.dynamic_tf_stream().map(|stream| stream.id),
            static_stream: plan.static_tf_stream().map(|stream| stream.id),
            state: TransformState::default(),
            persistent_messages: Vec::new(),
            counters: ProcessingCounters::default(),
        }
    }

    pub fn process(&mut self, message: &RawMessage) -> bool {
        let is_static = if Some(message.stream_id) == self.static_stream {
            if !self.persistent_messages.contains(message) {
                self.persistent_messages.push(message.clone());
                self.persistent_messages
                    .sort_by_key(|message| message.arrival_time);
            }
            true
        } else if Some(message.stream_id) == self.dynamic_stream {
            false
        } else {
            return false;
        };
        match decode_tf_message(&message.payload) {
            Ok(transforms) => {
                self.state.apply(TransformBatch {
                    arrival_time: message.arrival_time,
                    is_static,
                    transforms,
                });
                self.counters.decoded = self.counters.decoded.saturating_add(1);
            }
            Err(_) => self.counters.errors = self.counters.errors.saturating_add(1),
        }
        true
    }

    pub fn reset_for_restore(&mut self, target: crate::ArrivalTime) {
        self.state.clear();
        let persistent = self
            .persistent_messages
            .iter()
            .filter(|message| message.arrival_time <= target)
            .cloned()
            .collect::<Vec<_>>();
        for message in &persistent {
            self.apply_without_archiving(message, true);
        }
    }

    pub fn state(&self) -> &TransformState {
        &self.state
    }

    pub fn counters(&self) -> ProcessingCounters {
        self.counters
    }

    pub fn persistent_message_count(&self) -> usize {
        self.persistent_messages.len()
    }

    fn apply_without_archiving(&mut self, message: &RawMessage, is_static: bool) {
        match decode_tf_message(&message.payload) {
            Ok(transforms) => self.state.apply(TransformBatch {
                arrival_time: message.arrival_time,
                is_static,
                transforms,
            }),
            Err(_) => self.counters.errors = self.counters.errors.saturating_add(1),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SceneController {
    stream: Option<StreamId>,
    point_cloud: PointCloudState,
    builder: SceneFrameBuilder,
    counters: ProcessingCounters,
}

impl SceneController {
    pub const fn restore_semantics() -> RestoreSemantics {
        RestoreSemantics::RecentSample
    }

    pub fn new(plan: &SessionPlan) -> Self {
        Self {
            stream: plan.point_cloud_stream().map(|stream| stream.id),
            point_cloud: PointCloudState::default(),
            builder: SceneFrameBuilder::new(),
            counters: ProcessingCounters::default(),
        }
    }

    pub fn process(&mut self, message: &RawMessage) -> bool {
        if Some(message.stream_id) != self.stream {
            return false;
        }
        match decode_laser_scan(&message.payload) {
            Ok(scan) => {
                let mut points = Vec::with_capacity(scan.ranges.len());
                for (index, range) in scan.ranges.iter().copied().enumerate() {
                    if !range.is_finite() || range < scan.range_min || range > scan.range_max {
                        continue;
                    }
                    let angle = scan.angle_min + index as f32 * scan.angle_increment;
                    points.push([range * angle.cos(), range * angle.sin(), 0.0]);
                }
                self.point_cloud.apply(PointCloudFrame {
                    measurement_time: scan.measurement_time,
                    arrival_time: message.arrival_time,
                    frame_id: scan.frame_id,
                    points,
                });
                self.counters.decoded = self.counters.decoded.saturating_add(1);
            }
            Err(_) => self.counters.errors = self.counters.errors.saturating_add(1),
        }
        true
    }

    pub fn reset_for_restore(&mut self) {
        self.point_cloud.cold_seek();
        self.builder.reset();
    }

    pub fn snapshot<'a>(
        &'a mut self,
        path: &'a BevState,
        odometry: &'a TelemetryState,
        transforms: &'a TransformState,
        accumulate: bool,
    ) -> SceneSnapshot<'a> {
        self.builder
            .build(path, odometry, &self.point_cloud, transforms, accumulate)
    }

    pub fn point_cloud(&self) -> &PointCloudState {
        &self.point_cloud
    }

    pub fn counters(&self) -> ProcessingCounters {
        self.counters
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ArrivalTime, CompressedImage, MeasurementTime, PlaybackRequirements, SourceCatalog,
        StreamDescriptor, StreamTimingSummary, TransformStamped, encode_tf_message_cdr,
    };
    use bytes::Bytes;

    fn camera_plan() -> SessionPlan {
        let catalog = SourceCatalog {
            time_range: None,
            streams: vec![StreamDescriptor {
                id: StreamId(7),
                topic: "/camera".into(),
                schema: "sensor_msgs/msg/CompressedImage".into(),
                message_encoding: "cdr".into(),
                timing: StreamTimingSummary::default(),
            }],
        };
        SessionPlan::build(&catalog, "/camera", &PlaybackRequirements::default()).unwrap()
    }

    #[test]
    fn camera_admission_coalesces_before_decode_and_keeps_bytes_slice() {
        let payload = Bytes::from(
            crate::encode_compressed_image_cdr(&CompressedImage {
                measurement_time: MeasurementTime(1),
                frame_id: "camera".into(),
                format: "jpeg".into(),
                jpeg: vec![1, 2, 3, 4],
            })
            .unwrap(),
        );
        let mut controller = CameraController::new(&camera_plan());
        for time in [1, 2] {
            assert!(controller.admit(&RawMessage {
                stream_id: StreamId(7),
                arrival_time: ArrivalTime(time),
                payload: payload.clone(),
            }));
        }
        controller.advance(Duration::ZERO);
        let frame = controller.state().latest_for(CameraId(0)).unwrap();
        assert_eq!(frame.arrival_time, ArrivalTime(2));
        assert_eq!(controller.counters().dropped, 1);
        let payload_start = payload.as_ptr() as usize;
        let jpeg_start = frame.jpeg.as_ptr() as usize;
        assert!(jpeg_start >= payload_start);
        assert!(jpeg_start + frame.jpeg.len() <= payload_start + payload.len());
    }

    #[test]
    fn camera_rates_preserve_the_existing_policy() {
        assert_eq!(CameraController::focused_hz(), 10.0);
        assert_eq!(CameraController::background_hz(), 5.0);
    }

    #[test]
    fn repeated_static_transforms_restore_only_updates_valid_at_target() {
        let catalog = SourceCatalog {
            time_range: None,
            streams: vec![StreamDescriptor {
                id: StreamId(9),
                topic: crate::TF_STATIC_TOPIC.into(),
                schema: "tf2_msgs/msg/TFMessage".into(),
                message_encoding: "cdr".into(),
                timing: StreamTimingSummary::default(),
            }],
        };
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_transforms();
        let plan = SessionPlan::build(&catalog, "/unused", &requirements).unwrap();
        let mut controller = TransformController::new(&plan);
        for (arrival, x) in [(10, 1.0), (20, 2.0)] {
            let message = RawMessage {
                stream_id: StreamId(9),
                arrival_time: ArrivalTime(arrival),
                payload: encode_tf_message_cdr(&[TransformStamped {
                    measurement_time: MeasurementTime(arrival),
                    frame_id: "map".into(),
                    child_frame_id: "base".into(),
                    translation: [x, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                }])
                .unwrap()
                .into(),
            };
            assert!(controller.process(&message));
        }
        assert_eq!(controller.persistent_message_count(), 2);

        controller.reset_for_restore(ArrivalTime(15));
        assert_eq!(
            controller
                .state()
                .transform_points("base", "map", &[[0.0, 0.0, 0.0]])
                .unwrap(),
            vec![[1.0, 0.0, 0.0]]
        );
        controller.reset_for_restore(ArrivalTime(25));
        assert_eq!(
            controller
                .state()
                .transform_points("base", "map", &[[0.0, 0.0, 0.0]])
                .unwrap(),
            vec![[2.0, 0.0, 0.0]]
        );
    }
}
