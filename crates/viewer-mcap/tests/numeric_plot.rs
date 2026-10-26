use bytes::Bytes;
use std::collections::BTreeMap;
use std::io::Cursor;
use viewer_core::{
    ArrivalTime, DataWindowTimeRange, PlaybackRequirements, RosMessageSchema, SchemaDefinition,
    SessionPlan, StreamId, TopicTree, WorkspaceBindings,
};
use viewer_mcap::{
    McapSource, McapSourceError, NumericPlotError, NumericSeriesRequest, QueryLimits,
};

const SCHEMA: &str = "string label\nuint8[] padding\nexample_msgs/Vector vector\nuint32 trailing\n===\nMSG: example_msgs/Vector\nint16 small\nfloat64 x\nfloat32 y\n";
fn definition(text: &str) -> SchemaDefinition {
    SchemaDefinition {
        encoding: "ros2msg".into(),
        data: Bytes::copy_from_slice(text.as_bytes()),
    }
}
fn align(bytes: &mut Vec<u8>, size: usize) {
    let padding = (size - (bytes.len() - 4) % size) % size;
    bytes.resize(bytes.len() + padding, 0);
}
fn payload(x: f64, big: bool, trailing: bool) -> Vec<u8> {
    let mut b = vec![0, u8::from(!big), 0, 0];
    b.extend(if big {
        4u32.to_be_bytes()
    } else {
        4u32.to_le_bytes()
    });
    b.extend(b"abc\0");
    b.extend(if big {
        3u32.to_be_bytes()
    } else {
        3u32.to_le_bytes()
    });
    b.extend([1, 2, 3]);
    align(&mut b, 2);
    b.extend(if big {
        (-17i16).to_be_bytes()
    } else {
        (-17i16).to_le_bytes()
    });
    align(&mut b, 8);
    b.extend(if big {
        x.to_be_bytes()
    } else {
        x.to_le_bytes()
    });
    if trailing {
        b.extend(if big {
            2.25f32.to_be_bytes()
        } else {
            2.25f32.to_le_bytes()
        });
        b.extend(if big {
            19u32.to_be_bytes()
        } else {
            19u32.to_le_bytes()
        });
    }
    b
}
fn recording(count: u32) -> Vec<u8> {
    let mut output = Cursor::new(vec![]);
    {
        let mut writer = mcap::Writer::with_options(
            &mut output,
            mcap::WriteOptions::new().chunk_size(Some(4096)),
        )
        .unwrap();
        let schema = writer
            .add_schema("example_msgs/msg/Sample", "ros2msg", SCHEMA.as_bytes())
            .unwrap();
        let channel = writer
            .add_channel(schema, "/numeric", "cdr", &BTreeMap::new())
            .unwrap();
        for sequence in 0..count {
            writer
                .write_to_known_channel(
                    &mcap::records::MessageHeader {
                        channel_id: channel,
                        sequence,
                        log_time: 100 + sequence as u64,
                        publish_time: 90 + sequence as u64,
                    },
                    &payload((sequence % 97) as f64, false, true),
                )
                .unwrap();
        }
        writer.finish().unwrap();
    }
    output.into_inner()
}
#[test]
fn selected_nested_scalar_skips_string_sequence_and_alignment_in_both_endians() {
    let schema = RosMessageSchema::parse("example_msgs/msg/Sample", &definition(SCHEMA)).unwrap();
    assert_eq!(
        schema
            .numeric_fields()
            .unwrap()
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        ["vector.small", "vector.x", "vector.y", "trailing"]
    );
    for big in [false, true] {
        let bytes = payload(-123.5, big, true);
        assert_eq!(
            schema
                .compile_numeric("vector.small")
                .unwrap()
                .extract(&bytes)
                .unwrap(),
            -17.0
        );
        assert_eq!(
            schema
                .compile_numeric("vector.x")
                .unwrap()
                .extract(&bytes)
                .unwrap(),
            -123.5
        );
        assert_eq!(
            schema
                .compile_numeric("vector.y")
                .unwrap()
                .extract(&bytes)
                .unwrap(),
            2.25
        );
        assert_eq!(
            schema
                .compile_numeric("trailing")
                .unwrap()
                .extract(&bytes)
                .unwrap(),
            19.0
        );
        // Selected x remains extractable even without trailing fields. No universal decode.
        assert_eq!(
            schema
                .compile_numeric("vector.x")
                .unwrap()
                .extract(&payload(7.0, big, false))
                .unwrap(),
            7.0
        );
    }
    assert!(schema.compile_numeric("padding").is_err());
    assert!(schema.compile_numeric("vector.missing").is_err());
    let extractor = schema.compile_numeric("vector.x").unwrap();
    let mut bytes = payload(1.0, false, true);
    bytes[1] = 3;
    assert!(extractor.extract(&bytes).is_err());
    assert!(extractor.extract(&[0, 1, 0, 0]).is_err());
}
#[test]
fn scalar_types_arrays_and_constants_follow_wire_layout() {
    let schema = RosMessageSchema::parse("demo/msg/A", &definition("uint8 CONST=9\nbool enabled\nfloat64[2] covariance\nint8 a\nuint16 b\nint32 c\nint64 d\nuint64 e\n")).unwrap();
    let mut bytes = vec![0, 1, 0, 0, 1];
    align(&mut bytes, 8);
    bytes.extend([0; 16]);
    bytes.push(254);
    align(&mut bytes, 2);
    bytes.extend(65500u16.to_le_bytes());
    align(&mut bytes, 4);
    bytes.extend((-1_234_567i32).to_le_bytes());
    align(&mut bytes, 8);
    bytes.extend((-123i64).to_le_bytes());
    bytes.extend((1u64 << 40).to_le_bytes());
    for (path, expected) in [
        ("a", -2.0),
        ("b", 65500.0),
        ("c", -1234567.0),
        ("d", -123.0),
        ("e", (1u64 << 40) as f64),
    ] {
        assert_eq!(
            schema
                .compile_numeric(path)
                .unwrap()
                .extract(&bytes)
                .unwrap(),
            expected
        );
    }
}
#[test]
fn query_products_are_bounded_and_independent_of_playback_and_metadata() {
    let mut source = McapSource::copy_from_slice((recording(10_000)).as_ref()).unwrap();
    let stream = source.catalog().by_topic("/numeric").unwrap().id;
    let bindings = WorkspaceBindings {
        path_topic: "/path".into(),
        odometry_topic: "/odom".into(),
        point_cloud_topic: "/scan".into(),
        dynamic_tf_topic: "/tf".into(),
        static_tf_topic: "/tf_static".into(),
    };
    let plan = SessionPlan::build(
        source.catalog(),
        "/camera",
        &PlaybackRequirements::empty(),
        &bindings,
    )
    .unwrap();
    let selected = plan.selected_stream_ids();
    let tree = TopicTree::from_catalog(source.catalog());
    assert_eq!(tree.entries[0].numeric_fields.len(), 4);
    let request = NumericSeriesRequest {
        stream_id: stream,
        field_path: "vector.x".into(),
        range: DataWindowTimeRange::new(ArrivalTime(100), ArrivalTime(10100)).unwrap(),
        max_points: 31,
    };
    source.select_streams([stream]).unwrap();
    let first = source.read_through(ArrivalTime(101)).unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].publish_time_ns, Some(90));
    assert_eq!(first[0].sequence, Some(0));
    let overview = source.query_numeric_overview(&request).unwrap();
    assert_eq!(overview.input_sample_count, 10_000);
    assert_eq!(overview.decode_errors, 0);
    assert!(overview.samples.len() <= 31);
    assert!(
        overview
            .samples
            .iter()
            .all(|s| s.measurement_time.is_none())
    );
    let detail = source
        .query_numeric_detail(&request, QueryLimits::new(3, 1024).unwrap())
        .unwrap();
    assert_eq!(detail.samples.len(), 3);
    assert!(!detail.complete);
    assert_eq!(detail.samples[0].publish_time_ns, Some(90));
    let exact = source
        .query_numeric_current(stream, "vector.x", ArrivalTime(113))
        .unwrap()
        .unwrap();
    assert_eq!(exact.value, 13.0);
    assert_eq!(exact.log_time, ArrivalTime(113));
    assert_eq!(exact.publish_time_ns, Some(103));
    assert_eq!(exact.sequence, Some(13));
    assert!(
        source
            .query_numeric_current(stream, "vector.x", ArrivalTime(99))
            .unwrap()
            .is_none()
    );
    assert!(
        source
            .query_numeric_overview_cancellable(&request, || true)
            .is_err()
    );
    assert_eq!(source.read_through(ArrivalTime(102)).unwrap().len(), 1);
    assert_eq!(plan.selected_stream_ids(), selected);
    assert!(selected.is_empty());
}
#[test]
fn unsupported_and_recursive_schema_are_explicit() {
    let schema = RosMessageSchema::parse("demo/msg/A", &definition("A nested\n")).unwrap();
    assert!(schema.numeric_fields().is_err());
    assert!(schema.compile_numeric("value").is_err());
    assert!(
        RosMessageSchema::parse(
            "demo/msg/A",
            &SchemaDefinition {
                encoding: "ros2idl".into(),
                data: Bytes::new()
            }
        )
        .is_err()
    );
}

#[test]
fn overview_does_not_touch_unrelated_chunks() {
    let mut output = Cursor::new(vec![]);
    let channel;
    let unrelated;
    {
        let mut writer =
            mcap::Writer::with_options(&mut output, mcap::WriteOptions::new().compression(None))
                .unwrap();
        let schema = writer
            .add_schema("demo/msg/A", "ros2msg", b"float64 value\n")
            .unwrap();
        channel = writer
            .add_channel(schema, "/selected", "cdr", &BTreeMap::new())
            .unwrap();
        unrelated = writer
            .add_channel(schema, "/unrelated", "cdr", &BTreeMap::new())
            .unwrap();
        for stream in [channel, unrelated] {
            writer
                .write_to_known_channel(
                    &mcap::records::MessageHeader {
                        channel_id: stream,
                        sequence: 0,
                        log_time: 100,
                        publish_time: 99,
                    },
                    &[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 240, 63],
                )
                .unwrap();
            writer.flush().unwrap();
        }
        writer.finish().unwrap();
    }
    let mut bytes = output.into_inner();
    let summary = mcap::Summary::read(&bytes).unwrap().unwrap();
    let chunk = summary
        .chunk_indexes
        .iter()
        .find(|c| c.message_index_offsets.contains_key(&unrelated))
        .unwrap();
    // Corrupt an unrelated record header; touching this chunk must now fail.
    let offset = chunk.compressed_data_offset().unwrap() as usize;
    bytes[offset + 1..offset + 9].copy_from_slice(&u64::MAX.to_le_bytes());
    let mut source = McapSource::copy_from_slice((bytes).as_ref()).unwrap();
    source.select_streams([]).unwrap();
    assert!(source.read_through(ArrivalTime(101)).unwrap().is_empty());
    source.seek(ArrivalTime(101)).unwrap();
    let mut request = NumericSeriesRequest {
        stream_id: StreamId(channel as u32),
        field_path: "value".into(),
        range: DataWindowTimeRange::new(ArrivalTime(100), ArrivalTime(101)).unwrap(),
        max_points: 4,
    };
    let result = source.query_numeric_overview(&request).unwrap();
    assert_eq!(result.samples[0].value, 1.0);
    request.stream_id = StreamId(unrelated as u32);
    assert!(matches!(
        source.query_numeric_overview_cancellable(&request, || true),
        Err(NumericPlotError::Source(McapSourceError::QueryCancelled))
    ));
    assert!(source.query_numeric_overview(&request).is_err());
}
#[test]
fn nested_sequences_skip_without_building_values_and_reject_excessive_lengths() {
    let schema = RosMessageSchema::parse(
        "demo/msg/A",
        &definition("demo/B[] entries\nfloat64 value\n===\nMSG: demo/B\nstring text\n"),
    )
    .unwrap();
    let extractor = schema.compile_numeric("value").unwrap();
    let mut payload = vec![0, 1, 0, 0];
    payload.extend(2u32.to_le_bytes());
    for _ in 0..2 {
        payload.extend(4u32.to_le_bytes());
        payload.extend(b"abc\0");
    }
    align(&mut payload, 8);
    payload.extend(42f64.to_le_bytes());
    assert_eq!(extractor.extract(&payload).unwrap(), 42.0);
    payload[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(extractor.extract(&payload).is_err());
}

#[test]
fn empty_and_constant_only_ros_messages_consume_the_dummy_wire_member() {
    for body in ["", "uint8 CONST=9\n"] {
        let schema = RosMessageSchema::parse(
            "demo/msg/A",
            &definition(&format!(
                "std_msgs/Empty marker\nuint8 value\n===\nMSG: std_msgs/Empty\n{body}"
            )),
        )
        .unwrap();
        // ROS 2 rosidl inserts uint8 structure_needs_at_least_one_member=0.
        let bytes = [0, 1, 0, 0, 0, 42];
        assert_eq!(
            schema
                .compile_numeric("value")
                .unwrap()
                .extract(&bytes)
                .unwrap(),
            42.0
        );
        assert_eq!(schema.numeric_fields().unwrap().len(), 1);
        let schema = RosMessageSchema::parse(
            "demo/msg/A",
            &definition(&format!(
                "std_msgs/Empty[] markers\nuint8 value\n===\nMSG: std_msgs/Empty\n{body}"
            )),
        )
        .unwrap();
        assert_eq!(
            schema
                .compile_numeric("value")
                .unwrap()
                .extract(&[0, 1, 0, 0, 2, 0, 0, 0, 0, 0, 42])
                .unwrap(),
            42.0
        );
    }
}
#[test]
fn multiple_selected_fields_share_one_chunk_read_and_keep_separate_budgets() {
    let backing = recording(1000);
    let summary = mcap::Summary::read(&backing).unwrap().unwrap();
    let source = McapSource::copy_from_slice((backing).as_ref()).unwrap();
    let request = NumericSeriesRequest {
        stream_id: source.catalog().by_topic("/numeric").unwrap().id,
        field_path: "vector.x".into(),
        range: DataWindowTimeRange::new(ArrivalTime(100), ArrivalTime(1100)).unwrap(),
        max_points: 15,
    };
    let mut second = request.clone();
    second.field_path = "vector.small".into();
    second.max_points = 6;
    let batch = source
        .query_numeric_overviews_cancellable(&[request, second], || false)
        .unwrap();
    assert_eq!(batch.chunks_read, summary.chunk_indexes.len());
    assert_eq!(batch.series[0].input_sample_count, 1000);
    assert_eq!(batch.series[1].input_sample_count, 1000);
    assert!(batch.series[0].samples.len() <= 15);
    assert!(batch.series[1].samples.len() <= 6);
    assert!(batch.series[1].samples.iter().all(|s| s.value == -17.0));
}
