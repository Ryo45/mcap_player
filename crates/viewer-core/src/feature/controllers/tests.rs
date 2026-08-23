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
    let catalog = SourceCatalog {
        time_range: None,
        capabilities: Default::default(),
        streams: vec![StreamDescriptor {
            id: StreamId(7),
            topic: "/camera".into(),
            schema: "sensor_msgs/msg/CompressedImage".into(),
            message_encoding: "cdr".into(),
            timing: StreamTimingSummary::default(),
        }],
    };
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_all_cameras();
    SessionPlan::build(&catalog, "/camera", &requirements, &bindings()).unwrap()
}

#[test]
fn camera_admission_coalesces_before_decode_and_keeps_bytes_slice() {
    let payload = Bytes::from(
        crate::encode_compressed_image_cdr(&CompressedImage {
            measurement_time: MeasurementTime(1),
            frame_id: "camera".into(),
            format: "jpeg".into(),
            jpeg: vec![1, 2, 3, 4],
        })
        .unwrap(),
    );
    let mut controller = CameraController::new(&camera_plan());
    for time in [1, 2] {
        assert!(controller.admit(&RawMessage {
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
    let jpeg_start = frame.jpeg.as_ptr() as usize;
    assert!(jpeg_start >= payload_start);
    assert!(jpeg_start + frame.jpeg.len() <= payload_start + payload.len());
}

#[test]
fn camera_coalescing_discards_malformed_old_cdr_before_decode() {
    let valid = Bytes::from(
        crate::encode_compressed_image_cdr(&CompressedImage {
            measurement_time: MeasurementTime(2),
            frame_id: "camera".into(),
            format: "jpeg".into(),
            jpeg: vec![1, 2, 3],
        })
        .unwrap(),
    );
    let mut controller = CameraController::new(&camera_plan());
    assert!(controller.admit(&RawMessage {
        stream_id: StreamId(7),
        arrival_time: ArrivalTime(1),
        payload: Bytes::from_static(&[0xff]),
    }));
    assert!(controller.admit(&RawMessage {
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

#[test]
fn camera_rates_preserve_the_existing_policy() {
    assert_eq!(CameraController::focused_hz(), 10.0);
    assert_eq!(CameraController::background_hz(), 5.0);
}

#[test]
fn camera_restore_bypasses_playback_scheduler_for_every_selected_camera() {
    let catalog = SourceCatalog {
        time_range: None,
        capabilities: Default::default(),
        streams: (0..3)
            .map(|index| StreamDescriptor {
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
                jpeg: vec![time as u8],
            })
            .unwrap(),
        )
    };
    for index in 0..3 {
        assert!(
            controller
                .restore(&RawMessage {
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
