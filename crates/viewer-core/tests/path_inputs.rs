use std::time::Duration;
use viewer_core::{
    ArrivalTime, FeatureRuntime, MeasurementTime, PathMessage, PlaybackRequirements, RawMessage,
    SessionPlan, SourceCapabilities, SourceCatalog, StreamDescriptor, StreamId,
    StreamTimingSummary, WorkspaceBindings, encode_path_cdr,
};

fn bindings() -> WorkspaceBindings {
    WorkspaceBindings {
        path_topic: "/planning/path".into(),
        odometry_topic: "/odom".into(),
        point_cloud_topic: "/scan".into(),
        dynamic_tf_topic: "/tf".into(),
        static_tf_topic: "/tf_static".into(),
    }
}
fn catalog() -> SourceCatalog {
    SourceCatalog {
        time_range: None,
        capabilities: SourceCapabilities::INDEXED_RECORDING,
        streams: [
            (1, "/planning/path"),
            (2, "/perception/objects"),
            (3, "/unused/path"),
        ]
        .into_iter()
        .map(|(id, topic)| StreamDescriptor {
            id: StreamId(id),
            topic: topic.into(),
            schema: "nav_msgs/msg/Path".into(),
            message_encoding: "cdr".into(),
            schema_definition: None,
            timing: StreamTimingSummary::default(),
        })
        .collect(),
    }
}
fn plan(input: Option<&str>) -> SessionPlan {
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_path();
    if let Some(input) = input {
        requirements.require_path_topic(input);
        requirements.require_path_topic(input);
    }
    SessionPlan::build(&catalog(), "", &requirements, &bindings()).unwrap()
}
fn message(id: u32, time: i64, x: f64) -> RawMessage {
    RawMessage {
        stream_id: StreamId(id),
        arrival_time: ArrivalTime(time),
        publish_time_ns: None,
        sequence: None,
        payload: encode_path_cdr(&PathMessage {
            frame_id: "odom".into(),
            measurement_time: MeasurementTime(time + 100),
            points: vec![[x, 0., 0.]],
        })
        .unwrap()
        .into(),
    }
}
#[test]
fn selection_is_independent_and_shared_consumers_do_not_duplicate_decoding() {
    assert_eq!(plan(None).selected_stream_ids(), [StreamId(1)]);
    let shared = plan(Some("/planning/path"));
    assert!(shared.additional_path_streams().is_empty());
    assert_eq!(shared.restore_inputs().len(), 1);
    let mut runtime = FeatureRuntime::new(&shared, false);
    runtime.process_messages(Duration::ZERO, &[message(1, 10, 1.)]);
    assert!(std::ptr::eq(
        runtime.path(),
        runtime.path_for_stream(StreamId(1)).unwrap()
    ));
    assert_eq!(runtime.counters().decoded, 1);
    let independent = plan(Some("/perception/objects"));
    assert_eq!(
        independent.selected_stream_ids(),
        [StreamId(1), StreamId(2)]
    );
    assert_eq!(independent.restore_inputs().len(), 2);
    let mut runtime = FeatureRuntime::new(&independent, false);
    runtime.process_messages(
        Duration::ZERO,
        &[
            message(1, 10, 1.),
            message(2, 20, 20.),
            message(1, 30, 3.),
            message(2, 25, 25.),
        ],
    );
    assert_eq!(runtime.path().state().latest().unwrap().points[0][0], 3.);
    assert_eq!(
        runtime
            .path_for_stream(StreamId(2))
            .unwrap()
            .state()
            .latest()
            .unwrap()
            .points[0][0],
        25.
    );
    assert_eq!(runtime.counters().decoded, 3); // Two built-in Path messages, one latest-only input.
    assert_eq!(runtime.counters().dropped, 1);
    assert_eq!(runtime.counters().unknown_streams, 0);
    assert!(runtime.path_for_stream(StreamId(3)).is_none());
}
#[test]
fn admission_chooses_log_time_then_batch_order_before_decode() {
    let mut runtime = FeatureRuntime::new(&plan(Some("/perception/objects")), false);
    let mut old_invalid = message(2, 10, 1.);
    old_invalid.payload = vec![0].into();
    runtime.process_messages(
        Duration::ZERO,
        &[message(2, 20, 2.), old_invalid, message(2, 20, 3.)],
    );
    let input = runtime.path_for_stream(StreamId(2)).unwrap();
    assert_eq!(input.state().latest().unwrap().points[0][0], 3.);
    assert_eq!(input.counters().decoded, 1);
    assert_eq!(input.counters().dropped, 2);
    assert_eq!(
        input.counters().errors,
        0,
        "discarded serialized input must not be decoded"
    );
}
#[test]
fn independent_restore_rolls_back_both_inputs_and_clears_missing_predecessors() {
    let mut runtime = FeatureRuntime::new(&plan(Some("/perception/objects")), false);
    runtime.process_messages(Duration::ZERO, &[message(1, 20, 2.), message(2, 20, 4.)]);
    let before = runtime.counters();
    let revision = runtime.path().state().revision();
    let mut invalid = message(2, 10, 1.);
    invalid.payload = vec![0].into();
    assert!(
        runtime
            .restore_transactional(ArrivalTime(10), &[message(1, 10, 1.), invalid])
            .is_err()
    );
    assert_eq!(runtime.path().state().revision(), revision);
    assert_eq!(runtime.counters(), before);
    assert_eq!(
        runtime
            .path_for_stream(StreamId(2))
            .unwrap()
            .state()
            .latest()
            .unwrap()
            .points[0][0],
        4.
    );
    runtime
        .restore_transactional(ArrivalTime(10), &[message(1, 10, 1.), message(2, 5, 0.5)])
        .unwrap();
    assert_eq!(
        runtime.path().state().latest().unwrap().arrival_time,
        ArrivalTime(10)
    );
    assert_eq!(
        runtime
            .path_for_stream(StreamId(2))
            .unwrap()
            .state()
            .latest()
            .unwrap()
            .arrival_time,
        ArrivalTime(5)
    );
    runtime.restore_transactional(ArrivalTime(1), &[]).unwrap();
    assert!(runtime.path().state().latest().is_none());
    assert!(
        runtime
            .path_for_stream(StreamId(2))
            .unwrap()
            .state()
            .latest()
            .is_none()
    );
}
#[test]
fn required_input_rejects_missing_or_wrong_encoding_and_bounds_route_count() {
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_path_topic("/perception/objects");
    let mut catalog = catalog();
    catalog.streams[1].message_encoding = "json".into();
    assert!(SessionPlan::build(&catalog, "", &requirements, &bindings()).is_err());
    for i in 0..33 {
        requirements.require_path_topic(format!("/extra/{i}"));
    }
    assert!(
        SessionPlan::build(&catalog, "", &requirements, &bindings())
            .unwrap_err()
            .to_string()
            .contains("32")
    );
}
