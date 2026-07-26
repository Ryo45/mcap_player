use mcap::{WriteOptions, Writer, records::MessageHeader};
use std::{collections::BTreeMap, io::Cursor, time::Duration};
use viewer_core::{
    ArrivalTime, CameraId, CompressedImage, DomainState, McapPlayback, MeasurementTime,
    PipelineCounters, PlaybackCommand, PlaybackSpeed, PlaybackView, RawMessage, StreamDescriptor,
    StreamId, encode_compressed_image_cdr,
};

const CAMERA_TOPIC: &str = "/camera/front/image/compressed";
const START: i64 = 1_000_000_000;

enum PlaybackStep {
    Elapse(Duration),
    Command(PlaybackCommand),
}

struct PlaybackScenario {
    stream: StreamDescriptor,
    messages: Vec<RawMessage>,
    steps: Vec<PlaybackStep>,
    initial_focus: Option<CameraId>,
}

#[derive(Clone, Copy)]
struct PlaybackObservation {
    playback: PlaybackView,
    latest_camera_arrival: Option<ArrivalTime>,
    counters: PipelineCounters,
}

struct PlaybackOutcome {
    domain_state: DomainState,
    playback_status: PlaybackView,
    counters: PipelineCounters,
    observations: Vec<PlaybackObservation>,
}

impl PlaybackScenario {
    fn run(self) -> PlaybackOutcome {
        let camera_topic = self.stream.topic.clone();
        let backing = write_mcap(&self.stream, &self.messages);
        let mut playback = McapPlayback::new(backing, &camera_topic).unwrap();
        playback.set_focused_camera(self.initial_focus);
        let mut observations = Vec::with_capacity(self.steps.len());

        for step in self.steps {
            match step {
                PlaybackStep::Elapse(elapsed) => playback.tick(elapsed).unwrap(),
                PlaybackStep::Command(command) => apply_legacy_command(&mut playback, command),
            }
            observations.push(PlaybackObservation {
                playback: playback.clock().view(),
                latest_camera_arrival: playback
                    .state()
                    .camera
                    .latest_for(CameraId(0))
                    .map(|frame| frame.arrival_time),
                counters: playback.counters(),
            });
        }

        PlaybackOutcome {
            domain_state: playback.state().clone(),
            playback_status: playback.clock().view(),
            counters: playback.counters(),
            observations,
        }
    }
}

fn apply_legacy_command(playback: &mut McapPlayback<Vec<u8>>, command: PlaybackCommand) {
    // Temporary test adapter: scenario assertions stay stable while production
    // still exposes command handling through separate mutable playback methods.
    match command {
        PlaybackCommand::Toggle => playback.clock_mut().toggle(),
        PlaybackCommand::SetSpeed(speed) => playback.clock_mut().set_speed(speed),
        PlaybackCommand::Seek(cursor) => playback.seek(cursor).unwrap(),
    }
}

fn write_mcap(stream: &StreamDescriptor, messages: &[RawMessage]) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    {
        let mut writer =
            Writer::with_options(&mut bytes, WriteOptions::new().use_chunks(false)).unwrap();
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
        for (sequence, message) in messages.iter().enumerate() {
            assert_eq!(message.stream_id, stream.id);
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

fn camera_message(arrival_time: i64, marker: u8) -> RawMessage {
    let measurement_time = arrival_time - 10_000_000;
    RawMessage {
        stream_id: StreamId(1),
        arrival_time: ArrivalTime(arrival_time),
        payload: encode_compressed_image_cdr(&CompressedImage {
            measurement_time: MeasurementTime(measurement_time),
            frame_id: "camera_front_optical_frame".into(),
            format: "jpeg".into(),
            jpeg: vec![marker],
        })
        .unwrap(),
    }
}

#[test]
fn supplied_elapsed_and_commands_control_cursor_and_message_cutoff() {
    let outcome = PlaybackScenario {
        stream: StreamDescriptor {
            id: StreamId(1),
            topic: CAMERA_TOPIC.into(),
            schema: "sensor_msgs/msg/CompressedImage".into(),
            message_encoding: "cdr".into(),
        },
        messages: vec![
            camera_message(START, 0),
            camera_message(START + 250_000_000, 1),
            camera_message(START + 500_000_000, 2),
            camera_message(START + 1_000_000_000, 3),
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
    let latest = outcome.domain_state.camera.latest_for(CameraId(0)).unwrap();
    assert_eq!(latest.arrival_time, ArrivalTime(START + 500_000_000));
    assert_eq!(latest.jpeg, vec![2]);
}
