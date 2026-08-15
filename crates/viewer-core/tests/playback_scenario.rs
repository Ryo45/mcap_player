use mcap::{WriteOptions, Writer, records::MessageHeader};
use std::{
    collections::{BTreeMap, HashMap},
    io::Cursor,
    time::Duration,
};
use viewer_core::{
    ArrivalTime, CameraController, CameraId, CameraState, CameraStatus, CompressedImage,
    McapPlayback, MeasurementTime, PlaybackCommand, PlaybackPerformance, PlaybackRequirements,
    PlaybackSpeed, PlaybackView, ProcessingCounters, RawMessage, SessionPlan, StageTiming,
    StreamDescriptor, StreamId, encode_compressed_image_cdr,
};

const CAMERA_TOPIC: &str = "/camera/front/image/compressed";
const START: i64 = 1_000_000_000;

enum PlaybackStep {
    Elapse(Duration),
    Command(PlaybackCommand),
}

struct PlaybackScenario {
    streams: Vec<StreamDescriptor>,
    primary_camera_topic: String,
    messages: Vec<RawMessage>,
    steps: Vec<PlaybackStep>,
    initial_focus: Option<CameraId>,
}

#[derive(Clone, Copy)]
struct PlaybackObservation {
    playback: PlaybackView,
    latest_camera_arrival: Option<ArrivalTime>,
    camera_status: CameraStatus,
    counters: ProcessingCounters,
}

struct PlaybackOutcome {
    camera_state: CameraState,
    playback_status: PlaybackView,
    counters: ProcessingCounters,
    performance: PlaybackPerformance,
    observations: Vec<PlaybackObservation>,
}

impl PlaybackScenario {
    fn run(self) -> PlaybackOutcome {
        let backing = write_mcap(&self.streams, &self.messages);
        let mut playback = McapPlayback::new(backing).unwrap();
        let plan = SessionPlan::build(
            playback.catalog(),
            &self.primary_camera_topic,
            &PlaybackRequirements::default(),
        )
        .unwrap();
        playback.select_streams(plan.selected_stream_ids());
        let mut cameras = CameraController::new(&plan);
        cameras.set_focused_camera(self.initial_focus);
        let mut observations = Vec::with_capacity(self.steps.len());

        for step in self.steps {
            match step {
                PlaybackStep::Elapse(elapsed) => playback
                    .tick(elapsed, |elapsed, messages| {
                        for message in &messages {
                            cameras.admit(message);
                        }
                        cameras.advance(elapsed);
                    })
                    .unwrap(),
                PlaybackStep::Command(PlaybackCommand::Toggle) => playback.clock_mut().toggle(),
                PlaybackStep::Command(PlaybackCommand::SetSpeed(speed)) => {
                    playback.clock_mut().set_speed(speed);
                }
                PlaybackStep::Command(PlaybackCommand::Seek(cursor)) => playback
                    .seek_with(
                        &viewer_core::RestorePlanner::new(playback.catalog())
                            .plan(cursor, plan.restore_inputs())
                            .unwrap(),
                        |_, messages| {
                            cameras.reset_for_restore();
                            for message in &messages {
                                cameras.admit(message);
                            }
                            cameras.advance(Duration::ZERO);
                        },
                    )
                    .unwrap(),
            }
            observations.push(PlaybackObservation {
                playback: playback.clock().view(),
                latest_camera_arrival: cameras
                    .state()
                    .latest_for(CameraId(0))
                    .map(|frame| frame.arrival_time),
                camera_status: cameras.state().status_for(CameraId(0)),
                counters: cameras.counters(),
            });
        }

        PlaybackOutcome {
            camera_state: cameras.state().clone(),
            playback_status: playback.clock().view(),
            counters: cameras.counters(),
            performance: PlaybackPerformance::from_controllers(
                playback.source_read_timing(),
                StageTiming::default(),
                &cameras,
            ),
            observations,
        }
    }
}

fn write_mcap(streams: &[StreamDescriptor], messages: &[RawMessage]) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    {
        let mut writer =
            Writer::with_options(&mut bytes, WriteOptions::new().use_chunks(false)).unwrap();
        let mut channels = HashMap::new();
        for stream in streams {
            let schema = writer
                .add_schema(
                    &stream.schema,
                    "ros2msg",
                    b"std_msgs/Header header\nstring format\nuint8[] data\n",
                )
                .unwrap();
            let channel = writer
                .add_channel(
                    schema,
                    &stream.topic,
                    &stream.message_encoding,
                    &BTreeMap::new(),
                )
                .unwrap();
            channels.insert(stream.id, channel);
        }
        let mut ordered = messages.to_vec();
        ordered.sort_by_key(|message| (message.arrival_time, message.stream_id.0));
        for (sequence, message) in ordered.iter().enumerate() {
            let channel = channels
                .get(&message.stream_id)
                .copied()
                .expect("scenario message stream is described");
            writer
                .write_to_known_channel(
                    &MessageHeader {
                        channel_id: channel,
                        sequence: sequence as u32,
                        log_time: message.arrival_time.0 as u64,
                        publish_time: message.arrival_time.0.saturating_sub(10_000_000) as u64,
                    },
                    &message.payload,
                )
                .unwrap();
        }
        writer.finish().unwrap();
    }
    bytes.into_inner()
}

fn camera_stream(id: u32, topic: &str) -> StreamDescriptor {
    StreamDescriptor {
        id: StreamId(id),
        topic: topic.into(),
        schema: "sensor_msgs/msg/CompressedImage".into(),
        message_encoding: "cdr".into(),
        timing: viewer_core::StreamTimingSummary::default(),
    }
}

fn camera_message(stream_id: StreamId, arrival_time: i64, marker: u8) -> RawMessage {
    let measurement_time = arrival_time - 10_000_000;
    RawMessage {
        stream_id,
        arrival_time: ArrivalTime(arrival_time),
        payload: encode_compressed_image_cdr(&CompressedImage {
            measurement_time: MeasurementTime(measurement_time),
            frame_id: "camera_front_optical_frame".into(),
            format: "jpeg".into(),
            jpeg: vec![marker],
        })
        .unwrap()
        .into(),
    }
}

#[test]
fn supplied_elapsed_and_commands_control_cursor_and_message_cutoff() {
    let outcome = PlaybackScenario {
        streams: vec![camera_stream(1, CAMERA_TOPIC)],
        primary_camera_topic: CAMERA_TOPIC.into(),
        messages: vec![
            camera_message(StreamId(1), START, 0),
            camera_message(StreamId(1), START + 250_000_000, 1),
            camera_message(StreamId(1), START + 500_000_000, 2),
            camera_message(StreamId(1), START + 1_000_000_000, 3),
        ],
        steps: vec![
            PlaybackStep::Elapse(Duration::ZERO),
            PlaybackStep::Elapse(Duration::from_millis(500)),
            PlaybackStep::Command(PlaybackCommand::Toggle),
            PlaybackStep::Command(PlaybackCommand::SetSpeed(PlaybackSpeed::Half)),
            PlaybackStep::Elapse(Duration::from_millis(500)),
            PlaybackStep::Command(PlaybackCommand::SetSpeed(PlaybackSpeed::Double)),
            PlaybackStep::Elapse(Duration::from_millis(250)),
            PlaybackStep::Command(PlaybackCommand::Toggle),
            PlaybackStep::Elapse(Duration::from_secs(1)),
        ],
        initial_focus: Some(CameraId(0)),
    }
    .run();

    let initial = outcome.observations[0];
    let paused = outcome.observations[1];
    assert_eq!(initial.playback.cursor, ArrivalTime(START));
    assert_eq!(paused.playback.cursor, initial.playback.cursor);
    assert_eq!(paused.latest_camera_arrival, Some(ArrivalTime(START)));
    assert_eq!(paused.counters.decoded, 1);

    let half_speed = outcome.observations[4];
    assert_eq!(half_speed.playback.cursor, ArrivalTime(START + 250_000_000));
    assert_eq!(
        half_speed.latest_camera_arrival,
        Some(ArrivalTime(START + 250_000_000))
    );

    let double_speed = outcome.observations[6];
    assert_eq!(
        double_speed.playback.cursor,
        ArrivalTime(START + 750_000_000)
    );
    assert_eq!(
        double_speed.latest_camera_arrival,
        Some(ArrivalTime(START + 500_000_000))
    );

    assert_eq!(
        outcome.playback_status.cursor,
        ArrivalTime(START + 750_000_000)
    );
    assert!(!outcome.playback_status.playing);
    assert_eq!(outcome.counters.decoded, 3);
    let latest = outcome.camera_state.latest_for(CameraId(0)).unwrap();
    assert_eq!(latest.arrival_time, ArrivalTime(START + 500_000_000));
    assert_eq!(latest.jpeg, vec![2]);
}

#[test]
fn focused_camera_is_presented_at_ten_hz_and_background_cameras_at_five_hz() {
    let topics = [
        CAMERA_TOPIC,
        "/camera/rear/image/compressed",
        "/camera/left/image/compressed",
        "/camera/right/image/compressed",
        "/camera/front_left/image/compressed",
        "/camera/front_right/image/compressed",
        "/camera/rear_left/image/compressed",
    ];
    let streams = topics
        .iter()
        .enumerate()
        .map(|(index, topic)| camera_stream(index as u32 + 1, topic))
        .collect::<Vec<_>>();
    let mut messages = Vec::new();
    for tick in 0..=50 {
        let arrival = START + tick * 20_000_000;
        for stream in &streams {
            messages.push(camera_message(stream.id, arrival, tick as u8));
        }
    }
    let mut steps = vec![
        PlaybackStep::Elapse(Duration::ZERO),
        PlaybackStep::Command(PlaybackCommand::Toggle),
    ];
    steps.extend((0..10).map(|_| PlaybackStep::Elapse(Duration::from_millis(100))));

    let outcome = PlaybackScenario {
        streams,
        primary_camera_topic: CAMERA_TOPIC.into(),
        messages,
        steps,
        initial_focus: Some(CameraId(0)),
    }
    .run();

    let after_first_burst = outcome.observations[2];
    assert_eq!(
        after_first_burst.latest_camera_arrival,
        Some(ArrivalTime(START + 100_000_000)),
        "the focused camera must select the newest pending message"
    );

    let focused = outcome
        .performance
        .camera_presented_by_id
        .get(&CameraId(0))
        .copied()
        .unwrap_or_default();
    assert_eq!(focused, 11);
    for camera_id in 1..7 {
        let presented = outcome
            .performance
            .camera_presented_by_id
            .get(&CameraId(camera_id))
            .copied()
            .unwrap_or_default();
        assert!(
            (5..=6).contains(&presented),
            "background camera {camera_id} presented {presented} frames"
        );
    }
    assert_eq!(outcome.performance.camera_input_frames, 7 * 51);
    assert!(outcome.counters.dropped > 0);

    let focused_frame = outcome.camera_state.latest_for(CameraId(0)).unwrap();
    assert_eq!(
        focused_frame.arrival_time,
        ArrivalTime(START + 1_000_000_000)
    );
    assert_eq!(focused_frame.jpeg, vec![50]);
}

#[test]
fn seek_restores_the_recent_camera_sample_before_committing() {
    let seek_cursor = ArrivalTime(START + 650_000_000);
    let outcome = PlaybackScenario {
        streams: vec![camera_stream(1, CAMERA_TOPIC)],
        primary_camera_topic: CAMERA_TOPIC.into(),
        messages: vec![
            camera_message(StreamId(1), START, 0),
            camera_message(StreamId(1), START + 50_000_000, 1),
            camera_message(StreamId(1), START + 750_000_000, 2),
            camera_message(StreamId(1), START + 1_000_000_000, 3),
        ],
        steps: vec![
            PlaybackStep::Elapse(Duration::ZERO),
            PlaybackStep::Command(PlaybackCommand::Toggle),
            PlaybackStep::Elapse(Duration::from_millis(50)),
            PlaybackStep::Command(PlaybackCommand::Seek(seek_cursor)),
            PlaybackStep::Elapse(Duration::ZERO),
            PlaybackStep::Elapse(Duration::from_millis(100)),
        ],
        initial_focus: Some(CameraId(0)),
    }
    .run();

    let before_seek = outcome.observations[2];
    assert_eq!(before_seek.latest_camera_arrival, Some(ArrivalTime(START)));
    assert_eq!(before_seek.camera_status, CameraStatus::Ready);

    let immediately_after_seek = outcome.observations[3];
    assert_eq!(immediately_after_seek.playback.cursor, seek_cursor);
    assert!(immediately_after_seek.playback.playing);
    assert_eq!(
        immediately_after_seek.latest_camera_arrival,
        Some(ArrivalTime(START + 50_000_000))
    );
    assert_eq!(immediately_after_seek.camera_status, CameraStatus::Ready);

    let zero_elapsed_after_seek = outcome.observations[4];
    assert_eq!(zero_elapsed_after_seek.playback.cursor, seek_cursor);
    assert_eq!(
        zero_elapsed_after_seek.latest_camera_arrival,
        Some(ArrivalTime(START + 50_000_000))
    );
    assert_eq!(zero_elapsed_after_seek.camera_status, CameraStatus::Ready);

    let next_reached_frame = outcome.observations[5];
    assert_eq!(
        next_reached_frame.playback.cursor,
        ArrivalTime(START + 750_000_000)
    );
    assert_eq!(
        next_reached_frame.latest_camera_arrival,
        Some(ArrivalTime(START + 750_000_000))
    );
    assert_eq!(next_reached_frame.camera_status, CameraStatus::Ready);

    let latest = outcome.camera_state.latest_for(CameraId(0)).unwrap();
    assert_eq!(latest.arrival_time, ArrivalTime(START + 750_000_000));
    assert_eq!(latest.jpeg, vec![2]);
}

#[test]
fn seek_leaves_recent_sample_unavailable_when_bounded_restore_finds_none() {
    let seek_cursor = ArrivalTime(START + 50_000_000_000);
    let outcome = PlaybackScenario {
        streams: vec![camera_stream(1, CAMERA_TOPIC)],
        primary_camera_topic: CAMERA_TOPIC.into(),
        messages: vec![
            camera_message(StreamId(1), START, 0),
            camera_message(StreamId(1), START + 100_000_000_000, 1),
        ],
        steps: vec![
            PlaybackStep::Elapse(Duration::ZERO),
            PlaybackStep::Command(PlaybackCommand::Seek(seek_cursor)),
        ],
        initial_focus: Some(CameraId(0)),
    }
    .run();

    let after_seek = outcome.observations[1];
    assert_eq!(after_seek.playback.cursor, seek_cursor);
    assert_eq!(after_seek.latest_camera_arrival, None);
    assert_eq!(
        after_seek.camera_status,
        CameraStatus::WaitingForCameraFrame
    );
}
