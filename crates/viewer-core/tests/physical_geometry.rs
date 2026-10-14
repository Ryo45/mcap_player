use std::time::Duration;
use viewer_core::{
    ArrivalTime, FeatureRuntime, LaserScan, MeasurementTime, PathMessage, PathProjectionPolicy,
    PlatformGeometry, PlaybackRequirements, RawMessage, ScenePresentationInput,
    ScenePresentationState, SessionPlan, SourceCatalog, StreamDescriptor, StreamId,
    StreamTimingSummary, TransformStamped, WorkspaceBindings, decode_path, encode_laser_scan_cdr,
    encode_path_cdr, encode_tf_message_cdr,
};
fn geometry() -> PlatformGeometry {
    serde_json::from_str(include_str!(
        "../../../config/platform_geometry/turtlebot3_waffle_pi.json"
    ))
    .unwrap()
}
fn runtime() -> FeatureRuntime {
    let bindings: WorkspaceBindings =
        serde_json::from_str(include_str!("../../../config/workspace_bindings.json")).unwrap();
    let catalog = SourceCatalog {
        streams: [
            ("/planning/path", "nav_msgs/msg/Path"),
            ("/scan", "sensor_msgs/msg/LaserScan"),
            ("/tf", "tf2_msgs/msg/TFMessage"),
            ("/tf_static", "tf2_msgs/msg/TFMessage"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (topic, schema))| StreamDescriptor {
            id: StreamId(i as u32),
            topic: topic.into(),
            schema: schema.into(),
            schema_definition: None,
            message_encoding: "cdr".into(),
            timing: StreamTimingSummary::default(),
        })
        .collect(),
        ..Default::default()
    };
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_path();
    requirements.require_point_cloud();
    requirements.require_transforms();
    let plan = SessionPlan::build(&catalog, "unused", &requirements, &bindings).unwrap();
    FeatureRuntime::new(&plan, true)
}
fn raw(id: u32, payload: Vec<u8>) -> RawMessage {
    RawMessage {
        stream_id: StreamId(id),
        arrival_time: ArrivalTime(10),
        publish_time_ns: None,
        sequence: None,
        payload: payload.into(),
    }
}
fn tf(parent: &str, child: &str, translation: [f64; 3], rotation: [f64; 4]) -> TransformStamped {
    TransformStamped {
        frame_id: parent.into(),
        child_frame_id: child.into(),
        measurement_time: MeasurementTime(10),
        translation,
        rotation,
    }
}
fn approximate(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
}
#[test]
fn changing_world_frame_clears_accumulated_cloud_and_invalidates_gpu_revision() {
    let mut runtime = runtime();
    let scan = LaserScan {
        measurement_time: MeasurementTime(10),
        frame_id: "odom".into(),
        angle_min: 0.0,
        angle_increment: 1.0,
        range_min: 0.12,
        range_max: 3.5,
        ranges: vec![1.0],
    };
    runtime.process_messages(
        Duration::ZERO,
        &[raw(1, encode_laser_scan_cdr(&scan).unwrap())],
    );
    let mut scene = ScenePresentationState::default();
    let mut policy = PathProjectionPolicy::default();
    let first = scene.build(ScenePresentationInput {
        path: runtime.path().state(),
        telemetry: runtime.odometry().state(),
        point_cloud: runtime.point_cloud().unwrap().point_cloud(),
        transforms: runtime.transforms().state(),
        platform: None,
        path_projection: &policy,
        accumulate: true,
    });
    assert_eq!(first.cloud.len(), 1);
    let revision = first.cloud_revision;
    policy.fixed_frame = "missing_frame".into();
    let next = scene.build(ScenePresentationInput {
        path: runtime.path().state(),
        telemetry: runtime.odometry().state(),
        point_cloud: runtime.point_cloud().unwrap().point_cloud(),
        transforms: runtime.transforms().state(),
        platform: None,
        path_projection: &policy,
        accumulate: true,
    });
    assert!(next.cloud.is_empty());
    assert_ne!(next.cloud_revision, revision);
    assert!(next.diagnostics.current_tf_error.is_some());
}
#[test]
fn physical_geometry_survives_cdr_controllers_tf_and_scene_without_a_scale_correction() {
    let path = PathMessage {
        measurement_time: MeasurementTime(10),
        frame_id: "base_link".into(),
        points: vec![[0.0, 0.0, 0.2], [1.0, 0.0, 0.2]],
    };
    let scan = LaserScan {
        measurement_time: MeasurementTime(10),
        frame_id: "base_scan".into(),
        angle_min: 0.0,
        angle_increment: std::f32::consts::FRAC_PI_2,
        range_min: 0.12,
        range_max: 3.5,
        ranges: vec![1.0, 2.0, f32::INFINITY, 0.01],
    };
    let q = std::f64::consts::FRAC_1_SQRT_2;
    let mut runtime = runtime();
    runtime.process_messages(
        Duration::ZERO,
        &[
            raw(
                2,
                encode_tf_message_cdr(&[tf(
                    "odom",
                    "base_footprint",
                    [10.0, 20.0, 0.0],
                    [0.0, 0.0, q, q],
                )])
                .unwrap(),
            ),
            raw(
                3,
                encode_tf_message_cdr(&[
                    tf(
                        "base_footprint",
                        "base_link",
                        [0.0, 0.0, 0.01],
                        [0.0, 0.0, 0.0, 1.0],
                    ),
                    tf(
                        "base_link",
                        "base_scan",
                        [-0.064, 0.0, 0.122],
                        [0.0, 0.0, 0.0, 1.0],
                    ),
                ])
                .unwrap(),
            ),
            raw(0, encode_path_cdr(&path).unwrap()),
            raw(1, encode_laser_scan_cdr(&scan).unwrap()),
        ],
    );
    assert_eq!(runtime.path().state().latest().unwrap().points, path.points);
    let sensor = runtime
        .point_cloud()
        .unwrap()
        .point_cloud()
        .latest()
        .unwrap();
    assert_eq!(sensor.points.len(), 2);
    approximate(f64::from(sensor.points[0][0]), 1.0);
    approximate(f64::from(sensor.points[1][1]), 2.0);
    let g = geometry();
    let extent = g.extent();
    approximate(extent[0], 0.2825);
    approximate(extent[1], 0.306);
    approximate(extent[2], 0.14125);
    let mut scene = ScenePresentationState::default();
    let policy = PathProjectionPolicy::default();
    let view = scene.build(ScenePresentationInput {
        path: runtime.path().state(),
        telemetry: runtime.odometry().state(),
        point_cloud: runtime.point_cloud().unwrap().point_cloud(),
        transforms: runtime.transforms().state(),
        platform: Some(&g),
        path_projection: &policy,
        accumulate: false,
    });
    assert!(
        view.diagnostics.current_tf_error.is_none()
            && view.diagnostics.path_tf_error.is_none()
            && view.diagnostics.platform_tf_error.is_none()
    );
    approximate(f64::from(view.path[0][0]), 10.0);
    approximate(f64::from(view.path[1][1]), 21.0);
    approximate(f64::from(view.path[1][2]), 0.21);
    let distance = view.path[0]
        .iter()
        .zip(view.path[1])
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f32>()
        .sqrt();
    approximate(f64::from(distance), 1.0);
    approximate(f64::from(view.cloud[0][1]), 20.936);
    approximate(f64::from(view.cloud[0][2]), 0.132);
    let bounds = view.platform.unwrap();
    approximate(
        f64::from(bounds.iter().map(|p| p[2]).fold(f32::INFINITY, f32::min)),
        0.0,
    );
    approximate(
        f64::from(
            bounds
                .iter()
                .map(|p| p[2])
                .fold(f32::NEG_INFINITY, f32::max),
        ),
        0.14125,
    );
    approximate(f64::from(view.bev.path[1][1]), 1.0);
    assert_eq!(view.bev.footprint, Some([-0.153, -0.197, 0.153, 0.0855]));
}
#[test]
fn z_is_round_tripped_and_mixed_pose_frames_are_rejected() {
    let path = PathMessage {
        measurement_time: MeasurementTime(10),
        frame_id: "base_link".into(),
        points: vec![[1.0, 2.0, 0.3], [4.0, 5.0, -0.4]],
    };
    let mut bytes = encode_path_cdr(&path).unwrap();
    assert_eq!(decode_path(&bytes).unwrap(), path);
    let occurrences = bytes
        .windows(9)
        .enumerate()
        .filter_map(|(i, w)| (w == b"base_link").then_some(i))
        .collect::<Vec<_>>();
    bytes[occurrences[1]..occurrences[1] + 9].copy_from_slice(b"other_abc");
    assert!(decode_path(&bytes).is_err());
    let broken = PathMessage {
        points: vec![[1.0, 0.0, f64::NAN]],
        ..path
    };
    assert!(encode_path_cdr(&broken).is_err());
}
#[test]
fn a_world_frame_path_is_not_rotated_again_by_the_ego_pose() {
    let path = viewer_core::PathFrame {
        measurement_time: MeasurementTime(10),
        arrival_time: ArrivalTime(10),
        frame_id: "odom".into(),
        points: vec![[2.0, 3.0, 0.5], [3.0, 3.0, 0.5]],
    };
    let policy = PathProjectionPolicy::default();
    let tf = viewer_core::TransformState::default();
    assert_eq!(
        policy.positions_in_fixed_frame(&path, &tf).unwrap(),
        path.points
    );
}

#[test]
fn geometry_change_invalidates_bev_even_when_world_pose_is_unavailable() {
    let path = viewer_core::PathState::default();
    let telemetry = viewer_core::TelemetryState::default();
    let cloud = viewer_core::PointCloudState::default();
    let tf = viewer_core::TransformState::default();
    let policy = PathProjectionPolicy::default();
    let mut scene = ScenePresentationState::default();
    let original = geometry();
    let other =
        PlatformGeometry::new("base_link".into(), [-1.0, -0.5, 0.0], [2.0, 0.5, 1.0]).unwrap();
    let first = scene.build(ScenePresentationInput {
        path: &path,
        telemetry: &telemetry,
        point_cloud: &cloud,
        transforms: &tf,
        platform: Some(&original),
        path_projection: &policy,
        accumulate: false,
    });
    let revision = first.revision;
    assert!(first.platform.is_none());
    let next = scene.build(ScenePresentationInput {
        path: &path,
        telemetry: &telemetry,
        point_cloud: &cloud,
        transforms: &tf,
        platform: Some(&other),
        path_projection: &policy,
        accumulate: false,
    });
    assert_ne!(next.revision, revision);
    assert_eq!(next.bev.footprint, Some([-0.5, -1.0, 0.5, 2.0]));
    assert!(next.diagnostics.platform_tf_error.is_some());
}

#[test]
fn malformed_path_z_does_not_commit_a_partially_restored_feature_state() {
    let original = PathMessage {
        measurement_time: MeasurementTime(10),
        frame_id: "odom".into(),
        points: vec![[1.0, 2.0, 0.3]],
    };
    let mut runtime = runtime();
    runtime.process_messages(
        Duration::ZERO,
        &[raw(0, encode_path_cdr(&original).unwrap())],
    );
    let before = runtime.path().state().latest().unwrap().clone();
    let mut bytes = encode_path_cdr(&original).unwrap();
    let z = bytes.len() - 40;
    bytes[z..z + 8].copy_from_slice(&f64::NAN.to_le_bytes());
    assert!(
        runtime
            .stage_restore(ArrivalTime(20), &[raw(0, bytes)])
            .is_err()
    );
    assert_eq!(runtime.path().state().latest(), Some(&before));
}

#[test]
fn camera_projection_uses_authored_height_and_tf_ground_plane() {
    use viewer_core::{
        CameraCalibrationSet, CameraFrame, CameraId, PathFrame, TransformBatch, TransformState,
    };
    let calibration=CameraCalibrationSet::from_json(r#"{"version":1,"provenance":"analytic test","cameras":[{"frame_id":"optical","projection_model":"pinhole","width":320,"height":240,"fx":200,"fy":200,"cx":160,"cy":120,"distortion_model":"none"}]}"#).unwrap();
    let mut transforms = TransformState::default();
    transforms.apply(TransformBatch {
        arrival_time: ArrivalTime(10),
        is_static: true,
        transforms: vec![
            tf(
                "base_link",
                "optical",
                [0.0, 0.0, 1.0],
                [-0.5, 0.5, -0.5, 0.5],
            ),
            tf(
                "base_footprint",
                "base_link",
                [0.0, 0.0, 0.01],
                [0.0, 0.0, 0.0, 1.0],
            ),
        ],
    });
    let frame = CameraFrame {
        camera_id: CameraId(0),
        measurement_time: MeasurementTime(20),
        arrival_time: ArrivalTime(20),
        frame_id: "optical".into(),
        jpeg: Default::default(),
    };
    let path = PathFrame {
        measurement_time: MeasurementTime(10),
        arrival_time: ArrivalTime(10),
        frame_id: "base_link".into(),
        points: vec![[2.0, 0.0, 0.25]],
    };
    let mut policy = PathProjectionPolicy {
        fixed_frame: "base_link".into(),
        ground_frame: None,
    };
    let pixel = calibration
        .project_plan(&frame, &path, &transforms, (320, 240), &policy)
        .unwrap()
        .points[0]
        .unwrap();
    // Camera at z=1 m, point at z=.25 m, depth 2 m: v=120+200*(1-.25)/2.
    approximate(f64::from(pixel[0]), 160.0);
    approximate(f64::from(pixel[1]), 195.0);
    policy.ground_frame = Some("base_footprint".into());
    let ground = calibration
        .project_plan(&frame, &path, &transforms, (320, 240), &policy)
        .unwrap()
        .points[0]
        .unwrap();
    approximate(f64::from(ground[1]), 221.0);
    assert_eq!(path.points[0][2], 0.25);
}
