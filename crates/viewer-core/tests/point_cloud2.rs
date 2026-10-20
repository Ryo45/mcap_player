use bytes::Bytes;
use std::time::Duration;
use viewer_core::{
    ArrivalTime, FeatureRuntime, MeasurementTime, PathProjectionPolicy, PathState,
    PlaybackRequirements, PointCloudColorMode, PointCloudColoring, RawMessage,
    ScenePresentationInput, ScenePresentationState, SessionPlan, SourceCatalog, StreamDescriptor,
    StreamId, StreamTimingSummary, TelemetryState, TransformBatch, TransformStamped,
    TransformState, WorkspaceBindings,
};

const PAYLOAD: &[u8] =
    include_bytes!("../../../tests/fixtures/point-cloud2/organized-little-data.cdr");

fn plan() -> SessionPlan {
    let catalog = SourceCatalog {
        time_range: None,
        capabilities: Default::default(),
        streams: vec![StreamDescriptor {
            id: StreamId(1),
            topic: "/points".into(),
            schema: "sensor_msgs/msg/PointCloud2".into(),
            message_encoding: "cdr".into(),
            schema_definition: None,
            timing: StreamTimingSummary::default(),
        }],
    };
    let bindings = WorkspaceBindings {
        point_cloud_topic: "/points".into(),
        path_topic: "/path".into(),
        odometry_topic: "/odom".into(),
        dynamic_tf_topic: "/tf".into(),
        static_tf_topic: "/tf_static".into(),
    };
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_point_cloud();
    SessionPlan::build(&catalog, "/unused", &requirements, &bindings).unwrap()
}

fn raw(time: i64, payload: &'static [u8]) -> RawMessage {
    RawMessage {
        stream_id: StreamId(1),
        arrival_time: ArrivalTime(time),
        publish_time_ns: None,
        sequence: None,
        payload: Bytes::from_static(payload),
    }
}

#[test]
fn point_cloud2_uses_the_shared_route_and_failed_restore_leaves_xyz_and_attributes_intact() {
    let mut runtime = FeatureRuntime::new(&plan(), true);
    runtime.process_messages(Duration::ZERO, &[raw(10, PAYLOAD)]);
    let original = runtime
        .point_cloud()
        .unwrap()
        .point_cloud()
        .latest()
        .unwrap()
        .clone();
    assert_eq!(original.points.len(), 3);
    assert_eq!(original.attributes.intensity, Some(vec![0.0, 0.5, 1.0]));
    assert_eq!(runtime.counters().errors, 0);
    assert!(
        runtime
            .restore_transactional(ArrivalTime(20), &[raw(20, &[0xff])])
            .is_err()
    );
    assert_eq!(
        runtime.point_cloud().unwrap().point_cloud().latest(),
        Some(&original)
    );
    runtime.process_messages(Duration::ZERO, &[raw(20, &[0xff])]);
    assert_eq!(
        runtime.point_cloud().unwrap().point_cloud().latest(),
        Some(&original)
    );
    assert_eq!(runtime.counters().errors, 1);
    runtime.restore_transactional(ArrivalTime(5), &[]).unwrap();
    assert!(
        runtime
            .point_cloud()
            .unwrap()
            .point_cloud()
            .latest()
            .is_none()
    );
    runtime
        .restore_transactional(ArrivalTime(10), &[raw(10, PAYLOAD)])
        .unwrap();
    assert_eq!(
        runtime.point_cloud().unwrap().point_cloud().latest(),
        Some(&original)
    );
}

#[test]
fn tf_preserves_color_alignment_and_a_color_change_rebuilds_the_current_accumulated_cloud() {
    let mut runtime = FeatureRuntime::new(&plan(), true);
    runtime.process_messages(Duration::ZERO, &[raw(10, PAYLOAD)]);
    let mut transforms = TransformState::default();
    transforms.apply(TransformBatch {
        arrival_time: ArrivalTime(0),
        is_static: true,
        transforms: vec![TransformStamped {
            measurement_time: MeasurementTime(0),
            frame_id: "map".into(),
            child_frame_id: "lidar".into(),
            translation: [10.0, 0.0, 1.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
        }],
    });
    let path = PathState::default();
    let telemetry = TelemetryState::default();
    let policy = PathProjectionPolicy {
        fixed_frame: "map".into(),
        ..Default::default()
    };
    let cloud = runtime.point_cloud().unwrap().point_cloud();
    let mut presentation = ScenePresentationState::new();
    let input = |coloring| ScenePresentationInput {
        path: &path,
        telemetry: &telemetry,
        point_cloud: cloud,
        transforms: &transforms,
        platform: None,
        path_projection: &policy,
        accumulate: true,
        coloring,
    };
    let rgb = presentation.build(input(PointCloudColoring::default()));
    assert_eq!(
        rgb.cloud,
        [[11.0, 2.0, 4.0], [14.0, 5.0, 7.0], [9.0, -2.0, -2.0]]
    );
    assert_eq!(
        rgb.cloud_colors,
        [
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 128.0 / 255.0],
            [0.0, 0.0, 1.0, 1.0]
        ]
    );
    let revision = rgb.cloud_revision;
    let intensity = presentation.build(input(PointCloudColoring {
        mode: PointCloudColorMode::Intensity,
        intensity_range: [0.0, 1.0],
    }));
    assert_ne!(intensity.cloud_revision, revision);
    assert_eq!(intensity.cloud.len(), 3);
    assert_eq!(
        intensity.cloud_colors,
        [
            [0.0, 0.0, 1.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, 1.0]
        ]
    );
}
