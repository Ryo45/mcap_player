use super::wire::{align_output, push_string, push_u32};
use super::*;
use crate::MeasurementTime;
use bytes::Bytes;

#[test]
fn round_trip_and_normalizes_format() {
    let original = CompressedImage {
        measurement_time: MeasurementTime(12_345_000_006),
        frame_id: "front".into(),
        format: "JPEG compressed bgr8".into(),
        data: vec![0xff, 0xd8, 0xff, 0xd9],
    };
    let decoded =
        decode_compressed_image(&encode_compressed_image_cdr(&original).unwrap()).unwrap();
    assert_eq!(decoded.measurement_time, original.measurement_time);
    assert_eq!(decoded.format, "jpeg");
    assert_eq!(decoded.data, original.data);

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
            data: vec![0xff, 0xd8, 0x01, 0x02, 0xff, 0xd9],
        })
        .unwrap(),
    );
    let payload_start = payload.as_ptr() as usize;
    let payload_end = payload_start + payload.len();

    let decoded = decode_compressed_image_bytes(payload.clone()).unwrap();
    let jpeg_start = decoded.data.as_ptr() as usize;

    assert_eq!(decoded.data.as_ref(), [0xff, 0xd8, 0x01, 0x02, 0xff, 0xd9]);
    assert!(jpeg_start >= payload_start);
    assert!(jpeg_start + decoded.data.len() <= payload_end);
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
fn live_adapter_models_round_trip_through_concrete_cdr_encoders() {
    let path = PathMessage {
        measurement_time: MeasurementTime(12_345_000_006),
        frame_id: "map".into(),
        points: vec![[1.0, -2.0, 0.0], [3.5, 4.25, 0.0]],
    };
    assert_eq!(decode_path(&encode_path_cdr(&path).unwrap()).unwrap(), path);

    let odometry = Odometry {
        measurement_time: MeasurementTime(12_345_000_006),
        frame_id: "odom".into(),
        child_frame_id: "base_link".into(),
        position: [1.0, 2.0, 3.0],
        orientation: [0.0, 0.0, 0.25, 0.968_245_836_6],
        linear_velocity: [4.0, 0.0, 0.0],
        angular_velocity: [0.0, 0.0, -0.2],
    };
    assert_eq!(
        decode_odometry(&encode_odometry_cdr(&odometry).unwrap()).unwrap(),
        odometry
    );

    let scan = LaserScan {
        measurement_time: MeasurementTime(12_345_000_006),
        frame_id: "base_scan".into(),
        angle_min: -1.0,
        angle_increment: 0.5,
        range_min: 0.1,
        range_max: 30.0,
        ranges: vec![1.0, f32::INFINITY, 2.0],
    };
    assert_eq!(
        decode_laser_scan(&encode_laser_scan_cdr(&scan).unwrap()).unwrap(),
        scan
    );
}

#[test]
fn rejects_truncation_format_and_huge_lengths() {
    let valid = encode_compressed_image_cdr(&CompressedImage {
        measurement_time: MeasurementTime(0),
        frame_id: "f".into(),
        format: "jpeg".into(),
        data: vec![1],
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
        format: "16UC1; compressedDepth png".into(),
        data: vec![],
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
    assert_eq!(path.points, vec![[2.0, -1.0, 0.0]]);
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

#[test]
fn raw_images_preserve_stride_time_and_shared_backing() {
    use crate::{RawImageEncoding, decode_raw_image_bytes};
    use bytes::Bytes;
    for (name, channels, expected) in [
        ("rgb8", 3, RawImageEncoding::Rgb8),
        ("bgr8", 3, RawImageEncoding::Bgr8),
        ("rgba8", 4, RawImageEncoding::Rgba8),
        ("bgra8", 4, RawImageEncoding::Bgra8),
        ("mono8", 1, RawImageEncoding::Mono8),
    ] {
        for endian in [0, 1] {
            let mut bytes = vec![0, 1, 0, 0];
            push_u32(&mut bytes, u32::MAX);
            push_u32(&mut bytes, 123);
            push_string(&mut bytes, "optical").unwrap();
            push_u32(&mut bytes, 2);
            push_u32(&mut bytes, 3);
            push_string(&mut bytes, name).unwrap();
            bytes.push(endian);
            let step = 3 * channels + 2;
            push_u32(&mut bytes, step);
            push_u32(&mut bytes, step * 2);
            let start = bytes.len();
            bytes.resize(start + step as usize * 2, 42);
            let bytes = Bytes::from(bytes);
            let image = decode_raw_image_bytes(bytes.clone()).unwrap();
            assert_eq!(image.measurement_time, MeasurementTime(-999_999_877));
            assert_eq!(image.layout.encoding, expected);
            assert_eq!(image.layout.step, step);
            assert_eq!(image.data.as_ptr(), bytes.slice(start..).as_ptr());
            for length in 0..bytes.len() {
                assert!(decode_raw_image_bytes(bytes.slice(..length)).is_err());
            }
        }
    }
}

#[test]
fn raw_image_layout_rejects_bad_lengths_overflow_and_excessive_sizes() {
    use crate::{RawImageEncoding, RawImageLayout};
    let good = RawImageLayout {
        width: 2,
        height: 2,
        step: 8,
        encoding: RawImageEncoding::Rgb8,
    };
    assert!(good.validate(16).is_ok());
    for bad in [
        RawImageLayout { width: 0, ..good },
        RawImageLayout { step: 5, ..good },
        RawImageLayout {
            width: u32::MAX,
            ..good
        },
        RawImageLayout {
            height: u32::MAX,
            ..good
        },
    ] {
        assert!(bad.validate(16).is_err());
    }
    for len in [0, 15, 17, usize::MAX] {
        assert!(good.validate(len).is_err());
    }
}

#[test]
fn compressed_png_and_empty_jpeg_format_share_the_codec_boundary() {
    for (format, expected) in [
        ("rgb8; png compressed bgr8", "png"),
        ("PNG", "png"),
        ("", "jpeg"),
    ] {
        let bytes = encode_compressed_image_cdr(&CompressedImage {
            measurement_time: MeasurementTime(0),
            frame_id: "optical".into(),
            format: format.into(),
            data: vec![1, 2],
        })
        .unwrap();
        assert_eq!(decode_compressed_image(&bytes).unwrap().format, expected);
    }
}

#[test]
fn raw_image_encoder_round_trips_all_supported_layouts_and_rejects_invalid_input() {
    use crate::{RawImageEncoding, RawImageLayout, decode_raw_image_bytes, encode_raw_image_cdr};
    for encoding in [
        RawImageEncoding::Rgb8,
        RawImageEncoding::Bgr8,
        RawImageEncoding::Rgba8,
        RawImageEncoding::Bgra8,
        RawImageEncoding::Mono8,
    ] {
        let layout = RawImageLayout {
            width: 3,
            height: 2,
            step: 3 * encoding.channels() + 2,
            encoding,
        };
        let data = vec![17; layout.step as usize * 2];
        let encoded = encode_raw_image_cdr(MeasurementTime(-1), "optical", layout, &data).unwrap();
        let decoded = decode_raw_image_bytes(encoded.into()).unwrap();
        assert_eq!(decoded.measurement_time, MeasurementTime(-1));
        assert_eq!(decoded.frame_id, "optical");
        assert_eq!(decoded.layout, layout);
        assert_eq!(decoded.data, data);
        assert!(
            encode_raw_image_cdr(
                MeasurementTime(0),
                "optical",
                layout,
                &data[..data.len() - 1]
            )
            .is_err()
        );
        assert!(
            encode_raw_image_cdr(MeasurementTime(0), &"x".repeat(4097), layout, &data).is_err()
        );
    }
    for unsupported in ["mono16", "16UC1", "32FC1", "rgb16", ""] {
        assert!(RawImageEncoding::from_ros_encoding(unsupported).is_err());
    }
}
