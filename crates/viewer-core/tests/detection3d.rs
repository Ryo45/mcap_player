use std::time::Duration;
use viewer_core::{
    ArrivalTime, DecodeError, Detection3DArray, FeatureRuntime, MeasurementTime,
    PlaybackRequirements, RawMessage, SessionPlan, SourceCatalog, StreamDescriptor, StreamId,
    StreamTimingSummary, WorkspaceBindings, decode_detection3d_array, encode_detection3d_array_cdr,
};
fn golden() -> Detection3DArray {
    decode_detection3d_array(include_bytes!(
        "../../../tests/fixtures/ros2-detection3d/jazzy.cdr"
    ))
    .unwrap()
}
fn plan(enabled: bool) -> SessionPlan {
    let catalog = SourceCatalog {
        time_range: None,
        capabilities: Default::default(),
        streams: vec![StreamDescriptor {
            id: StreamId(8),
            topic: "/detections".into(),
            schema: "vision_msgs/msg/Detection3DArray".into(),
            schema_definition: None,
            message_encoding: "cdr".into(),
            timing: StreamTimingSummary::default(),
        }],
    };
    let bindings = WorkspaceBindings {
        path_topic: "/path".into(),
        odometry_topic: "/odom".into(),
        point_cloud_topic: "/scan".into(),
        dynamic_tf_topic: "/tf".into(),
        static_tf_topic: "/tf_static".into(),
    };
    let mut requirements = PlaybackRequirements::empty();
    if enabled {
        requirements.require_detection_topic("/detections");
        requirements.require_detection_topic("/detections");
    }
    SessionPlan::build(&catalog, "", &requirements, &bindings).unwrap()
}
fn raw(time: i64, x: f64) -> RawMessage {
    let mut message = golden();
    message.detections[0].bbox.center.position[0] = x;
    RawMessage {
        stream_id: StreamId(8),
        arrival_time: ArrivalTime(time),
        publish_time_ns: None,
        sequence: None,
        payload: encode_detection3d_array_cdr(&message).unwrap().into(),
    }
}
#[test]
fn independent_jazzy_rclpy_bytes_preserve_full_ros_fields() {
    let message = golden();
    assert_eq!(message.header.stamp, MeasurementTime(12_000_003_456));
    assert_eq!(message.header.frame_id, "sensor");
    assert_eq!(message.detections.len(), 2);
    let d = &message.detections[0];
    assert_eq!(d.header, message.header);
    assert_eq!(d.id, "track-α-42");
    assert_eq!(d.bbox.center.position, [1.25, -2., 0.75]);
    assert_eq!(d.bbox.size, [0.4, 0.3, 1.]);
    assert_eq!(d.results.len(), 2);
    assert_eq!(d.results[0].hypothesis.class_id, "person");
    assert_eq!(d.results[0].hypothesis.score, 0.91);
    assert_eq!(d.results[0].pose.pose.position[0], 3.5);
    assert_eq!(d.results[0].pose.covariance[35], 35.25);
    assert_eq!(d.results[1].pose.covariance[0], -1.);
    assert_eq!(d.results[1].pose.pose.position[0], 7.);
    assert_eq!(message.detections[1].id, "");
    assert!(message.detections[1].results.is_empty());
    assert_eq!(message.detections[1].bbox.center.position[0], -4.);
    assert_eq!(
        decode_detection3d_array(&encode_detection3d_array_cdr(&message).unwrap()).unwrap(),
        message
    );
}
#[test]
fn malformed_cdr_limits_and_all_truncations_fail_without_panics() {
    let bytes = include_bytes!("../../../tests/fixtures/ros2-detection3d/jazzy.cdr");
    for length in 0..bytes.len() {
        assert!(
            decode_detection3d_array(&bytes[..length]).is_err(),
            "length {length}"
        );
    }
    let mut extra = bytes.to_vec();
    extra.push(0);
    assert!(decode_detection3d_array(&extra).is_err());
    let mut wrong_encap = bytes.to_vec();
    wrong_encap[1] = 3;
    assert!(matches!(
        decode_detection3d_array(&wrong_encap),
        Err(DecodeError::InvalidEncapsulation)
    ));
    let mut too_many = golden();
    too_many
        .detections
        .resize(257, too_many.detections[0].clone());
    assert!(encode_detection3d_array_cdr(&too_many).is_err());
    let mut bad = golden();
    bad.detections[0].results[0].hypothesis.class_id = "x".repeat(129);
    assert!(encode_detection3d_array_cdr(&bad).is_err());
    let mut bad = golden();
    bad.detections[0].bbox.size[0] = f64::NAN;
    assert!(encode_detection3d_array_cdr(&bad).is_err());
    let mut oversized_string = vec![0, 1, 0, 0];
    oversized_string.extend_from_slice(&0i32.to_le_bytes());
    oversized_string.extend_from_slice(&0u32.to_le_bytes());
    oversized_string.extend_from_slice(&1_000_000u32.to_le_bytes());
    assert!(matches!(
        decode_detection3d_array(&oversized_string),
        Err(DecodeError::InvalidLength)
    ));
}
#[test]
fn selection_coalescing_and_transactional_predecessor_restore() {
    assert!(plan(false).selected_stream_ids().is_empty());
    let plan = plan(true);
    assert_eq!(plan.selected_stream_ids(), [StreamId(8)]);
    assert_eq!(plan.restore_inputs().len(), 1);
    let mut runtime = FeatureRuntime::new(&plan, false);
    let mut old_bad = raw(1, 0.);
    old_bad.payload = vec![0].into();
    runtime.process_messages(Duration::ZERO, &[raw(2, 2.), old_bad, raw(2, 3.)]);
    let state = runtime.detections_for_stream(StreamId(8)).unwrap();
    assert_eq!(
        state.state().latest().unwrap().message.detections[0]
            .bbox
            .center
            .position[0],
        3.
    );
    assert_eq!(state.counters().decoded, 1);
    assert_eq!(state.counters().dropped, 2);
    assert_eq!(state.counters().errors, 0);
    let revision = state.state().revision();
    let before = runtime.counters();
    let mut bad = raw(1, 1.);
    bad.payload = vec![0].into();
    assert!(
        runtime
            .restore_transactional(ArrivalTime(1), &[bad])
            .is_err()
    );
    assert_eq!(runtime.counters(), before);
    assert_eq!(
        runtime
            .detections_for_stream(StreamId(8))
            .unwrap()
            .state()
            .revision(),
        revision
    );
    runtime
        .restore_transactional(ArrivalTime(1), &[raw(1, 1.)])
        .unwrap();
    assert_eq!(
        runtime
            .detections_for_stream(StreamId(8))
            .unwrap()
            .state()
            .latest()
            .unwrap()
            .arrival_time,
        ArrivalTime(1)
    );
    runtime.restore_transactional(ArrivalTime(0), &[]).unwrap();
    assert!(
        runtime
            .detections_for_stream(StreamId(8))
            .unwrap()
            .state()
            .latest()
            .is_none()
    );
}
