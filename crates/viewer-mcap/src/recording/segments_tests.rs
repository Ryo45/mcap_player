use super::{McapSource, McapSourceError};
use crate::{LatestMessageReader, McapPlayback, QueryLimits, RangeQuery};
use mcap::{WriteOptions, Writer, records::MessageHeader};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use viewer_core::{ArrivalTime, DataWindowTimeRange, RestorePlan, StreamId};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mcap-parts-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn part(&self, name: &str, padding: bool, messages: &[(u64, u32)], indexed: bool) -> PathBuf {
        let path = self.0.join(name);
        let file = std::fs::File::create(&path).unwrap();
        let mut writer = Writer::with_options(
            file,
            WriteOptions::new()
                .compression(None)
                .chunk_size(Some(128))
                .emit_summary_records(indexed),
        )
        .unwrap();
        if padding {
            let schema = writer
                .add_schema("Other", "ros2msg", b"uint8 data")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/noise", "cdr", &BTreeMap::new())
                .unwrap();
            writer
                .write_to_known_channel(
                    &MessageHeader {
                        channel_id: channel,
                        sequence: 0,
                        log_time: messages[0].0,
                        publish_time: 0,
                    },
                    &vec![42; 2 * 1024 * 1024],
                )
                .unwrap();
            writer.flush().unwrap();
        }
        let schema = writer
            .add_schema(
                "sensor_msgs/msg/CompressedImage",
                "ros2msg",
                b"camera schema",
            )
            .unwrap();
        let camera = writer
            .add_channel(schema, "/camera", "cdr", &BTreeMap::new())
            .unwrap();
        let tf_schema = writer
            .add_schema("tf2_msgs/msg/TFMessage", "ros2msg", b"transform schema")
            .unwrap();
        let tf = writer
            .add_channel(tf_schema, "/tf_static", "cdr", &BTreeMap::new())
            .unwrap();
        for &(time, sequence) in messages {
            writer
                .write_to_known_channel(
                    &MessageHeader {
                        channel_id: camera,
                        sequence,
                        log_time: time,
                        publish_time: time - 1,
                    },
                    &sequence.to_le_bytes(),
                )
                .unwrap();
        }
        writer
            .write_to_known_channel(
                &MessageHeader {
                    channel_id: tf,
                    sequence: 0,
                    log_time: messages[0].0,
                    publish_time: 0,
                },
                name.as_bytes(),
            )
            .unwrap();
        writer.finish().unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn stream(source: &McapSource, topic: &str) -> StreamId {
    source
        .catalog()
        .streams
        .iter()
        .find(|s| s.topic == topic)
        .unwrap()
        .id
}
fn sequence(message: &viewer_core::RawMessage) -> u32 {
    u32::from_le_bytes(message.payload.as_ref().try_into().unwrap())
}

#[test]
fn parts_merge_ids_ties_ranges_and_exact_predecessors_without_scanning_payloads() {
    let f = Fixture::new();
    let early = f.part("z-early.mcap", false, &[(100, 1), (200, 2)], true);
    let late = f.part("a-late.mcap", true, &[(200, 3), (400, 4)], true);
    // A derived sidecar must neither extend the timeline nor be parsed as a recording part.
    std::fs::write(f.0.join("preview.mcap"), b"not a recording part").unwrap();
    let mut source = McapSource::open_file(&f.0).unwrap();
    assert!(
        source.read_diagnostics().file_read_bytes < 128 * 1024,
        "open must read metadata only"
    );
    let camera = stream(&source, "/camera");
    let health = crate::query_topic_health(
        &source,
        crate::TopicHealthRequirement { window_seconds: 1 },
        ArrivalTime(400),
        || false,
    )
    .unwrap();
    let health = health.iter().find(|h| h.topic == "/camera").unwrap();
    assert!(health.error.is_none());
    assert_eq!(health.sample_count, 4);
    assert_eq!(health.latest_arrival, Some(ArrivalTime(400)));
    assert_eq!(
        source.read_diagnostics().chunks_expanded,
        0,
        "health must use indexes, not camera/noise payloads"
    );
    assert_eq!(
        source
            .catalog()
            .streams
            .iter()
            .find(|s| s.id == camera)
            .unwrap()
            .timing
            .message_count,
        Some(4)
    );
    assert_eq!(source.time_range().start, ArrivalTime(100));
    assert_eq!(source.time_range().end_exclusive, ArrivalTime(401));
    source.select_streams([camera]).unwrap();
    let messages = source.read_through(ArrivalTime(400)).unwrap();
    assert_eq!(
        messages.iter().map(sequence).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    assert!(messages.iter().all(|m| m.stream_id == camera));
    assert_eq!(messages[2].publish_time_ns, Some(199));
    assert!(
        source.read_diagnostics().file_read_bytes < 256 * 1024,
        "unselected noise chunks must stay unread"
    );
    let query = RangeQuery {
        streams: vec![camera],
        range: DataWindowTimeRange::new(ArrivalTime(200), ArrivalTime(400)).unwrap(),
        limits: QueryLimits::new(10, 1024).unwrap(),
    };
    let rows = source.query_range(&query).unwrap();
    assert_eq!(
        source
            .indexed_range(&[camera], query.range)
            .unwrap()
            .diagnostics
            .per_message_copied_bytes,
        0
    );
    assert!(rows.complete);
    assert_eq!(
        rows.messages.iter().map(sequence).collect::<Vec<_>>(),
        [2, 3]
    );
    let mut reader = LatestMessageReader::new(source.fork_reader());
    for (time, expected) in [(200, 3), (199, 1), (400, 4), (300, 3)] {
        let (_, message) = reader.read_at(camera, ArrivalTime(time)).unwrap().unwrap();
        assert_eq!(sequence(&message), expected);
    }
    source.seek(ArrivalTime(201)).unwrap();
    assert_eq!(
        source
            .read_through(ArrivalTime(400))
            .unwrap()
            .iter()
            .map(sequence)
            .collect::<Vec<_>>(),
        [4]
    );
    let reordered = McapSource::open_files(&[late, early]).unwrap();
    assert_eq!(
        source.summary_identity().unwrap(),
        reordered.summary_identity().unwrap()
    );
}

#[test]
fn corrupt_later_chunk_rolls_back_messages_source_position_and_cursor() {
    let f = Fixture::new();
    f.part("early.mcap", false, &[(100, 1), (200, 2)], true);
    let late = f.part("late.mcap", false, &[(300, 3), (400, 4)], true);
    let mut bytes = std::fs::read(&late).unwrap();
    let summary = mcap::Summary::read(&bytes).unwrap().unwrap();
    let camera = summary
        .channels
        .values()
        .find(|c| c.topic == "/camera")
        .unwrap()
        .id;
    let chunk = summary
        .chunk_indexes
        .iter()
        .find(|c| c.message_index_offsets.contains_key(&camera))
        .unwrap();
    let offset = chunk.compressed_data_offset().unwrap() as usize;
    bytes[offset + chunk.compressed_size as usize / 2] ^= 1;
    std::fs::write(&late, bytes).unwrap();
    let mut playback = McapPlayback::open_file(&f.0).unwrap();
    let camera = stream(playback.recording_source(), "/camera");
    playback.select_streams([camera]).unwrap();
    playback.clock_mut().toggle();
    let committed = playback.clock().cursor();
    let mut called = false;
    assert!(
        playback
            .tick(std::time::Duration::from_nanos(1000), |_, _| called = true)
            .is_err()
    );
    assert!(!called);
    assert_eq!(playback.clock().cursor(), committed);
    let mut rows = Vec::new();
    playback
        .tick(std::time::Duration::ZERO, |_, messages| rows = messages)
        .unwrap();
    assert_eq!(rows.iter().map(sequence).collect::<Vec<_>>(), [1]);
}

#[test]
fn same_topic_with_changed_schema_is_rejected() {
    let f = Fixture::new();
    f.part("early.mcap", false, &[(100, 1)], true);
    let mut writer = Writer::new(std::fs::File::create(f.0.join("late.mcap")).unwrap()).unwrap();
    let schema = writer
        .add_schema(
            "sensor_msgs/msg/Image",
            "ros2msg",
            b"different image schema",
        )
        .unwrap();
    let channel = writer
        .add_channel(schema, "/camera", "cdr", &BTreeMap::new())
        .unwrap();
    writer
        .write_to_known_channel(
            &MessageHeader {
                channel_id: channel,
                sequence: 1,
                log_time: 200,
                publish_time: 200,
            },
            &[1],
        )
        .unwrap();
    writer.finish().unwrap();
    let error = McapSource::open_file(&f.0).err().unwrap();
    assert!(
        error
            .to_string()
            .contains("changes schema or message encoding")
    );
}

#[test]
fn real_camera_and_tf_restore_across_parts_in_both_seek_directions() {
    use viewer_core::{
        FeatureRuntime, PlaybackRequirements, RestorePlanner, SessionPlan, WorkspaceBindings,
    };
    let f = Fixture::new();
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap"),
    )
    .unwrap();
    let summary = mcap::Summary::read(&bytes).unwrap().unwrap();
    let start = summary.stats.as_ref().unwrap().message_start_time;
    let boundary = start + 3_000_000_000;
    let mut writers = ["early.mcap", "late.mcap"].map(|name| {
        Writer::with_options(
            std::fs::File::create(f.0.join(name)).unwrap(),
            WriteOptions::new().compression(None),
        )
        .unwrap()
    });
    for message in mcap::MessageStream::new(&bytes).unwrap() {
        let message = message.unwrap();
        writers[usize::from(message.log_time >= boundary)]
            .write(&message)
            .unwrap();
    }
    for writer in &mut writers {
        writer.finish().unwrap();
    }
    let mut playback = McapPlayback::open_file(&f.0).unwrap();
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_all_cameras();
    requirements.require_transforms();
    requirements.require_point_cloud();
    requirements.require_path();
    requirements.require_odometry();
    let bindings = WorkspaceBindings {
        path_topic: "/planning/path".into(),
        odometry_topic: "/odom".into(),
        point_cloud_topic: "/scan".into(),
        dynamic_tf_topic: "/tf".into(),
        static_tf_topic: "/tf_static".into(),
    };
    let plan = SessionPlan::build(
        playback.catalog(),
        "/camera/front/image/compressed",
        &requirements,
        &bindings,
    )
    .unwrap();
    playback.select_streams(plan.selected_stream_ids()).unwrap();
    let initial = RestorePlanner::new(playback.catalog())
        .plan(
            ArrivalTime((start + 5_000_000_000) as i64),
            plan.restore_inputs(),
        )
        .unwrap();
    playback.bootstrap_persistent(&initial.persistent).unwrap();
    let mut runtime = FeatureRuntime::new(&plan, true);
    for seconds in [4_u64, 1, 5] {
        let target = ArrivalTime((start + seconds * 1_000_000_000) as i64);
        let restore = RestorePlanner::new(playback.catalog())
            .plan(target, plan.restore_inputs())
            .unwrap();
        playback
            .seek_with(&restore, |target, rows| {
                runtime.restore_transactional(target, &rows)
            })
            .unwrap();
        assert_eq!(runtime.cameras().state().frames().count(), 7);
        assert_eq!(runtime.transforms().state().static_len(), 15);
        assert_eq!(runtime.transforms().state().dynamic_len(), 3);
        let cloud = runtime
            .point_cloud()
            .unwrap()
            .point_cloud()
            .latest()
            .unwrap();
        assert!(
            runtime
                .transforms()
                .state()
                .transform_points_at(
                    &cloud.frame_id,
                    "odom",
                    cloud.measurement_time,
                    &cloud.points
                )
                .is_some()
        );
        assert_eq!(runtime.counters().errors, 0);
        assert_eq!(playback.clock().cursor(), target);
    }
}

#[test]
fn seek_restores_persistent_data_from_previous_parts_and_failure_keeps_cursor() {
    let f = Fixture::new();
    f.part("early.mcap", false, &[(100, 1), (200, 2)], true);
    let late = f.part("late.mcap", true, &[(300, 3), (400, 4)], true);
    let mut playback = McapPlayback::open_file(&f.0).unwrap();
    let camera = stream(playback.recording_source(), "/camera");
    let tf = stream(playback.recording_source(), "/tf_static");
    playback.select_streams([camera, tf]).unwrap();
    playback.bootstrap_persistent(&[tf]).unwrap();
    let plan = RestorePlan {
        target: ArrivalTime(350),
        latest_before: vec![camera],
        histories: vec![],
        persistent: vec![tf],
    };
    playback
        .seek_with(&plan, |target, rows| {
            assert_eq!(target, ArrivalTime(350));
            assert_eq!(rows.iter().filter(|m| m.stream_id == tf).count(), 2);
            assert_eq!(
                sequence(rows.iter().find(|m| m.stream_id == camera).unwrap()),
                3
            );
            Ok::<(), std::convert::Infallible>(())
        })
        .unwrap();
    let mut next = Vec::new();
    playback.clock_mut().toggle();
    playback
        .tick(std::time::Duration::from_nanos(50), |_, rows| next = rows)
        .unwrap();
    assert_eq!(next.iter().map(sequence).collect::<Vec<_>>(), [4]);
    let committed = playback.clock().cursor();
    let before = playback.recording_source().fork_reader();
    // A modified later part invalidates the whole candidate, including queries into early parts.
    std::fs::OpenOptions::new()
        .write(true)
        .open(late)
        .unwrap()
        .set_len(10)
        .unwrap();
    let mut called = false;
    assert!(
        playback
            .seek_with(&plan, |_, _| {
                called = true;
                Ok::<(), std::convert::Infallible>(())
            })
            .is_err()
    );
    assert!(!called);
    assert_eq!(playback.clock().cursor(), committed);
    assert!(matches!(
        before.latest_before(&[camera], ArrivalTime(100)),
        Err(McapSourceError::SourceChanged)
    ));
}

#[test]
fn ambiguous_incompatible_and_unindexed_parts_fail_explicitly() {
    let f = Fixture::new();
    assert!(McapSource::open_file(&f.0).is_err());
    let a = f.part("a.mcap", false, &[(100, 1)], true);
    assert!(McapSource::open_files(&[a.clone(), a]).is_err());
    f.part("b.mcap", false, &[(200, 2)], false);
    let error = McapSource::open_file(&f.0).err().unwrap();
    assert!(matches!(
        error,
        McapSourceError::SummaryRequired | McapSourceError::ChunkIndexRequired
    ));
    assert!(error.to_string().contains("reindex or convert"));
    assert!(McapSource::open_files(&[]).is_err());
    assert!(McapSource::open_files(std::slice::from_ref(&f.0)).is_err());
}

#[test]
fn schema_only_part_does_not_extend_the_recording_to_time_zero() {
    let f = Fixture::new();
    let populated = f.part("messages.mcap", false, &[(100, 1), (200, 2)], true);
    let empty = f.0.join("empty.mcap");
    let mut writer = Writer::with_options(
        std::fs::File::create(&empty).unwrap(),
        WriteOptions::new().compression(None),
    )
    .unwrap();
    writer
        .add_schema("Unused", "ros2msg", b"uint8 unused")
        .unwrap();
    writer.flush().unwrap();
    writer.finish().unwrap();
    let summary = mcap::Summary::read(&std::fs::read(&empty).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(summary.stats.as_ref().unwrap().message_count, 0);
    assert!(
        !summary.chunk_indexes.is_empty(),
        "fixture must include a schema-only chunk"
    );
    for paths in [
        vec![empty.clone(), populated.clone()],
        vec![populated, empty.clone()],
    ] {
        let source = McapSource::open_files(&paths).unwrap();
        assert_eq!(source.time_range().start, ArrivalTime(100));
        assert_eq!(source.time_range().end_inclusive(), ArrivalTime(200));
        assert_eq!(source.summary().stats.as_ref().unwrap().message_count, 3);
    }
    let second_empty = f.0.join("second-empty.mcap");
    std::fs::copy(&empty, &second_empty).unwrap();
    let empty_source = McapSource::open_files(&[empty, second_empty]).unwrap();
    assert_eq!(empty_source.time_range().start, ArrivalTime(0));
    assert_eq!(empty_source.time_range().end_inclusive(), ArrivalTime(0));
}
