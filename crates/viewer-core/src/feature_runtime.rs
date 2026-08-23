use crate::{
    ArrivalTime, CameraController, CameraId, OdometryController, PathController,
    PlaybackPerformance, ProcessingCounters, RawMessage, SceneController, SessionPlan, StageTiming,
    StreamId, TransformController,
};
use std::{error::Error, fmt, time::Duration};
use web_time::Instant;

/// Concrete owner and transactional reducer for continuous feature state.
///
/// Plot, Preview and Exact Range Query deliberately remain outside this type. Platform-specific
/// source I/O, clocks and buffering also remain with the application adapters.
#[derive(Clone)]
pub struct FeatureRuntime {
    cameras: CameraController,
    path: PathController,
    odometry: OdometryController,
    transforms: TransformController,
    scene: Option<SceneController>,
    unknown_streams: u64,
    processing_time: StageTiming,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FeatureRestoreErrorKind {
    Decode {
        feature: &'static str,
        reason: String,
    },
    Unrouted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeatureRestoreError {
    pub stream_id: StreamId,
    pub arrival_time: ArrivalTime,
    pub kind: FeatureRestoreErrorKind,
}

impl fmt::Display for FeatureRestoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            FeatureRestoreErrorKind::Decode { feature, reason } => write!(
                formatter,
                "restore decode failed for {feature} stream {} at {} ns: {reason}",
                self.stream_id.0, self.arrival_time.0
            ),
            FeatureRestoreErrorKind::Unrouted => write!(
                formatter,
                "restore message for stream {} at {} ns did not route to a continuous feature",
                self.stream_id.0, self.arrival_time.0
            ),
        }
    }
}

impl Error for FeatureRestoreError {}

impl FeatureRuntime {
    pub fn new(plan: &SessionPlan, enable_scene: bool) -> Self {
        Self {
            cameras: CameraController::new(plan),
            path: PathController::new(plan),
            odometry: OdometryController::new(plan),
            transforms: TransformController::new(plan),
            scene: enable_scene.then(|| SceneController::new(plan)),
            unknown_streams: 0,
            processing_time: StageTiming::default(),
        }
    }

    /// Reduces forward messages using the existing tolerant playback semantics.
    ///
    /// A routed malformed message increments that controller's error counter and leaves its prior
    /// state visible. Camera admission remains serialized and coalesces before decode.
    pub fn process_messages(&mut self, elapsed: Duration, messages: &[RawMessage]) {
        let started = Instant::now();
        for message in messages {
            let matched = self.cameras.admit(message)
                | self.path.process(message)
                | self.odometry.process(message)
                | self.transforms.process(message)
                | self
                    .scene
                    .as_mut()
                    .is_some_and(|scene| scene.process(message));
            if !matched {
                self.unknown_streams = self.unknown_streams.saturating_add(1);
            }
        }
        self.cameras.advance(elapsed);
        self.processing_time.record(started.elapsed());
    }

    /// Builds a complete seek candidate without mutating the authoritative runtime.
    pub fn stage_restore(
        &self,
        target: ArrivalTime,
        messages: &[RawMessage],
    ) -> Result<Self, FeatureRestoreError> {
        let started = Instant::now();
        let mut candidate = self.clone();
        candidate.cameras.reset_for_restore();
        candidate.path.reset_for_restore();
        candidate.odometry.reset_for_restore();
        candidate.transforms.reset_for_restore(target);
        if let Some(scene) = &mut candidate.scene {
            scene.reset_for_restore();
        }

        for message in messages {
            let mut matched = false;
            matched |= restore_one("Camera", message, || candidate.cameras.restore(message))?;
            matched |= restore_one("Path", message, || candidate.path.restore(message))?;
            matched |= restore_one("Odometry", message, || candidate.odometry.restore(message))?;
            matched |= restore_one("Transform", message, || {
                candidate.transforms.restore(message)
            })?;
            if let Some(scene) = &mut candidate.scene {
                matched |= restore_one("Scene", message, || scene.restore(message))?;
            }
            if !matched {
                return Err(FeatureRestoreError {
                    stream_id: message.stream_id,
                    arrival_time: message.arrival_time,
                    kind: FeatureRestoreErrorKind::Unrouted,
                });
            }
        }
        candidate.processing_time.record(started.elapsed());
        Ok(candidate)
    }

    pub fn commit_restore(&mut self, candidate: Self) {
        *self = candidate;
    }

    pub fn restore_transactional(
        &mut self,
        target: ArrivalTime,
        messages: &[RawMessage],
    ) -> Result<(), FeatureRestoreError> {
        let candidate = self.stage_restore(target, messages)?;
        self.commit_restore(candidate);
        Ok(())
    }

    pub fn cameras(&self) -> &CameraController {
        &self.cameras
    }

    pub fn path(&self) -> &PathController {
        &self.path
    }

    pub fn odometry(&self) -> &OdometryController {
        &self.odometry
    }

    pub fn transforms(&self) -> &TransformController {
        &self.transforms
    }

    pub fn scene(&self) -> Option<&SceneController> {
        self.scene.as_ref()
    }

    pub fn scene_mut(&mut self) -> Option<&mut SceneController> {
        self.scene.as_mut()
    }

    pub fn set_scheduling_priority(&mut self, camera: Option<CameraId>) {
        self.cameras.set_focused_camera(camera);
    }

    pub fn scheduling_priority(&self) -> Option<CameraId> {
        self.cameras.focused_camera()
    }

    pub fn counters(&self) -> ProcessingCounters {
        let mut counters = ProcessingCounters {
            unknown_streams: self.unknown_streams,
            ..ProcessingCounters::default()
        };
        counters.merge(self.cameras.counters());
        counters.merge(self.path.counters());
        counters.merge(self.odometry.counters());
        counters.merge(self.transforms.counters());
        if let Some(scene) = &self.scene {
            counters.merge(scene.counters());
        }
        counters
    }

    pub fn processing_time(&self) -> StageTiming {
        self.processing_time
    }

    pub fn playback_performance(&self, source_read: StageTiming) -> PlaybackPerformance {
        PlaybackPerformance::from_controllers(source_read, self.processing_time, &self.cameras)
    }
}

fn restore_one(
    feature: &'static str,
    message: &RawMessage,
    apply: impl FnOnce() -> Result<bool, crate::DecodeError>,
) -> Result<bool, FeatureRestoreError> {
    apply().map_err(|error| FeatureRestoreError {
        stream_id: message.stream_id,
        arrival_time: message.arrival_time,
        kind: FeatureRestoreErrorKind::Decode {
            feature,
            reason: error.to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BevPathFrame, CameraFrame, PlaybackRequirements, PointCloudFrame, SourceCatalog,
        TelemetryFrame, WorkspaceBindings,
    };
    use bytes::Bytes;

    #[derive(Clone, Debug, PartialEq)]
    struct VisibleSnapshot {
        cameras: Vec<(CameraId, CameraFrame)>,
        path: Option<BevPathFrame>,
        odometry: Option<TelemetryFrame>,
        static_transforms: usize,
        dynamic_transforms: usize,
        transform_revision: u64,
        scene: Option<PointCloudFrame>,
        priority: Option<CameraId>,
        counters: ProcessingCounters,
    }

    fn bindings() -> WorkspaceBindings {
        WorkspaceBindings {
            path_topic: "/planning/path".into(),
            odometry_topic: "/odom".into(),
            point_cloud_topic: "/scan".into(),
            dynamic_tf_topic: "/tf".into(),
            static_tf_topic: "/tf_static".into(),
        }
    }

    fn requirements() -> PlaybackRequirements {
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_all_cameras();
        requirements.require_path();
        requirements.require_odometry();
        requirements.require_point_cloud();
        requirements.require_transforms();
        requirements
    }

    fn fixture() -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap"),
        )
        .unwrap()
    }

    fn snapshot(runtime: &FeatureRuntime) -> VisibleSnapshot {
        VisibleSnapshot {
            cameras: runtime
                .cameras()
                .state()
                .frames()
                .map(|(camera, frame)| (*camera, frame.clone()))
                .collect(),
            path: runtime.path().state().latest().cloned(),
            odometry: runtime.odometry().state().latest().cloned(),
            static_transforms: runtime.transforms().state().static_len(),
            dynamic_transforms: runtime.transforms().state().dynamic_len(),
            transform_revision: runtime.transforms().state().revision(),
            scene: runtime
                .scene()
                .and_then(|scene| scene.point_cloud().latest())
                .cloned(),
            priority: runtime.scheduling_priority(),
            counters: runtime.counters(),
        }
    }

    fn populated_runtime() -> (SessionPlan, Vec<RawMessage>, FeatureRuntime) {
        let mut source = crate::McapSource::new(fixture()).unwrap();
        let catalog: SourceCatalog = source.catalog().clone();
        let plan = SessionPlan::build(
            &catalog,
            "/camera/front/image/compressed",
            &requirements(),
            &bindings(),
        )
        .unwrap();
        source.select_streams(plan.selected_stream_ids()).unwrap();
        let (_, end) = source.time_range();
        let messages = source.read_until(end).unwrap();
        let mut runtime = FeatureRuntime::new(&plan, true);
        runtime.process_messages(Duration::from_secs(10), &messages);
        (plan, messages, runtime)
    }

    fn malformed(stream_id: StreamId, arrival_time: ArrivalTime) -> RawMessage {
        RawMessage {
            stream_id,
            arrival_time,
            payload: Bytes::from_static(&[0xff]),
        }
    }

    #[test]
    fn malformed_restore_for_any_feature_preserves_every_visible_state() {
        let (plan, _, mut runtime) = populated_runtime();
        let before = snapshot(&runtime);
        let target = ArrivalTime(5_000_000_000);
        let streams = [
            plan.camera_routes()[0].stream.id,
            plan.path_stream().unwrap().id,
            plan.odometry_stream().unwrap().id,
            plan.dynamic_tf_stream().unwrap().id,
            plan.point_cloud_stream().unwrap().id,
        ];

        for stream in streams {
            let error = runtime
                .restore_transactional(target, &[malformed(stream, target)])
                .unwrap_err();
            assert_eq!(error.stream_id, stream);
            assert_eq!(
                snapshot(&runtime),
                before,
                "stream {} partially committed",
                stream.0
            );
        }
    }

    #[test]
    fn multi_feature_failure_discards_earlier_successful_candidate_updates() {
        let (plan, messages, mut runtime) = populated_runtime();
        let before = snapshot(&runtime);
        let camera_stream = plan.camera_routes()[0].stream.id;
        let valid_camera = messages
            .iter()
            .rev()
            .find(|message| message.stream_id == camera_stream)
            .unwrap()
            .clone();
        let invalid_path = malformed(
            plan.path_stream().unwrap().id,
            ArrivalTime(valid_camera.arrival_time.0.saturating_add(1)),
        );

        assert!(
            runtime
                .restore_transactional(invalid_path.arrival_time, &[valid_camera, invalid_path])
                .is_err()
        );
        assert_eq!(snapshot(&runtime), before);
    }

    #[test]
    fn identical_native_and_web_runtime_scenarios_have_one_result() {
        let (plan, messages, _) = populated_runtime();
        let mut native = FeatureRuntime::new(&plan, false);
        let mut web = FeatureRuntime::new(&plan, false);
        native.process_messages(Duration::from_millis(250), &messages);
        web.process_messages(Duration::from_millis(250), &messages);
        assert_eq!(snapshot(&native), snapshot(&web));
    }
}
