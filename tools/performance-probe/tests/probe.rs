use serde_json::Value;
use std::{
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use viewer_core::{InspectorMessage, InspectorScalar, InspectorSchema, InspectorValue};
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
fn raw_background_is_valid_ros_image_cdr_and_not_reported_as_decoded_camera() {
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
    assert_eq!(all["unknown_messages"], 20);
    assert_eq!(all["feature_decoded"], 10);
    assert_eq!(all["jpeg_decodes"], 0);
    std::fs::remove_file(path).unwrap();
}
