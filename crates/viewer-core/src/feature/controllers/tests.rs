use super::*;
use crate::{
    ArrivalTime, CameraId, CompressedImage, MeasurementTime, PlaybackRequirements, RawMessage,
    SessionPlan, SourceCatalog, StreamDescriptor, StreamId, StreamTimingSummary, TransformStamped,
    WorkspaceBindings, encode_tf_message_cdr,
};
use bytes::Bytes;
use std::time::Duration;

fn bindings() -> WorkspaceBindings {
    WorkspaceBindings {
        path_topic: "/planning/path".into(),
        odometry_topic: "/odom".into(),
        point_cloud_topic: "/scan".into(),
        dynamic_tf_topic: "/tf".into(),
        static_tf_topic: "/tf_static".into(),
    }
}

fn camera_plan() -> SessionPlan {
    camera_plan_for_topics(&["/camera"])
}

fn camera_plan_for_topics(topics: &[&str]) -> SessionPlan {
    let catalog = SourceCatalog {
        time_range: None,
        capabilities: Default::default(),
        streams: topics
            .iter()
            .enumerate()
            .map(|(index, topic)| StreamDescriptor {
                schema_definition: None,
                id: StreamId(7 + index as u32),
                topic: (*topic).into(),
                schema: "sensor_msgs/msg/CompressedImage".into(),
                message_encoding: "cdr".into(),
                timing: StreamTimingSummary::default(),
            })
            .collect(),
    };
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_all_cameras();
    SessionPlan::build(&catalog, topics[0], &requirements, &bindings()).unwrap()
}

#[test]
fn presenting_one_pending_camera_keeps_other_frames_and_rolls_back_bad_cdr() {
    let plan = camera_plan_for_topics(&["/front", "/rear"]);
    let mut controller = CameraController::new(&plan);
    let first = scheduled_camera_message(1);
    controller.restore(&first).unwrap();
    let mut second = scheduled_camera_message(2);
    second.stream_id = StreamId(8);
    controller.admit(&second);
    let original = controller
        .state()
        .latest_for(CameraId(0))
        .unwrap()
        .data
        .as_ptr();
    controller.present_pending(CameraId(1)).unwrap();
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(1))
            .unwrap()
            .arrival_time,
        ArrivalTime(2)
    );
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(0))
            .unwrap()
            .data
            .as_ptr(),
        original
    );
    let counters = controller.counters();
    controller.present_pending(CameraId(1)).unwrap();
    assert_eq!(controller.counters(), counters);
    second.payload = Bytes::from_static(&[0xff]);
    second.arrival_time = ArrivalTime(3);
    controller.admit(&second);
    let counters = controller.counters();
    assert!(controller.present_pending(CameraId(1)).is_err());
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(1))
            .unwrap()
            .arrival_time,
        ArrivalTime(2)
    );
    assert_eq!(controller.counters(), counters);
    assert_eq!(controller.focused_camera(), Some(CameraId(0)));
}

#[test]
fn camera_admission_coalesces_before_decode_and_keeps_bytes_slice() {
    let payload = Bytes::from(
        crate::encode_compressed_image_cdr(&CompressedImage {
            measurement_time: MeasurementTime(1),
            frame_id: "camera".into(),
            format: "jpeg".into(),
            data: vec![1, 2, 3, 4],
        })
        .unwrap(),
    );
    let mut controller = CameraController::new(&camera_plan());
    for time in [1, 2] {
        assert!(controller.admit(&RawMessage {
            publish_time_ns: None,
            sequence: None,
            stream_id: StreamId(7),
            arrival_time: ArrivalTime(time),
            payload: payload.clone(),
        }));
    }
    controller.advance(Duration::ZERO);
    let frame = controller.state().latest_for(CameraId(0)).unwrap();
    assert_eq!(frame.arrival_time, ArrivalTime(2));
    assert_eq!(controller.counters().dropped, 1);
    let payload_start = payload.as_ptr() as usize;
    let jpeg_start = frame.data.as_ptr() as usize;
    assert!(jpeg_start >= payload_start);
    assert!(jpeg_start + frame.data.len() <= payload_start + payload.len());
}

#[test]
fn camera_coalescing_discards_malformed_old_cdr_before_decode() {
    let valid = Bytes::from(
        crate::encode_compressed_image_cdr(&CompressedImage {
            measurement_time: MeasurementTime(2),
            frame_id: "camera".into(),
            format: "jpeg".into(),
            data: vec![1, 2, 3],
        })
        .unwrap(),
    );
    let mut controller = CameraController::new(&camera_plan());
    assert!(controller.admit(&RawMessage {
        publish_time_ns: None,
        sequence: None,
        stream_id: StreamId(7),
        arrival_time: ArrivalTime(1),
        payload: Bytes::from_static(&[0xff]),
    }));
    assert!(controller.admit(&RawMessage {
        publish_time_ns: None,
        sequence: None,
        stream_id: StreamId(7),
        arrival_time: ArrivalTime(2),
        payload: valid,
    }));

    controller.advance(Duration::ZERO);

    assert_eq!(controller.counters().dropped, 1);
    assert_eq!(controller.counters().decoded, 1);
    assert_eq!(controller.counters().errors, 0);
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(0))
            .unwrap()
            .arrival_time,
        ArrivalTime(2)
    );
}

fn scheduled_camera_message(time: i64) -> RawMessage {
    RawMessage {
        publish_time_ns: None,
        sequence: None,
        stream_id: StreamId(7),
        arrival_time: ArrivalTime(time),
        payload: crate::encode_compressed_image_cdr(&CompressedImage {
            measurement_time: MeasurementTime(time),
            frame_id: "camera".into(),
            format: "jpeg".into(),
            data: vec![1, 2, 3],
        })
        .unwrap()
        .into(),
    }
}

#[test]
fn focused_camera_follows_irregular_input_without_a_fixed_deadline() {
    let mut controller = CameraController::new(&camera_plan());
    controller.admit(&scheduled_camera_message(0));
    controller.advance(Duration::ZERO);
    controller.admit(&scheduled_camera_message(100));
    controller.advance(Duration::from_millis(33));
    controller.admit(&scheduled_camera_message(200));
    controller.advance(Duration::from_millis(17));
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(0))
            .unwrap()
            .arrival_time,
        ArrivalTime(200)
    );
    assert_eq!(controller.counters().decoded, 3);
}

#[test]
fn focused_camera_after_a_stall_keeps_latest_and_accepts_the_next_input() {
    let mut controller = CameraController::new(&camera_plan());
    controller.admit(&scheduled_camera_message(0));
    controller.advance(Duration::ZERO);
    for time in [100, 200, 300] {
        controller.admit(&scheduled_camera_message(time));
    }
    controller.advance(Duration::from_millis(350));
    assert_eq!(controller.counters().decoded, 2);
    assert_eq!(controller.counters().dropped, 2);
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(0))
            .unwrap()
            .arrival_time,
        ArrivalTime(300)
    );
    controller.admit(&scheduled_camera_message(351));
    controller.advance(Duration::ZERO);
    assert_eq!(controller.counters().decoded, 3);
    controller.admit(&scheduled_camera_message(390));
    controller.advance(Duration::from_millis(49));
    assert_eq!(controller.counters().decoded, 4);
    controller.advance(Duration::from_millis(1));
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(0))
            .unwrap()
            .arrival_time,
        ArrivalTime(390)
    );
    assert_eq!(controller.counters().decoded, 4);
    // No catch-up decoding on repeated ticks at the same time.
    controller.advance(Duration::ZERO);
    assert_eq!(controller.counters().decoded, 4);
}

#[test]
fn camera_background_phases_survive_a_stall() {
    let plan = camera_plan_for_topics(&["/camera/0", "/camera/1", "/camera/2"]);
    let mut controller = CameraController::new(&plan);
    for id in 0..3 {
        let mut message = scheduled_camera_message(0);
        message.stream_id = StreamId(7 + id);
        controller.admit(&message);
    }
    controller.advance(Duration::from_millis(350));
    assert_eq!(controller.counters().decoded, 3);
    for id in 0..3 {
        let mut message = scheduled_camera_message(400);
        message.stream_id = StreamId(7 + id);
        controller.admit(&message);
    }
    controller.advance(Duration::from_millis(50));
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(0))
            .unwrap()
            .arrival_time,
        ArrivalTime(400)
    );
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(1))
            .unwrap()
            .arrival_time,
        ArrivalTime(400)
    );
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(2))
            .unwrap()
            .arrival_time,
        ArrivalTime(0)
    );
    assert_eq!(controller.counters().decoded, 5);
    controller.advance(Duration::from_millis(100));
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(2))
            .unwrap()
            .arrival_time,
        ArrivalTime(400)
    );
    assert_eq!(controller.counters().decoded, 6);
}

#[test]
fn camera_focus_and_restore_reanchor_their_schedule() {
    let plan = camera_plan_for_topics(&["/camera/0", "/camera/1"]);
    let mut controller = CameraController::new(&plan);
    controller.admit(&scheduled_camera_message(0));
    controller.advance(Duration::from_millis(107));
    controller.set_focused_camera(Some(CameraId(1)));
    let mut focused = scheduled_camera_message(108);
    focused.stream_id = StreamId(8);
    controller.admit(&focused);
    controller.admit(&scheduled_camera_message(108));
    controller.advance(Duration::ZERO);
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(1))
            .unwrap()
            .arrival_time,
        ArrivalTime(108)
    );
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(0))
            .unwrap()
            .arrival_time,
        ArrivalTime(0)
    );
    controller.reset_for_restore();
    focused.arrival_time = ArrivalTime(1);
    controller.admit(&focused);
    controller.advance(Duration::ZERO);
    assert_eq!(
        controller
            .state()
            .latest_for(CameraId(1))
            .unwrap()
            .arrival_time,
        ArrivalTime(1)
    );
    assert!(controller.state().latest_for(CameraId(0)).is_none());
}

#[test]
fn camera_cadence_handles_saturated_elapsed_without_arithmetic_overflow() {
    let mut controller = CameraController::new(&camera_plan());
    controller.admit(&scheduled_camera_message(1));
    controller.advance(Duration::MAX);
    assert_eq!(controller.counters().decoded, 1);
    controller.advance(Duration::from_nanos(1));
    assert_eq!(controller.counters().decoded, 1);
}

#[test]
fn background_camera_rate_remains_five_hz() {
    assert_eq!(CameraController::background_hz(), 5.0);
}

#[test]
fn camera_restore_bypasses_playback_scheduler_for_every_selected_camera() {
    let catalog = SourceCatalog {
        time_range: None,
        capabilities: Default::default(),
        streams: (0..3)
            .map(|index| StreamDescriptor {
                schema_definition: None,
                id: StreamId(7 + index),
                topic: format!("/camera/{index}"),
                schema: "sensor_msgs/msg/CompressedImage".into(),
                message_encoding: "cdr".into(),
                timing: StreamTimingSummary::default(),
            })
            .collect(),
    };
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_all_cameras();
    let plan = SessionPlan::build(&catalog, "/camera/0", &requirements, &bindings()).unwrap();
    let mut controller = CameraController::new(&plan);
    let payload = |time| {
        Bytes::from(
            crate::encode_compressed_image_cdr(&CompressedImage {
                measurement_time: MeasurementTime(time),
                frame_id: "camera".into(),
                format: "jpeg".into(),
                data: vec![time as u8],
            })
            .unwrap(),
        )
    };
    for index in 0..3 {
        assert!(
            controller
                .restore(&RawMessage {
                    publish_time_ns: None,
                    sequence: None,
                    stream_id: StreamId(7 + index),
                    arrival_time: ArrivalTime(i64::from(index)),
                    payload: payload(i64::from(index)),
                })
                .unwrap()
        );
    }

    assert_eq!(controller.state().frames().count(), 3);
    assert_eq!(controller.counters().decoded, 3);
    assert_eq!(controller.counters().dropped, 0);
}

#[test]
fn repeated_static_transforms_restore_only_updates_valid_at_target() {
    let catalog = SourceCatalog {
        time_range: None,
        capabilities: Default::default(),
        streams: vec![StreamDescriptor {
            schema_definition: None,
            id: StreamId(9),
            topic: "/tf_static".into(),
            schema: "tf2_msgs/msg/TFMessage".into(),
            message_encoding: "cdr".into(),
            timing: StreamTimingSummary::default(),
        }],
    };
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_transforms();
    let plan = SessionPlan::build(&catalog, "/unused", &requirements, &bindings()).unwrap();
    let mut controller = TransformController::new(&plan);
    let mut archive = Vec::new();
    for (arrival, x) in [(10, 1.0), (20, 2.0)] {
        let message = RawMessage {
            publish_time_ns: None,
            sequence: None,
            stream_id: StreamId(9),
            arrival_time: ArrivalTime(arrival),
            payload: encode_tf_message_cdr(&[TransformStamped {
                measurement_time: MeasurementTime(arrival),
                frame_id: "map".into(),
                child_frame_id: "base".into(),
                translation: [x, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
            }])
            .unwrap()
            .into(),
        };
        assert!(controller.process(&message));
        archive.push(message);
    }

    controller.reset_for_restore(ArrivalTime(15));
    for message in archive
        .iter()
        .filter(|message| message.arrival_time <= ArrivalTime(15))
    {
        controller.process(message);
    }
    assert_eq!(
        controller
            .state()
            .transform_points("base", "map", &[[0.0, 0.0, 0.0]])
            .unwrap(),
        vec![[1.0, 0.0, 0.0]]
    );
    controller.reset_for_restore(ArrivalTime(25));
    for message in archive
        .iter()
        .filter(|message| message.arrival_time <= ArrivalTime(25))
    {
        controller.process(message);
    }
    assert_eq!(
        controller
            .state()
            .transform_points("base", "map", &[[0.0, 0.0, 0.0]])
            .unwrap(),
        vec![[2.0, 0.0, 0.0]]
    );
}

#[test]
fn raw_camera_selection_coalescing_and_failed_restore_preserve_current_frame() {
    use crate::message::cdr::wire::{push_string, push_u32};
    let catalog = SourceCatalog {
        time_range: None,
        capabilities: Default::default(),
        streams: vec![StreamDescriptor {
            id: StreamId(7),
            topic: "/raw".into(),
            schema: "sensor_msgs/msg/Image".into(),
            message_encoding: "cdr".into(),
            schema_definition: None,
            timing: Default::default(),
        }],
    };
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_camera_topic("/raw");
    let plan = SessionPlan::build(&catalog, "/raw", &requirements, &bindings()).unwrap();
    let mut controller = CameraController::new(&plan);
    let mut bytes = vec![0, 1, 0, 0];
    push_u32(&mut bytes, 0);
    push_u32(&mut bytes, 1);
    push_string(&mut bytes, "optical").unwrap();
    push_u32(&mut bytes, 1);
    push_u32(&mut bytes, 2);
    push_string(&mut bytes, "bgr8").unwrap();
    bytes.push(0);
    push_u32(&mut bytes, 8);
    push_u32(&mut bytes, 8);
    bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6, 0, 0]);
    let mut message = RawMessage {
        stream_id: StreamId(7),
        arrival_time: ArrivalTime(1),
        publish_time_ns: None,
        sequence: None,
        payload: Bytes::from_static(b"bad"),
    };
    controller.admit(&message);
    message.arrival_time = ArrivalTime(2);
    message.payload = bytes.into();
    controller.admit(&message);
    controller.advance(Duration::ZERO);
    let before = controller.state().latest_for(CameraId(0)).unwrap().clone();
    assert_eq!(before.arrival_time, ArrivalTime(2));
    assert!(matches!(before.format, crate::CameraImageFormat::Raw(_)));
    assert_eq!(controller.counters().errors, 0);
    assert_eq!(controller.counters().dropped, 1);
    let mut candidate = controller.restore_candidate();
    assert_eq!(candidate.counters(), controller.counters());
    assert_eq!(candidate.focused_camera(), controller.focused_camera());
    assert_eq!(candidate.input_frames(), controller.input_frames());
    assert_eq!(candidate.presented_by_id(), controller.presented_by_id());
    assert!(candidate.state().latest_for(CameraId(0)).is_none());
    // The Raw-vs-Compressed route is configuration, and must survive candidate construction.
    assert!(candidate.restore(&message).unwrap());
    assert_eq!(candidate.state().latest_for(CameraId(0)), Some(&before));
    assert_eq!(controller.state().latest_for(CameraId(0)), Some(&before));
    message.payload = Bytes::from_static(b"bad");
    message.arrival_time = ArrivalTime(3);
    assert!(controller.restore(&message).is_err());
    assert_eq!(controller.state().latest_for(CameraId(0)), Some(&before));
}

#[test]
fn focused_camera_tracks_thirty_hz_then_slower_irregular_input_without_repeats() {
    let mut controller = CameraController::new(&camera_plan());
    let mut previous = 0;
    for index in 0..30 {
        let time = index * 1_000_000_000 / 30;
        controller.admit(&scheduled_camera_message(time));
        controller.advance(Duration::from_nanos((time - previous) as u64));
        assert_eq!(
            controller
                .state()
                .latest_for(CameraId(0))
                .unwrap()
                .arrival_time,
            ArrivalTime(time)
        );
        previous = time;
    }
    for time in [1_100_000_000, 1_270_000_000, 1_310_000_000] {
        controller.admit(&scheduled_camera_message(time));
        controller.advance(Duration::from_nanos((time - previous) as u64));
        assert_eq!(
            controller
                .state()
                .latest_for(CameraId(0))
                .unwrap()
                .arrival_time,
            ArrivalTime(time)
        );
        previous = time;
    }
    assert_eq!(controller.counters().decoded, 33);
    controller.advance(Duration::from_secs(1));
    assert_eq!(controller.counters().decoded, 33);
}
