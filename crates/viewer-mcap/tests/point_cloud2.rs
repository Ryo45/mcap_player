use std::{collections::BTreeMap, io::Cursor, time::Duration};
use viewer_core::{
    ArrivalTime, FeatureRuntime, PlaybackRequirements, SessionPlan, WorkspaceBindings,
};
use viewer_mcap::McapSource;

#[test]
fn indexed_point_cloud2_latest_before_preserves_ros_attributes_at_time_boundaries() {
    let mut output = Cursor::new(Vec::new());
    {
        let mut writer =
            mcap::Writer::with_options(&mut output, mcap::WriteOptions::new().compression(None))
                .unwrap();
        let schema = writer
            .add_schema(
                "sensor_msgs/msg/PointCloud2",
                "ros2msg",
                include_bytes!("../../../tests/fixtures/point-cloud2/schema.ros2msg"),
            )
            .unwrap();
        let channel = writer
            .add_channel(schema, "/points", "cdr", &BTreeMap::new())
            .unwrap();
        for time in [10, 20, 30] {
            writer
                .write_to_known_channel(
                    &mcap::records::MessageHeader {
                        channel_id: channel,
                        sequence: time as u32,
                        log_time: time,
                        publish_time: time,
                    },
                    include_bytes!(
                        "../../../tests/fixtures/point-cloud2/organized-little-data.cdr"
                    ),
                )
                .unwrap();
        }
        writer.finish().unwrap();
    }
    let mut source = McapSource::from_bytes(output.into_inner().into()).unwrap();
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_point_cloud();
    let bindings = WorkspaceBindings {
        point_cloud_topic: "/points".into(),
        path_topic: "/path".into(),
        odometry_topic: "/odom".into(),
        dynamic_tf_topic: "/tf".into(),
        static_tf_topic: "/tf_static".into(),
    };
    let plan = SessionPlan::build(source.catalog(), "/unused", &requirements, &bindings).unwrap();
    let selected = plan.selected_stream_ids();
    source.select_streams(selected.clone()).unwrap();
    let mut runtime = FeatureRuntime::new(&plan, true);
    for target in [10, 19, 20, 29, 30] {
        let restored = source
            .latest_before(&selected, ArrivalTime(target))
            .unwrap();
        assert_eq!(
            restored.messages[0].arrival_time,
            ArrivalTime(target / 10 * 10)
        );
        runtime
            .restore_transactional(ArrivalTime(target), &restored.messages)
            .unwrap();
        let frame = runtime
            .point_cloud()
            .unwrap()
            .point_cloud()
            .latest()
            .unwrap();
        assert_eq!(frame.points[0], [1.0, 2.0, 3.0]);
        assert_eq!(frame.attributes.rgba.as_ref().unwrap()[1], [0, 255, 0, 128]);
    }
    let messages = source.read_through(ArrivalTime(30)).unwrap();
    runtime.process_messages(Duration::ZERO, &messages);
    assert_eq!(
        runtime
            .point_cloud()
            .unwrap()
            .point_cloud()
            .latest()
            .unwrap()
            .arrival_time,
        ArrivalTime(30)
    );
}
