use std::{fs, path::PathBuf, time::Duration};
use viewer_core::{
    CameraCalibrationSet, CameraController, McapPlayback, McapSource, OdometryController,
    PathController, PlaybackRequirements, SceneController, SessionPlan, TransformController,
};

fn fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap");
    fs::read(path).expect("camera/scan/TF fixture")
}

#[test]
fn every_scan_resolves_to_world_at_its_measurement_time() {
    let mut source = McapSource::new(fixture()).unwrap();
    let plan = SessionPlan::build(
        source.catalog(),
        "/camera/front/image/compressed",
        &PlaybackRequirements::default(),
    )
    .unwrap();
    source.select_streams(plan.selected_stream_ids());
    let mut transforms = TransformController::new(&plan);
    let mut scene = SceneController::new(&plan);
    let (_, end) = source.time_range();
    let mut scan_count = 0;

    for message in source.read_until(end).unwrap() {
        transforms.process(&message);
        if scene.process(&message) {
            let scan = scene.point_cloud().latest().expect("decoded scan");
            let world = transforms
                .state()
                .transform_points_at(&scan.frame_id, "odom", scan.measurement_time, &scan.points)
                .expect("scan-time base_scan -> odom transform");
            assert_eq!(world.len(), scan.points.len());
            assert!(world.iter().flatten().all(|value| value.is_finite()));
            scan_count += 1;
        }
    }

    assert_eq!(scan_count, 44);
    assert_eq!(transforms.state().static_len(), 15);
    assert_eq!(transforms.state().dynamic_len(), 3);
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
                .state()
                .transform_points("base_link", frame, &[[1.0, 0.0, 0.0]])
                .is_some(),
            "base_link -> {frame} is missing"
        );
    }
}

#[test]
fn seven_camera_fixture_routes_exact_messages_to_concrete_controllers() {
    let bytes = fixture();
    let mut playback = McapPlayback::new(bytes).unwrap();
    let plan = SessionPlan::build(
        playback.catalog(),
        "/camera/front/image/compressed",
        &PlaybackRequirements::default(),
    )
    .unwrap();
    playback.select_streams(plan.selected_stream_ids());
    let mut cameras = CameraController::new(&plan);
    let mut path = PathController::new(&plan);
    let mut odometry = OdometryController::new(&plan);
    let mut transforms = TransformController::new(&plan);
    let mut scene = SceneController::new(&plan);
    playback.clock_mut().toggle();
    playback
        .tick(Duration::from_secs(10), |elapsed, messages| {
            for message in &messages {
                cameras.admit(message);
                path.process(message);
                odometry.process(message);
                transforms.process(message);
                scene.process(message);
            }
            cameras.advance(elapsed);
        })
        .unwrap();

    assert_eq!(
        cameras.topics(),
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
            cameras
                .state()
                .latest_for(viewer_core::CameraId(camera_id))
                .is_some(),
            "camera {camera_id} did not decode"
        );
    }

    let calibrations =
        CameraCalibrationSet::from_json(include_str!("../../../config/camera_calibration.json"))
            .unwrap();
    let path_frame = path.state().latest().expect("fixture path");
    let telemetry = odometry.state().latest().expect("fixture odometry");
    assert!(telemetry.speed.is_finite());
    assert_eq!(transforms.state().static_len(), 15);
    assert_eq!(transforms.state().dynamic_len(), 3);
    let mut total_visible = 0;
    for (camera_id, camera) in cameras.state().frames() {
        let projected = calibrations
            .project_plan(camera, path_frame, transforms.state(), (320, 240))
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

    let raw_scan = scene.point_cloud().latest().expect("fixture scan");
    assert_eq!(raw_scan.frame_id, "base_scan");
    let snapshot = scene.snapshot(path.state(), odometry.state(), transforms.state(), true);
    assert!(!snapshot.cloud.is_empty());
    assert_eq!(
        snapshot.diagnostics.last_tf_route.as_deref(),
        Some("base_scan → odom")
    );
    assert_eq!(cameras.counters().errors, 0);
    assert_eq!(path.counters().errors, 0);
    assert_eq!(odometry.counters().errors, 0);
    assert_eq!(transforms.counters().errors, 0);
    assert_eq!(scene.counters().errors, 0);
}
