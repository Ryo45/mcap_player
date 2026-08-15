use std::{collections::BTreeMap, fs, path::PathBuf, time::Duration, time::Instant};
use viewer_core::{
    ArrivalTime, CameraController, CameraId, McapPlayback, McapSource, PlaybackRequirements,
    SessionPlan,
};
use viewer_renderer::decode_camera_frame;

fn fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/camera-jpeg/camera_front_3s.mcap");
    fs::read(path).expect("canonical camera fixture")
}

fn seven_camera_fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap");
    fs::read(path).expect("seven-camera fixture")
}

#[test]
fn fixture_has_30_decodable_frames_and_distinct_time_domains() {
    let mut source = McapSource::new(fixture()).unwrap();
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_camera_topic("/camera/front/image/compressed");
    let plan = SessionPlan::build(
        source.catalog(),
        "/camera/front/image/compressed",
        &requirements,
    )
    .unwrap();
    source.select_streams(plan.selected_stream_ids());
    let mut cameras = CameraController::new(&plan);
    let (_, end) = source.time_range();
    for message in source.read_until(end).unwrap() {
        assert!(cameras.admit(&message));
        cameras.advance(Duration::from_millis(100));
        let frame = cameras.state().latest_by_arrival().unwrap();
        assert_ne!(frame.measurement_time.0, frame.arrival_time.0);
        let decoded = decode_camera_frame(frame).unwrap();
        assert_eq!((decoded.width, decoded.height), (320, 240));
    }
    assert_eq!(cameras.counters().decoded, 30);
    assert_eq!(
        cameras.state().latest_by_arrival().unwrap().arrival_time,
        end
    );
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
            .all(|message| message.arrival_time >= cursor)
    );
    assert!(
        messages
            .iter()
            .all(|message| message.arrival_time <= ArrivalTime(cursor.0 + 200_000_000))
    );
}

#[test]
fn malformed_message_does_not_stop_the_next_frame() {
    let mut source = McapSource::new(fixture()).unwrap();
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_camera_topic("/camera/front/image/compressed");
    let plan = SessionPlan::build(
        source.catalog(),
        "/camera/front/image/compressed",
        &requirements,
    )
    .unwrap();
    source.select_streams(plan.selected_stream_ids());
    let mut cameras = CameraController::new(&plan);
    let (_, end) = source.time_range();
    let messages = source.read_until(end).unwrap();
    let mut bad = messages[0].clone();
    bad.payload.truncate(7);
    cameras.admit(&bad);
    cameras.advance(Duration::ZERO);
    cameras.admit(&messages[1]);
    cameras.advance(Duration::from_millis(100));
    assert_eq!(cameras.counters().errors, 1);
    assert_eq!(cameras.counters().decoded, 1);
}

#[test]
#[ignore = "manual release-mode performance diagnostic"]
fn seven_camera_display_policy_stays_within_the_decode_budget() {
    let mut playback = McapPlayback::new(seven_camera_fixture()).unwrap();
    let plan = SessionPlan::build(
        playback.catalog(),
        "/camera/front/image/compressed",
        &PlaybackRequirements::default(),
    )
    .unwrap();
    playback.select_streams(plan.selected_stream_ids());
    let mut cameras = CameraController::new(&plan);
    playback.clock_mut().toggle();
    let mut arrivals = BTreeMap::new();
    let mut decoded_by_camera = BTreeMap::<CameraId, u64>::new();
    let mut decode_time = Duration::ZERO;

    for _ in 0..250 {
        playback
            .tick(Duration::from_millis(20), |elapsed, messages| {
                for message in &messages {
                    cameras.admit(message);
                }
                cameras.advance(elapsed);
            })
            .unwrap();
        for (camera_id, frame) in cameras.state().frames() {
            if arrivals.get(camera_id) == Some(&frame.arrival_time) {
                continue;
            }
            let started = Instant::now();
            let image = decode_camera_frame(frame).unwrap();
            decode_time = decode_time.saturating_add(started.elapsed());
            assert_eq!((image.width, image.height), (320, 240));
            arrivals.insert(*camera_id, frame.arrival_time);
            *decoded_by_camera.entry(*camera_id).or_default() += 1;
        }
    }

    let focused = decoded_by_camera[&CameraId(0)];
    assert!((45..=51).contains(&focused));
    for camera_id in 1..7 {
        let frames = decoded_by_camera[&CameraId(camera_id)];
        assert!((22..=26).contains(&frames));
    }
    let decoded = decoded_by_camera.values().sum::<u64>();
    eprintln!(
        "seven-camera JPEG budget: {decoded} frames, {:.3} ms/frame, {:.1}% of one CPU over 5 s",
        decode_time.as_secs_f64() * 1_000.0 / decoded as f64,
        decode_time.as_secs_f64() / 5.0 * 100.0,
    );
}
