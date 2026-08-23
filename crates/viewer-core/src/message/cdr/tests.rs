use super::common::{align_output, push_string, push_u32};
use super::*;
use crate::MeasurementTime;
use bytes::Bytes;

#[test]
fn round_trip_and_normalizes_format() {
    let original = CompressedImage {
        measurement_time: MeasurementTime(12_345_000_006),
        frame_id: "front".into(),
        format: "JPEG compressed bgr8".into(),
        jpeg: vec![0xff, 0xd8, 0xff, 0xd9],
    };
    let decoded =
        decode_compressed_image(&encode_compressed_image_cdr(&original).unwrap()).unwrap();
    assert_eq!(decoded.measurement_time, original.measurement_time);
    assert_eq!(decoded.format, "jpeg");
    assert_eq!(decoded.jpeg, original.jpeg);

    let mut cdr = original;
    cdr.format = "rgb8; jpeg compressed bgr8".into();
    assert_eq!(
        decode_compressed_image(&encode_compressed_image_cdr(&cdr).unwrap())
            .unwrap()
            .format,
        "jpeg"
    );
}

#[test]
fn bytes_decoder_retains_jpeg_in_the_cdr_backing_allocation() {
    let payload = Bytes::from(
        encode_compressed_image_cdr(&CompressedImage {
            measurement_time: MeasurementTime(12_345_000_006),
            frame_id: "front".into(),
            format: "jpeg".into(),
            jpeg: vec![0xff, 0xd8, 0x01, 0x02, 0xff, 0xd9],
        })
        .unwrap(),
    );
    let payload_start = payload.as_ptr() as usize;
    let payload_end = payload_start + payload.len();

    let decoded = decode_compressed_image_bytes(payload.clone()).unwrap();
    let jpeg_start = decoded.jpeg.as_ptr() as usize;

    assert_eq!(decoded.jpeg.as_ref(), [0xff, 0xd8, 0x01, 0x02, 0xff, 0xd9]);
    assert!(jpeg_start >= payload_start);
    assert!(jpeg_start + decoded.jpeg.len() <= payload_end);
}

#[test]
fn tf_message_round_trip() {
    let transforms = vec![TransformStamped {
        measurement_time: MeasurementTime(12_345_000_006),
        frame_id: "base_link".into(),
        child_frame_id: "camera_front_optical_frame".into(),
        translation: [0.2, 0.0, 0.4],
        rotation: [-0.5, 0.5, -0.5, 0.5],
    }];
    let payload = encode_tf_message_cdr(&transforms).unwrap();
    assert_eq!(decode_tf_message(&payload).unwrap(), transforms);
}

#[test]
fn rejects_truncation_format_and_huge_lengths() {
    let valid = encode_compressed_image_cdr(&CompressedImage {
        measurement_time: MeasurementTime(0),
        frame_id: "f".into(),
        format: "jpeg".into(),
        jpeg: vec![1],
    })
    .unwrap();
    assert_eq!(
        decode_compressed_image(&valid[..valid.len() - 1]),
        Err(DecodeError::Truncated)
    );
    let mut bad = valid.clone();
    bad[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode_compressed_image(&bad).is_err());
    let unsupported = encode_compressed_image_cdr(&CompressedImage {
        measurement_time: MeasurementTime(0),
        frame_id: "f".into(),
        format: "png".into(),
        jpeg: vec![],
    })
    .unwrap();
    assert!(matches!(
        decode_compressed_image(&unsupported),
        Err(DecodeError::UnsupportedFormat(_))
    ));

    let mut path = vec![0, 1, 0, 0];
    push_u32(&mut path, 0);
    push_u32(&mut path, 0);
    push_string(&mut path, "map").unwrap();
    push_u32(&mut path, u32::MAX);
    assert!(matches!(
        decode_path(&path),
        Err(DecodeError::InvalidLength)
    ));
}

#[test]
fn reads_big_endian() {
    let payload = [
        0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 5, b'j', b'p', b'e',
        b'g', 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let decoded = decode_compressed_image(&payload).unwrap();
    assert_eq!(decoded.measurement_time, MeasurementTime(1_000_000_002));
}

#[test]
fn decodes_little_endian_path() {
    let mut payload = vec![0, 1, 0, 0];
    push_u32(&mut payload, 12);
    push_u32(&mut payload, 34);
    push_string(&mut payload, "base_link").unwrap();
    push_u32(&mut payload, 1);
    push_u32(&mut payload, 12);
    push_u32(&mut payload, 34);
    push_string(&mut payload, "base_link").unwrap();
    for value in [2.0_f64, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0] {
        align_output(&mut payload, 8);
        payload.extend_from_slice(&value.to_le_bytes());
    }

    let path = decode_path(&payload).unwrap();
    assert_eq!(path.measurement_time, MeasurementTime(12_000_000_034));
    assert_eq!(path.frame_id, "base_link");
    assert_eq!(path.points, vec![[2.0, -1.0]]);
}

#[test]
fn decodes_little_endian_odometry() {
    let mut payload = vec![0, 1, 0, 0];
    push_u32(&mut payload, 20);
    push_u32(&mut payload, 50);
    push_string(&mut payload, "odom").unwrap();
    push_string(&mut payload, "base_footprint").unwrap();
    let pose = [1.5_f64, -2.0, 0.0, 0.0, 0.0, 0.1, 0.995];
    for value in pose {
        align_output(&mut payload, 8);
        payload.extend_from_slice(&value.to_le_bytes());
    }
    for _ in 0..36 {
        align_output(&mut payload, 8);
        payload.extend_from_slice(&0.0_f64.to_le_bytes());
    }
    for value in [0.4_f64, 0.1, 0.0, 0.0, 0.0, -0.2] {
        align_output(&mut payload, 8);
        payload.extend_from_slice(&value.to_le_bytes());
    }
    for _ in 0..36 {
        align_output(&mut payload, 8);
        payload.extend_from_slice(&0.0_f64.to_le_bytes());
    }

    let odometry = decode_odometry(&payload).unwrap();
    assert_eq!(odometry.measurement_time, MeasurementTime(20_000_000_050));
    assert_eq!(odometry.position, [1.5, -2.0, 0.0]);
    assert_eq!(odometry.linear_velocity, [0.4, 0.1, 0.0]);
    assert_eq!(odometry.angular_velocity[2], -0.2);
}

#[test]
fn decodes_little_endian_laser_scan() {
    let mut payload = vec![0, 1, 0, 0];
    push_u32(&mut payload, 12);
    push_u32(&mut payload, 34);
    push_string(&mut payload, "base_scan").unwrap();
    for value in [-1.0_f32, 1.0, 0.5, 0.0, 0.1, 0.12, 8.0] {
        push_u32(&mut payload, value.to_bits());
    }
    push_u32(&mut payload, 3);
    for value in [1.0_f32, f32::INFINITY, 2.0] {
        push_u32(&mut payload, value.to_bits());
    }
    push_u32(&mut payload, 0);

    let scan = decode_laser_scan(&payload).unwrap();
    assert_eq!(scan.measurement_time, MeasurementTime(12_000_000_034));
    assert_eq!(scan.frame_id, "base_scan");
    assert_eq!(scan.ranges.len(), 3);
}

#[test]
fn decodes_tf_message() {
    let mut payload = vec![0, 1, 0, 0];
    push_u32(&mut payload, 1);
    push_u32(&mut payload, 2);
    push_u32(&mut payload, 3);
    push_string(&mut payload, "base_link").unwrap();
    push_string(&mut payload, "base_scan").unwrap();
    for value in [0.1_f64, 0.0, 0.2, 0.0, 0.0, 0.0, 1.0] {
        align_output(&mut payload, 8);
        payload.extend_from_slice(&value.to_le_bytes());
    }
    let transforms = decode_tf_message(&payload).unwrap();
    assert_eq!(transforms.len(), 1);
    assert_eq!(transforms[0].frame_id, "base_link");
    assert_eq!(transforms[0].child_frame_id, "base_scan");
    assert_eq!(transforms[0].translation, [0.1, 0.0, 0.2]);
}
