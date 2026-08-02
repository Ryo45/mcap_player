use std::{fs, path::PathBuf};
use viewer_core::{
    CameraCalibrationSet, DomainUpdate, McapSource, PipelineSet, SceneFrameBuilder, StreamBinding,
    TransformState,
};

fn fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap");
    fs::read(path).expect("camera/scan/TF fixture")
}

#[test]
fn every_scan_resolves_to_world_at_its_measurement_time() {
    let mut source = McapSource::new(fixture()).unwrap();
    let scan = source.catalog().by_topic("/scan").unwrap();
    let dynamic_tf = source.catalog().by_topic("/tf").unwrap();
    let static_tf = source.catalog().by_topic("/tf_static").unwrap();
    let mut pipelines = PipelineSet::new(
        &source.catalog().streams,
        &[
            (scan.id, StreamBinding::LaserScan),
            (
                dynamic_tf.id,
                StreamBinding::Transforms { is_static: false },
            ),
            (static_tf.id, StreamBinding::Transforms { is_static: true }),
        ],
    );
    let (_, end) = source.time_range();
    let mut transforms = TransformState::default();
    let mut scan_count = 0;

    for message in source.read_until(end).unwrap() {
        let mut updates = Vec::new();
        pipelines.decode(message, &mut updates);
        for update in updates {
            match update {
                DomainUpdate::Transforms(batch) => transforms.apply(batch),
                DomainUpdate::PointCloud(scan) => {
                    let world = transforms
                        .transform_points_at(
                            &scan.frame_id,
                            "odom",
                            scan.measurement_time,
                            &scan.points,
                        )
                        .expect("scan-time base_scan -> odom transform");
                    assert_eq!(world.len(), scan.points.len());
                    assert!(world.iter().flatten().all(|value| value.is_finite()));
                    scan_count += 1;
                }
                _ => unreachable!("only scan and TF streams are bound"),
            }
        }
    }

    assert_eq!(scan_count, 44);
    assert_eq!(transforms.static_len(), 15);
    assert_eq!(transforms.dynamic_len(), 3);
    for frame in [
        "camera_front_optical_frame",
        "camera_rear_optical_frame",
        "camera_left_optical_frame",
        "camera_right_optical_frame",
        "camera_front_left_optical_frame",
        "camera_front_right_optical_frame",
        "camera_rear_left_optical_frame",
    ] {
        assert!(
            transforms
                .transform_points("base_link", frame, &[[1.0, 0.0, 0.0]])
                .is_some(),
            "base_link -> {frame} is missing"
        );
    }
}

#[test]
fn seven_camera_fixture_discovers_and_decodes_all_topics() {
    let bytes = fixture();
    let mut playback =
        viewer_core::McapPlayback::new(bytes, "/camera/front/image/compressed").unwrap();
    playback
        .apply_command(viewer_core::PlaybackCommand::Toggle)
        .unwrap();
    playback.tick(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(
        playback.camera_topics(),
        &[
            (
                viewer_core::CameraId(0),
                "/camera/front/image/compressed".into(),
            ),
            (
                viewer_core::CameraId(1),
                "/camera/rear/image/compressed".into(),
            ),
            (
                viewer_core::CameraId(2),
                "/camera/left/image/compressed".into(),
            ),
            (
                viewer_core::CameraId(3),
                "/camera/right/image/compressed".into(),
            ),
            (
                viewer_core::CameraId(4),
                "/camera/front_left/image/compressed".into(),
            ),
            (
                viewer_core::CameraId(5),
                "/camera/front_right/image/compressed".into(),
            ),
            (
                viewer_core::CameraId(6),
                "/camera/rear_left/image/compressed".into(),
            ),
        ]
    );
    for camera_id in 0..7 {
        assert!(
            playback
                .state()
                .camera
                .latest_for(viewer_core::CameraId(camera_id))
                .is_some(),
            "camera {camera_id} did not decode"
        );
    }
    let calibrations =
        CameraCalibrationSet::from_json(include_str!("../../../config/camera_calibration.json"))
            .unwrap();
    let path = playback.state().bev.latest().expect("fixture path");
    let mut total_visible = 0;
    for (camera_id, camera) in playback.state().camera.frames() {
        let projected = calibrations
            .project_plan(camera, path, &playback.state().transforms, (320, 240))
            .expect("camera projection");
        if *camera_id == viewer_core::CameraId(0) {
            assert!(
                projected.visible_points >= 20,
                "front camera only sees {} path points",
                projected.visible_points
            );
        }
        total_visible += projected.visible_points;
    }
    assert!(total_visible > 0, "plan is not visible in any camera");

    let raw_scan = playback.state().point_cloud.latest().expect("fixture scan");
    assert_eq!(raw_scan.frame_id, "base_scan");
    let mut scene_builder = SceneFrameBuilder::new();
    let scene = scene_builder.build(playback.state(), true);
    assert!(!scene.cloud.is_empty());
    assert_eq!(
        scene.diagnostics.last_tf_route.as_deref(),
        Some("base_scan → odom")
    );
}
