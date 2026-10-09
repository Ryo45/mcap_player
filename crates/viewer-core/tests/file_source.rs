#![cfg(not(target_arch = "wasm32"))]
use std::{collections::BTreeMap, error::Error, io::Cursor, path::PathBuf};
use viewer_core::*;
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn fixture() -> (Fixture, Vec<u8>) {
    let mut output = Cursor::new(Vec::new());
    {
        let mut writer = mcap::Writer::with_options(
            &mut output,
            mcap::WriteOptions::new()
                .compression(None)
                .calculate_chunk_crcs(false),
        )
        .unwrap();
        let schema = writer
            .add_schema("demo/msg/Scalar", "ros2msg", b"uint32 value\n")
            .unwrap();
        let channel = writer
            .add_channel(schema, "/scalar", "cdr", &BTreeMap::new())
            .unwrap();
        for time in [10u64, 20, 30] {
            if time == 30 {
                writer.flush().unwrap();
            }
            let mut payload = vec![0, 1, 0, 0];
            payload.extend((time as u32).to_le_bytes());
            writer
                .write_to_known_channel(
                    &mcap::records::MessageHeader {
                        channel_id: channel,
                        sequence: time as u32,
                        log_time: time,
                        publish_time: time - 1,
                    },
                    &payload,
                )
                .unwrap();
        }
        writer.finish().unwrap();
    }
    let bytes = output.into_inner();
    let path = std::env::temp_dir().join(format!(
        "mcap-file-source-{}-{:?}.mcap",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&path, &bytes).unwrap();
    (Fixture(path), bytes)
}
#[test]
fn range_reader_and_owned_bytes_have_identical_catalog_and_exact_messages() {
    let (file, bytes) = fixture();
    let mut native = McapSource::open_file(&file.0).unwrap();
    let mut memory = McapSource::from_bytes(bytes.into()).unwrap();
    assert_eq!(native.catalog().streams, memory.catalog().streams);
    assert_eq!(native.time_range(), memory.time_range());
    assert_eq!(
        native.summary_identity().unwrap(),
        memory.summary_identity().unwrap()
    );
    for time in [10, 20, 30] {
        assert_eq!(
            native.read_through(ArrivalTime(time)).unwrap(),
            memory.read_through(ArrivalTime(time)).unwrap()
        );
        assert_eq!(
            native
                .latest_before(&[StreamId(1)], ArrivalTime(time))
                .unwrap()
                .messages,
            memory
                .latest_before(&[StreamId(1)], ArrivalTime(time))
                .unwrap()
                .messages
        );
    }
    assert_eq!(native.time_range().end_exclusive, ArrivalTime(31));
}
#[test]
fn truncation_cannot_invalidate_delivered_payload_and_seek_does_not_commit() {
    let (file, _) = fixture();
    let mut playback = McapPlayback::open_file(&file.0).unwrap();
    let source = playback.query_source();
    let raw = source
        .latest_before(&[StreamId(1)], ArrivalTime(10))
        .unwrap()
        .messages
        .remove(0);
    let retained = raw.payload.clone();
    let before = playback.clock().view();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&file.0)
        .unwrap()
        .set_len(0)
        .unwrap();
    let plan = RestorePlanner::new(playback.catalog())
        .plan(
            ArrivalTime(20),
            [RestoreInput {
                stream_id: StreamId(1),
                semantics: RestoreSemantics::LatestBefore,
            }],
        )
        .unwrap();
    let mut applied = false;
    let result = playback.seek_with(&plan, |_, _| {
        applied = true;
        Ok::<(), std::io::Error>(())
    });
    assert!(matches!(
        result,
        Err(McapSeekError::Source(McapOpenError::SourceChanged))
    ));
    assert!(!applied);
    assert_eq!(playback.clock().view(), before);
    assert_eq!(retained.as_ref(), &[0, 1, 0, 0, 10, 0, 0, 0]);
    assert!(matches!(
        source.query_numeric_current(StreamId(1), "value", ArrivalTime(20)),
        Err(NumericPlotError::Source(McapOpenError::SourceChanged))
    ));
}
#[test]
fn multi_chunk_failure_keeps_forward_position_at_last_successful_read() {
    let (file, mut bytes) = fixture();
    let summary = mcap::Summary::read(&bytes).unwrap().unwrap();
    let offset = summary.chunk_indexes[1].compressed_data_offset().unwrap() as usize;
    bytes[offset + 1..offset + 9].copy_from_slice(&u64::MAX.to_le_bytes());
    std::fs::write(&file.0, bytes).unwrap();
    let mut source = McapSource::open_file(&file.0).unwrap();
    assert_eq!(source.read_through(ArrivalTime(10)).unwrap().len(), 1);
    assert!(source.read_through(ArrivalTime(30)).is_err());
    let retry = source.read_through(ArrivalTime(20)).unwrap();
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].arrival_time, ArrivalTime(20));
}
#[test]
fn io_error_kind_is_available_through_source_chain() {
    let (file, _) = fixture();
    std::fs::remove_file(&file.0).unwrap();
    let error = match McapSource::open_file(&file.0) {
        Err(error) => error,
        Ok(_) => panic!("deleted file opened"),
    };
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .kind(),
        std::io::ErrorKind::NotFound
    );
}
#[cfg(unix)]
#[test]
fn query_readers_keep_the_open_file_identity_when_its_path_is_replaced() {
    let (file, _) = fixture();
    let source = McapSource::open_file(&file.0).unwrap();
    let query = source.fork_reader();
    let replacement = file.0.with_extension("replacement");
    std::fs::write(&replacement, b"a different file").unwrap();
    std::fs::rename(&replacement, &file.0).unwrap();
    let current = query
        .query_numeric_current(StreamId(1), "value", ArrivalTime(20))
        .unwrap()
        .unwrap();
    assert_eq!(current.value, 20.0);
}

#[test]
fn planning_and_window_errors_keep_structured_values() {
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_camera_topic("/missing");
    let bindings = WorkspaceBindings {
        path_topic: "/path".into(),
        odometry_topic: "/odom".into(),
        point_cloud_topic: "/scan".into(),
        dynamic_tf_topic: "/tf".into(),
        static_tf_topic: "/tf_static".into(),
    };
    let error = SessionPlan::build(
        &SourceCatalog::default(),
        "/missing",
        &requirements,
        &bindings,
    )
    .unwrap_err();
    assert!(
        matches!(error,SessionPlanError::MissingRequiredStream {feature:"Camera",ref topic,..} if topic=="/missing")
    );
    let mut store = MemoryWindowStore::new(ArrivalTime(0), 256).unwrap();
    let window = SerializedWindow::new(
        DataWindowTimeRange::new(ArrivalTime(10), ArrivalTime(20)).unwrap(),
        vec![],
        0,
    )
    .unwrap();
    assert!(matches!(
        store.insert(window),
        Err(DataWindowError::NonContiguousWindow {
            actual_start: ArrivalTime(10),
            expected_start: ArrivalTime(0)
        })
    ));
}

#[cfg(unix)]
#[test]
fn opening_and_seeking_a_gigabyte_sparse_recording_reads_only_indexed_ranges() {
    use std::io::{Seek, SeekFrom, Write};
    let (file, bytes) = fixture();
    let footer = mcap::read::footer(&bytes).unwrap();
    let data_end = footer.summary_start as usize - 13;
    assert_eq!(bytes[data_end], mcap::records::op::DATA_END);
    let gap = 1024u64 * 1024 * 1024;
    let shift = gap + 9;
    let mut suffix = bytes[data_end..].to_vec();
    suffix[9..13].fill(0); // Data-section CRC is optional; the opaque private record changes it.
    let mut cursor = 13;
    while cursor < suffix.len() - 8 {
        let length =
            u64::from_le_bytes(suffix[cursor + 1..cursor + 9].try_into().unwrap()) as usize;
        if suffix[cursor] == mcap::records::op::SUMMARY_OFFSET {
            let start = u64::from_le_bytes(suffix[cursor + 10..cursor + 18].try_into().unwrap());
            suffix[cursor + 10..cursor + 18].copy_from_slice(&(start + shift).to_le_bytes());
        }
        if suffix[cursor] == mcap::records::op::FOOTER {
            suffix[cursor + 9..cursor + 17]
                .copy_from_slice(&(footer.summary_start + shift).to_le_bytes());
            suffix[cursor + 17..cursor + 25]
                .copy_from_slice(&(footer.summary_offset_start + shift).to_le_bytes());
            suffix[cursor + 25..cursor + 29].fill(0);
        }
        cursor += 9 + length;
    }
    {
        let mut sparse = std::fs::File::create(&file.0).unwrap();
        sparse.set_len(bytes.len() as u64 + shift).unwrap();
        sparse.write_all(&bytes[..data_end]).unwrap();
        sparse.write_all(&[0x80]).unwrap();
        sparse.write_all(&gap.to_le_bytes()).unwrap();
        sparse
            .seek(SeekFrom::Start(data_end as u64 + shift))
            .unwrap();
        sparse.write_all(&suffix).unwrap();
    }
    let source = McapSource::open_file(&file.0).unwrap();
    assert!(source.summary_identity().unwrap().file_size > gap);
    let current = source
        .query_numeric_current(StreamId(1), "value", ArrivalTime(30))
        .unwrap()
        .unwrap();
    assert_eq!(current.value, 30.0);
}
