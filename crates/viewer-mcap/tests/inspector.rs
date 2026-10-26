use bytes::Bytes;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;
use viewer_core::{
    ArrivalTime, InspectorContainer, InspectorMessage, InspectorScalar, InspectorSchema,
    InspectorValue, MeasurementTime, RawMessage, SchemaDefinition, StreamDescriptor, StreamId,
};
use viewer_mcap::{LatestMessageReader, McapSource, McapSourceError};
const SCHEMA: &str = "std_msgs/Header header\nbool enabled\nuint64 identifier\nfloat32[3] vector\nuint8[] data\nstring label\ndemo/Item[] items\n===\nMSG: std_msgs/Header\nbuiltin_interfaces/Time stamp\nstring frame_id\n===\nMSG: builtin_interfaces/Time\nint32 sec\nuint32 nanosec\n===\nMSG: demo/Item\nint16 code\nstring name\n";
fn stream(text: &str) -> StreamDescriptor {
    StreamDescriptor {
        id: StreamId(1),
        topic: "/sample".into(),
        schema: "demo/msg/Sample".into(),
        schema_definition: Some(Arc::new(SchemaDefinition {
            encoding: "ros2msg".into(),
            data: Bytes::copy_from_slice(text.as_bytes()),
        })),
        message_encoding: "cdr".into(),
        timing: Default::default(),
    }
}
struct Cdr {
    bytes: Vec<u8>,
    big: bool,
}
impl Cdr {
    fn new(big: bool) -> Self {
        Self {
            bytes: vec![0, u8::from(!big), 0, 0],
            big,
        }
    }
    fn align(&mut self, n: usize) {
        let pad = (n - (self.bytes.len() - 4) % n) % n;
        self.bytes.resize(self.bytes.len() + pad, 0);
    }
    fn u32(&mut self, n: u32) {
        self.align(4);
        self.bytes.extend(if self.big {
            n.to_be_bytes()
        } else {
            n.to_le_bytes()
        });
    }
    fn u64(&mut self, n: u64) {
        self.align(8);
        self.bytes.extend(if self.big {
            n.to_be_bytes()
        } else {
            n.to_le_bytes()
        });
    }
    fn i16(&mut self, n: i16) {
        self.align(2);
        self.bytes.extend(if self.big {
            n.to_be_bytes()
        } else {
            n.to_le_bytes()
        });
    }
    fn text(&mut self, s: &str) {
        self.u32(s.len() as u32 + 1);
        self.bytes.extend(s.as_bytes());
        self.bytes.push(0);
    }
}
fn payload(big: bool) -> Vec<u8> {
    let mut cdr = Cdr::new(big);
    cdr.u32((-2i32) as u32);
    cdr.u32(123);
    cdr.text("frame");
    cdr.bytes.push(1);
    cdr.u64(u64::MAX);
    for n in [1.25f32, -2.5, 3.0] {
        cdr.u32(n.to_bits());
    }
    cdr.u32(100000);
    cdr.bytes.extend((0..100000).map(|n| (n % 256) as u8));
    cdr.text("日本語");
    cdr.u32(150);
    for n in 0..150 {
        cdr.i16(n);
        cdr.text(&format!("item {n}"));
    }
    cdr.bytes
}
fn raw(bytes: Vec<u8>, time: i64) -> RawMessage {
    RawMessage {
        stream_id: StreamId(1),
        arrival_time: ArrivalTime(time),
        publish_time_ns: Some(0),
        sequence: Some(7),
        payload: bytes.into(),
    }
}
fn container(value: &InspectorValue) -> &InspectorContainer {
    let InspectorValue::Container(c) = value else {
        panic!("expected container")
    };
    c
}
#[test]
fn metadata_exact_scalars_and_lazy_nested_sequences_work_in_both_endians() {
    for big in [false, true] {
        let schema = InspectorSchema::for_stream(&stream(SCHEMA)).unwrap();
        let message = InspectorMessage::new(&schema, &raw(payload(big), 100)).unwrap();
        assert_eq!(message.log_time, ArrivalTime(100));
        assert_eq!(message.publish_time_ns, Some(0));
        assert_eq!(message.sequence, Some(7));
        assert_eq!(
            message.header_stamp.unwrap().measurement_time(),
            Some(MeasurementTime(-1_999_999_877))
        );
        let root = message.page(&message.root, None).unwrap();
        assert_eq!(root.fields.len(), 7);
        assert!(matches!(
            root.fields[1].value,
            InspectorValue::Scalar(InspectorScalar::Bool(true))
        ));
        assert!(matches!(
            root.fields[2].value,
            InspectorValue::Scalar(InspectorScalar::Unsigned(u64::MAX))
        ));
        let vector = message
            .page(container(&root.fields[3].value), None)
            .unwrap();
        assert!(
            matches!(vector.fields[1].value,InspectorValue::Scalar(InspectorScalar::Float(n)) if n == -2.5)
        );
        let data = container(&root.fields[4].value);
        assert_eq!(data.len(), 100000);
        let page = message.page(data, None).unwrap();
        assert_eq!(page.fields.len(), 64);
        let next = message.page(data, page.next).unwrap();
        assert_eq!(next.start_index, 64);
        assert!(matches!(
            next.fields[0].value,
            InspectorValue::Scalar(InspectorScalar::Unsigned(64))
        ));
        assert!(
            matches!(&root.fields[5].value,InspectorValue::Scalar(InspectorScalar::Text {preview,..}) if preview=="日本語")
        );
        let items = container(&root.fields[6].value);
        let first = message.page(items, None).unwrap();
        let second = message.page(items, first.next).unwrap();
        assert_eq!(second.start_index, 64);
        let item = message
            .page(container(&second.fields[0].value), None)
            .unwrap();
        assert!(matches!(
            item.fields[0].value,
            InspectorValue::Scalar(InspectorScalar::Signed(64))
        ));
        assert!(
            matches!(&item.fields[1].value,InspectorValue::Scalar(InspectorScalar::Text {preview,..}) if preview=="item 64")
        );
    }
}
#[test]
fn huge_strings_are_truncated_and_malformed_payloads_and_encodings_are_explicit() {
    let schema = InspectorSchema::for_stream(&stream("string text\n")).unwrap();
    let mut cdr = Cdr::new(false);
    cdr.text(&"語".repeat(2000));
    let model = InspectorMessage::new(&schema, &raw(cdr.bytes, 10)).unwrap();
    let page = model.page(&model.root, None).unwrap();
    assert!(
        matches!(&page.fields[0].value,InspectorValue::Scalar(InspectorScalar::Text {preview,truncated:true,bytes:6000}) if preview.chars().count()==512)
    );
    let model =
        InspectorMessage::new(&schema, &raw(vec![0, 1, 0, 0, 255, 255, 255, 255], 10)).unwrap();
    assert!(model.page(&model.root, None).is_err());
    assert!(InspectorMessage::new(&schema, &raw(vec![0, 3, 0, 0], 10)).is_err());
    let mut missing = stream("Other nested\n");
    assert!(InspectorSchema::for_stream(&missing).is_err());
    missing.message_encoding = "json".into();
    assert!(InspectorSchema::for_stream(&missing).is_err());
}
fn recording() -> Vec<u8> {
    let mut bytes = Cursor::new(vec![]);
    {
        let mut writer = mcap::Writer::with_options(
            &mut bytes,
            mcap::WriteOptions::new().compression(Some(mcap::Compression::Zstd)),
        )
        .unwrap();
        let schema = writer
            .add_schema("demo/msg/Scalar", "ros2msg", b"uint32 value\n")
            .unwrap();
        let channel = writer
            .add_channel(schema, "/scalar", "cdr", &BTreeMap::new())
            .unwrap();
        for n in [10u64, 20, 20, 30] {
            let mut cdr = Cdr::new(false);
            cdr.u32(n as u32);
            writer
                .write_to_known_channel(
                    &mcap::records::MessageHeader {
                        channel_id: channel,
                        sequence: n as u32,
                        log_time: n,
                        publish_time: n - 1,
                    },
                    &cdr.bytes,
                )
                .unwrap();
        }
        writer.finish().unwrap();
    }
    bytes.into_inner()
}
#[test]
fn adjacent_cursor_queries_reuse_serialized_chunk_and_index_without_mutating_playback() {
    let mut source = McapSource::from_bytes(recording().into()).unwrap();
    source.select_streams([]).unwrap();
    let before = source.time_range();
    let mut reader = LatestMessageReader::new(source.fork_reader());
    assert!(
        reader
            .read_at(StreamId(1), ArrivalTime(9))
            .unwrap()
            .is_none()
    );
    let first = reader
        .read_at(StreamId(1), ArrivalTime(10))
        .unwrap()
        .unwrap();
    let reads = reader.diagnostics();
    assert_eq!(reads.chunks_read, 1);
    assert_eq!(reads.index_reads, 1);
    let between = reader
        .read_at(StreamId(1), ArrivalTime(19))
        .unwrap()
        .unwrap();
    assert_eq!(first.0, between.0);
    let boundary = reader
        .read_at(StreamId(1), ArrivalTime(20))
        .unwrap()
        .unwrap();
    assert_ne!(first.0, boundary.0);
    assert_eq!(boundary.1.arrival_time, ArrivalTime(20));
    let last = reader
        .read_at(StreamId(1), ArrivalTime(30))
        .unwrap()
        .unwrap();
    assert_eq!(last.1.publish_time_ns, Some(29));
    assert_eq!(reader.diagnostics(), reads);
    assert_eq!(source.time_range(), before);
    assert!(source.read_through(ArrivalTime(30)).unwrap().is_empty());
    assert!(matches!(
        reader.read_at_cancellable(StreamId(1), ArrivalTime(10), || true),
        Err(McapSourceError::QueryCancelled)
    ));
}
#[test]
fn empty_ros_messages_are_skipped_in_nested_inspection() {
    let schema = InspectorSchema::for_stream(&stream(
        "demo/Empty marker\nuint8 value\n===\nMSG: demo/Empty\n",
    ))
    .unwrap();
    let model = InspectorMessage::new(&schema, &raw(vec![0, 1, 0, 0, 0, 42], 10)).unwrap();
    let root = model.page(&model.root, None).unwrap();
    assert!(container(&root.fields[0].value).is_empty());
    assert!(matches!(
        root.fields[1].value,
        InspectorValue::Scalar(InspectorScalar::Unsigned(42))
    ));
}

#[test]
fn overlapping_chunk_time_ranges_and_equal_time_ties_return_the_actual_latest_record() {
    let mut output = Cursor::new(Vec::new());
    {
        let mut writer =
            mcap::Writer::with_options(&mut output, mcap::WriteOptions::new().compression(None))
                .unwrap();
        let schema = writer
            .add_schema("demo/msg/A", "ros2msg", b"uint32 value\n")
            .unwrap();
        let channel = writer
            .add_channel(schema, "/a", "cdr", &BTreeMap::new())
            .unwrap();
        for (sequence, time) in [10u64, 100, 20, 50].into_iter().enumerate() {
            if sequence == 2 {
                writer.flush().unwrap();
            }
            let mut cdr = Cdr::new(false);
            cdr.u32(sequence as u32);
            writer
                .write_to_known_channel(
                    &mcap::records::MessageHeader {
                        channel_id: channel,
                        sequence: sequence as u32,
                        log_time: time,
                        publish_time: time,
                    },
                    &cdr.bytes,
                )
                .unwrap();
        }
        writer.finish().unwrap();
    }
    let source = McapSource::from_bytes(output.into_inner().into()).unwrap();
    assert_eq!(
        source
            .latest_before(&[StreamId(1)], ArrivalTime(110))
            .unwrap()
            .messages[0]
            .arrival_time,
        ArrivalTime(100)
    );
    let mut reader = LatestMessageReader::new(source);
    assert_eq!(
        reader
            .read_at(StreamId(1), ArrivalTime(110))
            .unwrap()
            .unwrap()
            .1
            .arrival_time,
        ArrivalTime(100)
    );
    assert_eq!(
        reader
            .read_at(StreamId(1), ArrivalTime(70))
            .unwrap()
            .unwrap()
            .1
            .arrival_time,
        ArrivalTime(50)
    );
}
#[test]
fn a_large_variable_sequence_at_the_last_field_is_paged_without_whole_sequence_traversal() {
    let schema = InspectorSchema::for_stream(&stream(
        "demo/Entry[] entries\n===\nMSG: demo/Entry\nstring text\n",
    ))
    .unwrap();
    let mut cdr = Cdr::new(false);
    cdr.u32(200000);
    for _ in 0..200000 {
        cdr.text("entry");
    }
    let model = InspectorMessage::new(&schema, &raw(cdr.bytes, 10)).unwrap();
    let root = model.page(&model.root, None).unwrap();
    let entries = container(&root.fields[0].value);
    assert_eq!(entries.len(), 200000);
    let page = model.page(entries, None).unwrap();
    assert_eq!(page.fields.len(), 64);
    let text = model.page(container(&page.fields[0].value), None).unwrap();
    assert!(
        matches!(&text.fields[0].value,InspectorValue::Scalar(InspectorScalar::Text {preview,..}) if preview=="entry")
    );
}

#[test]
fn repeated_equal_log_times_do_not_force_a_scan_of_all_message_indexes() {
    let mut output = Cursor::new(Vec::new());
    {
        let mut writer =
            mcap::Writer::with_options(&mut output, mcap::WriteOptions::new().compression(None))
                .unwrap();
        let schema = writer
            .add_schema("demo/msg/A", "ros2msg", b"uint32 value\n")
            .unwrap();
        let channel = writer
            .add_channel(schema, "/a", "cdr", &BTreeMap::new())
            .unwrap();
        for sequence in 0..100 {
            let mut cdr = Cdr::new(false);
            cdr.u32(sequence);
            writer
                .write_to_known_channel(
                    &mcap::records::MessageHeader {
                        channel_id: channel,
                        sequence,
                        log_time: 10,
                        publish_time: 0,
                    },
                    &cdr.bytes,
                )
                .unwrap();
            writer.flush().unwrap();
        }
        writer.finish().unwrap();
    }
    let mut reader =
        LatestMessageReader::new(McapSource::from_bytes(output.into_inner().into()).unwrap());
    let message = reader
        .read_at(StreamId(1), ArrivalTime(10))
        .unwrap()
        .unwrap()
        .1;
    assert_eq!(message.sequence, Some(99));
    assert_eq!(reader.diagnostics().index_reads, 1);
    assert_eq!(reader.diagnostics().chunks_read, 1);
}
