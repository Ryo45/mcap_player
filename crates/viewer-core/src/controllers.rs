use crate::{
    BevPathFrame, BevState, CameraFrame, CameraId, CameraState, DYNAMIC_TF_HISTORY,
    PointCloudFrame, PointCloudState, RawMessage, RestoreSemantics, SessionPlan, StreamId,
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
        RestoreSemantics::LatestBefore
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
            let _ = self.decode_and_apply(camera_id, message);
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

    /// Applies one exact seek predecessor immediately, bypassing playback rate scheduling.
    ///
    /// Unlike forward admission, restore is strict: a routed malformed predecessor is an error
    /// so a staging [`crate::FeatureRuntime`] can discard the whole candidate state.
    pub fn restore(&mut self, message: &RawMessage) -> Result<bool, crate::DecodeError> {
        let Some(camera_id) = self.stream_to_camera.get(&message.stream_id).copied() else {
            return Ok(false);
        };
        self.input_frames = self.input_frames.saturating_add(1);
        self.decode_and_apply(camera_id, message.clone())?;
        self.presented_frames = self.presented_frames.saturating_add(1);
        let count = self.presented_by_id.entry(camera_id).or_default();
        *count = count.saturating_add(1);
        Ok(true)
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

    fn decode_and_apply(
        &mut self,
        camera_id: CameraId,
        message: RawMessage,
    ) -> Result<(), crate::DecodeError> {
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
                Ok(())
            }
            Err(error) => {
                self.counters.errors = self.counters.errors.saturating_add(1);
                Err(error)
            }
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
        RestoreSemantics::LatestBefore
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
        let _ = self.decode_and_apply(message);
        true
    }

    pub fn restore(&mut self, message: &RawMessage) -> Result<bool, crate::DecodeError> {
        if Some(message.stream_id) != self.stream {
            return Ok(false);
        }
        self.decode_and_apply(message)?;
        Ok(true)
    }

    fn decode_and_apply(&mut self, message: &RawMessage) -> Result<(), crate::DecodeError> {
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
                Ok(())
            }
            Err(error) => {
                self.counters.errors = self.counters.errors.saturating_add(1);
                Err(error)
            }
        }
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
        RestoreSemantics::LatestBefore
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
        let _ = self.decode_and_apply(message);
        true
    }

    pub fn restore(&mut self, message: &RawMessage) -> Result<bool, crate::DecodeError> {
        if Some(message.stream_id) != self.stream {
            return Ok(false);
        }
        self.decode_and_apply(message)?;
        Ok(true)
    }

    fn decode_and_apply(&mut self, message: &RawMessage) -> Result<(), crate::DecodeError> {
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
                Ok(())
            }
            Err(error) => {
                self.counters.errors = self.counters.errors.saturating_add(1);
                Err(error)
            }
        }
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
    counters: ProcessingCounters,
}

impl TransformController {
    pub const fn dynamic_restore_semantics() -> RestoreSemantics {
        RestoreSemantics::History(DYNAMIC_TF_HISTORY)
    }

    pub const fn static_restore_semantics() -> RestoreSemantics {
        RestoreSemantics::Persistent
    }

    pub fn new(plan: &SessionPlan) -> Self {
        Self {
            dynamic_stream: plan.dynamic_tf_stream().map(|stream| stream.id),
            static_stream: plan.static_tf_stream().map(|stream| stream.id),
            state: TransformState::default(),
            counters: ProcessingCounters::default(),
        }
    }

    pub fn process(&mut self, message: &RawMessage) -> bool {
        let is_static = if Some(message.stream_id) == self.static_stream {
            true
        } else if Some(message.stream_id) == self.dynamic_stream {
            false
        } else {
            return false;
        };
        let _ = self.decode_and_apply(message, is_static);
        true
    }

    pub fn restore(&mut self, message: &RawMessage) -> Result<bool, crate::DecodeError> {
        let is_static = if Some(message.stream_id) == self.static_stream {
            true
        } else if Some(message.stream_id) == self.dynamic_stream {
            false
        } else {
            return Ok(false);
        };
        self.decode_and_apply(message, is_static)?;
        Ok(true)
    }

    fn decode_and_apply(
        &mut self,
        message: &RawMessage,
        is_static: bool,
    ) -> Result<(), crate::DecodeError> {
        match decode_tf_message(&message.payload) {
            Ok(transforms) => {
                self.state.apply(TransformBatch {
                    arrival_time: message.arrival_time,
                    is_static,
                    transforms,
                });
                self.counters.decoded = self.counters.decoded.saturating_add(1);
                Ok(())
            }
            Err(error) => {
                self.counters.errors = self.counters.errors.saturating_add(1);
                Err(error)
            }
        }
    }

    pub fn reset_for_restore(&mut self, _target: crate::ArrivalTime) {
        self.state.clear();
    }

    pub fn state(&self) -> &TransformState {
        &self.state
    }

    pub fn counters(&self) -> ProcessingCounters {
        self.counters
    }
}

#[derive(Clone, Debug)]
pub struct SceneController {
    stream: Option<StreamId>,
    point_cloud: PointCloudState,
    counters: ProcessingCounters,
}

impl SceneController {
    pub const fn restore_semantics() -> RestoreSemantics {
        RestoreSemantics::LatestBefore
    }

    pub fn new(plan: &SessionPlan) -> Self {
        Self {
            stream: plan.point_cloud_stream().map(|stream| stream.id),
            point_cloud: PointCloudState::default(),
            counters: ProcessingCounters::default(),
        }
    }

    pub fn process(&mut self, message: &RawMessage) -> bool {
        if Some(message.stream_id) != self.stream {
            return false;
        }
        let _ = self.decode_and_apply(message);
        true
    }

    pub fn restore(&mut self, message: &RawMessage) -> Result<bool, crate::DecodeError> {
        if Some(message.stream_id) != self.stream {
            return Ok(false);
        }
        self.decode_and_apply(message)?;
        Ok(true)
    }

    fn decode_and_apply(&mut self, message: &RawMessage) -> Result<(), crate::DecodeError> {
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
                Ok(())
            }
            Err(error) => {
                self.counters.errors = self.counters.errors.saturating_add(1);
                Err(error)
            }
        }
    }

    pub fn reset_for_restore(&mut self) {
        self.point_cloud.cold_seek();
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
        StreamDescriptor, StreamTimingSummary, TransformStamped, WorkspaceBindings,
        encode_tf_message_cdr,
    };
    use bytes::Bytes;

    fn bindings() -> WorkspaceBindings {
        WorkspaceBindings {
            path_topic: "/planning/path".into(),
            odometry_topic: "/odom".into(),
            point_cloud_topic: "/scan".into(),
            dynamic_tf_topic: "/tf".into(),
            static_tf_topic: "/tf_static".into(),
        }
    }

    fn camera_plan() -> SessionPlan {
        let catalog = SourceCatalog {
            time_range: None,
            capabilities: Default::default(),
            streams: vec![StreamDescriptor {
                id: StreamId(7),
                topic: "/camera".into(),
                schema: "sensor_msgs/msg/CompressedImage".into(),
                message_encoding: "cdr".into(),
                timing: StreamTimingSummary::default(),
            }],
        };
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_all_cameras();
        SessionPlan::build(&catalog, "/camera", &requirements, &bindings()).unwrap()
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
    fn camera_coalescing_discards_malformed_old_cdr_before_decode() {
        let valid = Bytes::from(
            crate::encode_compressed_image_cdr(&CompressedImage {
                measurement_time: MeasurementTime(2),
                frame_id: "camera".into(),
                format: "jpeg".into(),
                jpeg: vec![1, 2, 3],
            })
            .unwrap(),
        );
        let mut controller = CameraController::new(&camera_plan());
        assert!(controller.admit(&RawMessage {
            stream_id: StreamId(7),
            arrival_time: ArrivalTime(1),
            payload: Bytes::from_static(&[0xff]),
        }));
        assert!(controller.admit(&RawMessage {
            stream_id: StreamId(7),
            arrival_time: ArrivalTime(2),
            payload: valid,
        }));

        controller.advance(Duration::ZERO);

        assert_eq!(controller.counters().dropped, 1);
        assert_eq!(controller.counters().decoded, 1);
        assert_eq!(controller.counters().errors, 0);
        assert_eq!(
            controller
                .state()
                .latest_for(CameraId(0))
                .unwrap()
                .arrival_time,
            ArrivalTime(2)
        );
    }

    #[test]
    fn camera_rates_preserve_the_existing_policy() {
        assert_eq!(CameraController::focused_hz(), 10.0);
        assert_eq!(CameraController::background_hz(), 5.0);
    }

    #[test]
    fn camera_restore_bypasses_playback_scheduler_for_every_selected_camera() {
        let catalog = SourceCatalog {
            time_range: None,
            capabilities: Default::default(),
            streams: (0..3)
                .map(|index| StreamDescriptor {
                    id: StreamId(7 + index),
                    topic: format!("/camera/{index}"),
                    schema: "sensor_msgs/msg/CompressedImage".into(),
                    message_encoding: "cdr".into(),
                    timing: StreamTimingSummary::default(),
                })
                .collect(),
        };
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_all_cameras();
        let plan = SessionPlan::build(&catalog, "/camera/0", &requirements, &bindings()).unwrap();
        let mut controller = CameraController::new(&plan);
        let payload = |time| {
            Bytes::from(
                crate::encode_compressed_image_cdr(&CompressedImage {
                    measurement_time: MeasurementTime(time),
                    frame_id: "camera".into(),
                    format: "jpeg".into(),
                    jpeg: vec![time as u8],
                })
                .unwrap(),
            )
        };
        for index in 0..3 {
            assert!(
                controller
                    .restore(&RawMessage {
                        stream_id: StreamId(7 + index),
                        arrival_time: ArrivalTime(i64::from(index)),
                        payload: payload(i64::from(index)),
                    })
                    .unwrap()
            );
        }

        assert_eq!(controller.state().frames().count(), 3);
        assert_eq!(controller.counters().decoded, 3);
        assert_eq!(controller.counters().dropped, 0);
    }

    #[test]
    fn repeated_static_transforms_restore_only_updates_valid_at_target() {
        let catalog = SourceCatalog {
            time_range: None,
            capabilities: Default::default(),
            streams: vec![StreamDescriptor {
                id: StreamId(9),
                topic: "/tf_static".into(),
                schema: "tf2_msgs/msg/TFMessage".into(),
                message_encoding: "cdr".into(),
                timing: StreamTimingSummary::default(),
            }],
        };
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_transforms();
        let plan = SessionPlan::build(&catalog, "/unused", &requirements, &bindings()).unwrap();
        let mut controller = TransformController::new(&plan);
        let mut archive = Vec::new();
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
            archive.push(message);
        }

        controller.reset_for_restore(ArrivalTime(15));
        for message in archive
            .iter()
            .filter(|message| message.arrival_time <= ArrivalTime(15))
        {
            controller.process(message);
        }
        assert_eq!(
            controller
                .state()
                .transform_points("base", "map", &[[0.0, 0.0, 0.0]])
                .unwrap(),
            vec![[1.0, 0.0, 0.0]]
        );
        controller.reset_for_restore(ArrivalTime(25));
        for message in archive
            .iter()
            .filter(|message| message.arrival_time <= ArrivalTime(25))
        {
            controller.process(message);
        }
        assert_eq!(
            controller
                .state()
                .transform_points("base", "map", &[[0.0, 0.0, 0.0]])
                .unwrap(),
            vec![[2.0, 0.0, 0.0]]
        );
    }
}
