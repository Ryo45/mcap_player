use serde_json::Value;
use std::{
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use viewer_core::{InspectorMessage, InspectorScalar, InspectorSchema, InspectorValue};
use viewer_mcap::FileMessageStream;
use viewer_mcap::McapSource;

fn command(args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_performance-probe"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn fixture(encoding: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "mcap-perf-{}-{nonce}-{encoding}.mcap",
        std::process::id()
    ));
    command(&[
        "generate",
        path.to_str().unwrap(),
        "zstd",
        "1048576",
        "grouped",
        "1",
        encoding,
    ]);
    path
}

#[test]
fn probe_counts_real_file_reads_and_selected_feature_decodes() {
    let path = fixture("jpeg");
    let scan = command(&["run", path.to_str().unwrap(), "scan", "indexed", "0"]);
    let all = command(&["run", path.to_str().unwrap(), "all", "indexed", "0"]);
    assert_eq!(scan["delivered_messages"], 10);
    assert_eq!(all["delivered_messages"], 30);
    assert_eq!(scan["feature_decoded"], 10);
    assert_eq!(scan["jpeg_decodes"], 0);
    assert_eq!(all["jpeg_decodes"], 4);
    assert!(scan["open_io"]["requested_bytes"].as_u64().unwrap() > 0);
    assert!(
        scan["io"]["requested_bytes"].as_u64().unwrap()
            < all["io"]["requested_bytes"].as_u64().unwrap()
    );
    assert!(
        scan["io"]["expanded_bytes"].as_u64().unwrap() >= scan["payload_bytes"].as_u64().unwrap()
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn grouped_ros_fixture_delivers_exactly_once_in_order_at_100_ms_ticks() {
    let path = fixture("jpeg");
    let mut source = McapSource::open_file(&path).unwrap();
    let streams = source
        .catalog()
        .streams
        .iter()
        .map(|s| s.id)
        .collect::<Vec<_>>();
    source.select_streams(streams).unwrap();
    let start = source.time_range().start.0;
    let mut all = Vec::new();
    for tick in 0..10 {
        let end = start + (tick + 1) * 100_000_000;
        let messages = source
            .read_through(viewer_core::ArrivalTime(end - 1))
            .unwrap();
        assert!(
            messages
                .iter()
                .all(|m| m.arrival_time.0 >= start + tick * 100_000_000 && m.arrival_time.0 < end)
        );
        all.extend(messages);
    }
    assert_eq!(all.len(), 30);
    assert!(
        all.windows(2)
            .all(|pair| (pair[0].arrival_time, pair[0].stream_id.0)
                <= (pair[1].arrival_time, pair[1].stream_id.0))
    );
    let replay = command(&["run", path.to_str().unwrap(), "all", "forward", "0", "100"]);
    assert_eq!(replay["delivered_messages"], 30);
    assert_eq!(replay["unknown_messages"], 0);
    assert_eq!(replay["feature_decoded"], 30);
    assert_eq!(replay["jpeg_decodes"], 20);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn rechunk_preserves_message_payloads_and_timestamps() {
    let input = fixture("jpeg");
    let output = input.with_extension("rechunk.mcap");
    command(&[
        "rechunk",
        input.to_str().unwrap(),
        output.to_str().unwrap(),
        "none",
        "536870912",
    ]);
    let messages = |path: &PathBuf| {
        FileMessageStream::open(path)
            .unwrap()
            .map(|m| {
                let m = m.unwrap();
                (
                    m.channel.topic.clone(),
                    m.log_time,
                    m.publish_time,
                    m.sequence,
                    m.data.into_owned(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(messages(&input), messages(&output));
    std::fs::remove_file(input).unwrap();
    std::fs::remove_file(output).unwrap();
}

#[test]
fn eight_cameras_at_ten_hz_use_real_rgba_decode_and_keep_latest_pixels() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("mcap-rate-{nonce}.mcap"));
    let stats = command(&[
        "generate",
        path.to_str().unwrap(),
        "none",
        "65536",
        "mixed",
        "1",
        "jpeg",
        "32",
        "24",
        "2",
        "10",
        "8",
    ]);
    assert_eq!(stats["camera_hz"], 10);
    assert_eq!(stats["cameras"], 8);
    let result = command(&[
        "run",
        path.to_str().unwrap(),
        "all",
        "forward",
        "0",
        "50",
        "rgba",
    ]);
    assert_eq!(result["delivered_messages"], 90);
    assert_eq!(result["unknown_messages"], 0);
    assert_eq!(result["pixel_format"], "rgba");
    assert_eq!(result["retained_pixel_bytes"], 8 * 32 * 24 * 4);
    assert!(result["jpeg_decodes"].as_u64().unwrap() < 80);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn raw_images_are_valid_ros_cdr_and_use_the_shared_camera_feature() {
    let path = fixture("raw");
    let mut source = McapSource::open_file(&path).unwrap();
    let image = source
        .catalog()
        .streams
        .iter()
        .find(|s| s.schema == "sensor_msgs/msg/Image")
        .unwrap()
        .clone();
    let schema = InspectorSchema::for_stream(&image).unwrap();
    source.select_streams([image.id]).unwrap();
    let frames = source
        .read_through(source.time_range().end_inclusive())
        .unwrap();
    assert_eq!(frames.len(), 5);
    let message = InspectorMessage::new(&schema, &frames[0]).unwrap();
    assert_eq!(message.header_stamp.unwrap().seconds, 1_735_689_600);
    let page = message.page(&message.root, None).unwrap();
    for (name, expected) in [
        ("height", 240),
        ("width", 320),
        ("step", 960),
        ("is_bigendian", 0),
    ] {
        let field = page.fields.iter().find(|f| f.name == name).unwrap();
        assert!(
            matches!(field.value, InspectorValue::Scalar(InspectorScalar::Unsigned(v)) if v == expected)
        );
    }
    let data = page.fields.iter().find(|f| f.name == "data").unwrap();
    assert!(matches!(&data.value, InspectorValue::Container(c) if c.len() == 320*240*3));
    let all = command(&["run", path.to_str().unwrap(), "all", "forward", "0"]);
    assert_eq!(all["unknown_messages"], 0);
    // The 200 ms probe tick coalesces the four 5 Hz Camera streams.
    assert_eq!(all["feature_decoded"], 14);
    assert_eq!(all["jpeg_decodes"], 0);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn storage_variants_keep_payloads_and_timestamps_and_unchunked_is_explicit() {
    let source = fixture("raw");
    let output = source.with_extension("direct.mcap");
    let facts = command(&[
        "storage",
        source.to_str().unwrap(),
        output.to_str().unwrap(),
        "none",
        "0",
        "0",
    ]);
    assert_eq!(facts["files"][0]["chunks"], 0);
    assert!(
        McapSource::open_file(&output)
            .err()
            .unwrap()
            .to_string()
            .contains("Chunk")
    );
    let original = FileMessageStream::open(&source)
        .unwrap()
        .map(|m| {
            let m = m.unwrap();
            (
                m.channel.topic.clone(),
                m.log_time,
                m.sequence,
                m.data.to_vec(),
            )
        })
        .collect::<Vec<_>>();
    let repacked = FileMessageStream::open(&output)
        .unwrap()
        .map(|m| {
            let m = m.unwrap();
            (
                m.channel.topic.clone(),
                m.log_time,
                m.sequence,
                m.data.to_vec(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(original, repacked);
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(output).unwrap();
}

#[test]
fn storage_split_compression_preserves_every_message() {
    let input = fixture("raw");
    let source = input.with_extension("two-seconds.mcap");
    command(&[
        "generate",
        source.to_str().unwrap(),
        "none",
        "1048576",
        "mixed",
        "2",
        "raw",
    ]);
    let expected = FileMessageStream::open(&source)
        .unwrap()
        .map(|m| m.unwrap())
        .collect::<Vec<_>>();
    for codec in ["none", "zstd", "lz4"] {
        let directory = input.with_extension(format!("{codec}-parts"));
        let facts = command(&[
            "storage",
            source.to_str().unwrap(),
            directory.to_str().unwrap(),
            codec,
            "1048576",
            "1",
        ]);
        assert_eq!(facts["files"].as_array().unwrap().len(), 2);
        let actual = facts["files"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|part| {
                FileMessageStream::open(std::path::Path::new(part["path"].as_str().unwrap()))
                    .unwrap()
                    .map(|m| m.unwrap())
            })
            .collect::<Vec<_>>();
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(actual.channel, expected.channel);
            assert_eq!(actual.log_time, expected.log_time);
            assert_eq!(actual.publish_time, expected.publish_time);
            assert_eq!(actual.sequence, expected.sequence);
            assert_eq!(actual.data, expected.data);
        }
        let source = McapSource::open_file(&directory).unwrap();
        assert_eq!(source.time_range().start.0, expected[0].log_time as i64);
        for part in facts["files"].as_array().unwrap() {
            std::fs::remove_file(part["path"].as_str().unwrap()).unwrap();
        }
        std::fs::remove_dir(directory).unwrap();
    }
    std::fs::remove_file(input).unwrap();
    std::fs::remove_file(source).unwrap();
}

#[test]
fn repeated_full_seek_reuses_chunks_without_changing_the_restored_frame() {
    let original = fixture("raw");
    let path = original.with_extension("seek-revisit.mcap");
    command(&[
        "generate",
        path.to_str().unwrap(),
        "zstd",
        "4096",
        "mixed",
        "2",
        "raw",
        "64",
        "36",
        "32",
        "10",
        "1",
    ]);
    let result = command(&["seek-workload", path.to_str().unwrap(), "same", "6"]);
    let samples = result["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 6);
    assert!(samples[0]["chunks_expanded"].as_u64().unwrap() > 0);
    assert!(samples[1]["chunks_expanded"].as_u64().unwrap() > 0);
    assert!(samples[2..].iter().all(|row| row["chunks_expanded"] == 0));
    assert!(result["io"]["seek_cache_backing_bytes"].as_u64().unwrap() > 0);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(original).unwrap();
}
