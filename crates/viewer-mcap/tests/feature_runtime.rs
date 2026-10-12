use std::time::Duration;
use viewer_core::*;
#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use viewer_core::{
        CameraFrame, PathFrame, PlaybackRequirements, PointCloudFrame, SourceCatalog,
        TelemetryFrame, WorkspaceBindings,
    };

    #[derive(Clone, Debug, PartialEq)]
    struct VisibleSnapshot {
        cameras: Vec<(CameraId, CameraFrame)>,
        path: Option<PathFrame>,
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
                .point_cloud()
                .and_then(|scene| scene.point_cloud().latest())
                .cloned(),
            priority: runtime.scheduling_priority(),
            counters: runtime.counters(),
        }
    }

    fn populated_runtime() -> (SessionPlan, Vec<RawMessage>, FeatureRuntime) {
        let mut source = viewer_mcap::McapSource::copy_from_slice((fixture()).as_ref()).unwrap();
        let catalog: SourceCatalog = source.catalog().clone();
        let plan = SessionPlan::build(
            &catalog,
            "/camera/front/image/compressed",
            &requirements(),
            &bindings(),
        )
        .unwrap();
        source.select_streams(plan.selected_stream_ids()).unwrap();
        let end = source.time_range().end_inclusive();
        let messages = source.read_through(end).unwrap();
        let mut runtime = FeatureRuntime::new(&plan, true);
        runtime.process_messages(Duration::from_secs(10), &messages);
        (plan, messages, runtime)
    }

    fn malformed(stream_id: StreamId, arrival_time: ArrivalTime) -> RawMessage {
        RawMessage {
            publish_time_ns: None,
            sequence: None,
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

#[cfg(test)]
mod sharing_tests {
    use super::*;
    #[test]
    fn continuous_materialization_is_shared_across_consumers() {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap"),
        )
        .unwrap();
        let mut source = viewer_mcap::McapSource::copy_from_slice((bytes).as_ref()).unwrap();
        let mut requirements = viewer_core::PlaybackRequirements::empty();
        requirements.require_camera_topic("/camera/front/image/compressed");
        requirements.require_transforms();
        requirements.require_point_cloud();
        let bindings = viewer_core::WorkspaceBindings {
            path_topic: "/planning/path".into(),
            odometry_topic: "/odom".into(),
            point_cloud_topic: "/scan".into(),
            dynamic_tf_topic: "/tf".into(),
            static_tf_topic: "/tf_static".into(),
        };
        let plan = SessionPlan::build(
            source.catalog(),
            "/camera/front/image/compressed",
            &requirements,
            &bindings,
        )
        .unwrap();
        source.select_streams(plan.selected_stream_ids()).unwrap();
        let messages = source
            .read_through(source.time_range().end_inclusive())
            .unwrap();
        let mut runtime = FeatureRuntime::new(&plan, true);
        runtime.process_messages(Duration::from_secs(10), &messages);
        let counts = runtime.counters();
        assert_eq!(runtime.cameras().counters().decoded, 1);
        assert_eq!(
            runtime.transforms().counters().decoded,
            messages
                .iter()
                .filter(
                    |m| Some(m.stream_id) == plan.dynamic_tf_stream().map(|s| s.id)
                        || Some(m.stream_id) == plan.static_tf_stream().map(|s| s.id)
                )
                .count() as u64
        );
        assert_eq!(
            runtime.point_cloud().unwrap().counters().decoded,
            messages
                .iter()
                .filter(|m| Some(m.stream_id) == plan.point_cloud_stream().map(|s| s.id))
                .count() as u64
        );
        for _ in 0..10 {
            // Camera panels/overlays, Scene and diagnostics borrow the very same feature states.
            let first = runtime.cameras().state().latest_for(CameraId(0)).unwrap();
            let second = runtime.cameras().state().latest_for(CameraId(0)).unwrap();
            assert_eq!(first.jpeg.as_ptr(), second.jpeg.as_ptr());
            assert!(std::ptr::eq(
                runtime.transforms().state(),
                runtime.transforms().state()
            ));
            assert!(std::ptr::eq(
                runtime.point_cloud().unwrap().point_cloud(),
                runtime.point_cloud().unwrap().point_cloud()
            ));
        }
        assert_eq!(runtime.counters(), counts);
    }
}
