use std::{fs, path::PathBuf};
use viewer_core::{
    ArrivalTime, CameraId, CameraState, DomainUpdate, McapSource, PipelineSet, StreamBinding,
};
use viewer_renderer::decode_jpeg;

fn fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/camera-jpeg/camera_front_3s.mcap");
    fs::read(path).expect("canonical camera fixture")
}

#[test]
fn fixture_has_30_decodable_frames_and_distinct_time_domains() {
    let mut source = McapSource::new(fixture()).unwrap();
    let descriptor = source
        .catalog()
        .by_topic("/camera/front/image/compressed")
        .unwrap();
    let mut pipelines = PipelineSet::new(
        &source.catalog().streams,
        &[(descriptor.id, StreamBinding::Camera(CameraId(0)))],
    );
    let (_, end) = source.time_range();
    let mut updates = vec![];
    for message in source.read_until(end).unwrap() {
        pipelines.decode(message.raw, &mut updates);
    }
    assert_eq!(updates.len(), 30);
    let mut state = CameraState::default();
    for (index, update) in updates.into_iter().enumerate() {
        let DomainUpdate::Camera(frame) = update else {
            panic!("camera-only fixture produced a non-camera update");
        };
        assert_ne!(frame.measurement_time.0, frame.arrival_time.0);
        let decoded = decode_jpeg(&frame.jpeg).unwrap();
        assert_eq!((decoded.width, decoded.height), (320, 240));
        assert!(
            state.apply(0, frame),
            "frame {index} must advance arrival state"
        );
    }
    assert_eq!(pipelines.counters().decoded, 30);
    assert_eq!(state.latest().unwrap().arrival_time, end);
}

#[test]
fn cold_seek_returns_only_cursor_or_newer_frames() {
    let mut source = McapSource::new(fixture()).unwrap();
    let (start, _) = source.time_range();
    let cursor = ArrivalTime(start.0 + 1_500_000_000);
    source.seek(cursor).unwrap();
    let messages = source
        .read_until(ArrivalTime(cursor.0 + 200_000_000))
        .unwrap();
    assert!(!messages.is_empty());
    assert!(
        messages
            .iter()
            .all(|message| message.raw.arrival_time >= cursor)
    );
    assert!(
        messages
            .iter()
            .all(|message| message.raw.arrival_time <= ArrivalTime(cursor.0 + 200_000_000))
    );
}

#[test]
fn malformed_message_does_not_stop_the_next_frame() {
    let mut source = McapSource::new(fixture()).unwrap();
    let descriptor = source
        .catalog()
        .by_topic("/camera/front/image/compressed")
        .unwrap();
    let mut pipelines = PipelineSet::new(
        &source.catalog().streams,
        &[(descriptor.id, StreamBinding::Camera(CameraId(0)))],
    );
    let (_, end) = source.time_range();
    let messages = source.read_until(end).unwrap();
    let mut bad = messages[0].raw.clone();
    bad.payload.truncate(7);
    let mut output = vec![];
    pipelines.decode(bad, &mut output);
    pipelines.decode(messages[1].raw.clone(), &mut output);
    assert_eq!(pipelines.counters().errors, 1);
    assert_eq!(pipelines.counters().decoded, 1);
    assert_eq!(output.len(), 1);
}
