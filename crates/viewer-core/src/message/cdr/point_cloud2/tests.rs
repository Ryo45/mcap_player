use super::super::wire::{push_string, push_u32};
use super::*;

const LITTLE: &[u8] =
    include_bytes!("../../../../../../tests/fixtures/point-cloud2/organized-little-data.cdr");
const BIG_DATA: &[u8] =
    include_bytes!("../../../../../../tests/fixtures/point-cloud2/organized-big-data.cdr");

#[test]
fn ros_jazzy_fixture_preserves_organized_xyz_colors_intensity_and_padding() {
    for data in [LITTLE, BIG_DATA] {
        let cloud = decode_point_cloud2(data).unwrap();
        assert_eq!(cloud.measurement_time, MeasurementTime(12_345_000_000));
        assert_eq!(cloud.frame_id, "lidar");
        assert_eq!(
            cloud.points,
            vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [-1.0, -2.0, -3.0]]
        );
        assert_eq!(
            cloud.attributes.rgba,
            Some(vec![[255, 0, 0, 255], [0, 255, 0, 128], [0, 0, 255, 255]])
        );
        assert_eq!(cloud.attributes.intensity, Some(vec![0.0, 0.5, 1.0]));
    }
    let empty = decode_point_cloud2(include_bytes!(
        "../../../../../../tests/fixtures/point-cloud2/empty.cdr"
    ))
    .unwrap();
    assert!(empty.points.is_empty());
}

fn payload(
    fields: &[(&str, u32, u8, u32)],
    width: u32,
    height: u32,
    point_step: u32,
    row_step: u32,
    data: &[u8],
) -> Vec<u8> {
    let mut out = vec![0, 1, 0, 0];
    push_u32(&mut out, 0);
    push_u32(&mut out, 0);
    push_string(&mut out, "lidar").unwrap();
    push_u32(&mut out, height);
    push_u32(&mut out, width);
    push_u32(&mut out, fields.len() as u32);
    for (name, offset, datatype, count) in fields {
        push_string(&mut out, name).unwrap();
        push_u32(&mut out, *offset);
        out.push(*datatype);
        push_u32(&mut out, *count);
    }
    out.push(0);
    push_u32(&mut out, point_step);
    push_u32(&mut out, row_step);
    push_u32(&mut out, data.len() as u32);
    out.extend_from_slice(data);
    out.push(1);
    out
}

#[test]
fn rejects_malformed_layouts_and_truncation_without_panicking() {
    let xyz = [("x", 0, 7, 1), ("y", 4, 7, 1), ("z", 8, 7, 1)];
    for data in [
        payload(&xyz, 1, 1, 12, 11, &[0; 12]),
        payload(&xyz, 1, 2, 12, 12, &[0; 12]),
        payload(&xyz, 1, 1, 8, 12, &[0; 12]),
        payload(&xyz, 1, 1, 0, 12, &[0; 12]),
        payload(&xyz, 1_000_001, 1, 12, 12, &[0; 12]),
        payload(
            &[("x", 0, 7, 1), ("x", 4, 7, 1), ("z", 8, 7, 1)],
            1,
            1,
            12,
            12,
            &[0; 12],
        ),
        payload(
            &[("x", 0, 7, 2), ("y", 4, 7, 1), ("z", 8, 7, 1)],
            1,
            1,
            12,
            12,
            &[0; 12],
        ),
        payload(&[("x", 0, 7, 1), ("y", 4, 7, 1)], 1, 1, 12, 12, &[0; 12]),
        payload(
            &[("x", u32::MAX, 8, u32::MAX), ("y", 4, 7, 1), ("z", 8, 7, 1)],
            1,
            1,
            12,
            12,
            &[0; 12],
        ),
    ] {
        assert!(decode_point_cloud2(&data).is_err());
    }
    for length in 0..LITTLE.len() {
        assert!(
            decode_point_cloud2(&LITTLE[..length]).is_err(),
            "accepted truncation at {length}"
        );
    }
}

#[test]
fn float_rgb_uses_bits_even_when_the_packed_value_is_nan() {
    let data = [
        1.0f32.to_le_bytes(),
        2.0f32.to_le_bytes(),
        3.0f32.to_le_bytes(),
        0xffff00ffu32.to_le_bytes(),
    ]
    .concat();
    let fields = [
        ("x", 0, 7, 1),
        ("y", 4, 7, 1),
        ("z", 8, 7, 1),
        ("rgb", 12, 7, 1),
    ];
    let cloud = decode_point_cloud2(&payload(&fields, 1, 1, 16, 16, &data)).unwrap();
    assert_eq!(cloud.attributes.rgba, Some(vec![[255, 0, 255, 255]]));
}

#[test]
fn coordinates_accept_all_jazzy_numeric_point_field_types() {
    for datatype in 1..=8 {
        let number = match datatype {
            1 => vec![(-2i8) as u8],
            2 => vec![2],
            3 => (-2i16).to_le_bytes().to_vec(),
            4 => 2u16.to_le_bytes().to_vec(),
            5 => (-2i32).to_le_bytes().to_vec(),
            6 => 2u32.to_le_bytes().to_vec(),
            7 => 2.5f32.to_le_bytes().to_vec(),
            8 => 2.5f64.to_le_bytes().to_vec(),
            _ => unreachable!(),
        };
        let size = number.len() as u32;
        let data = [number.clone(), number.clone(), number].concat();
        let fields = [
            ("x", 0, datatype, 1),
            ("y", size, datatype, 1),
            ("z", size * 2, datatype, 1),
        ];
        let cloud =
            decode_point_cloud2(&payload(&fields, 1, 1, size * 3, size * 3, &data)).unwrap();
        let expected = if matches!(datatype, 1 | 3 | 5) {
            -2.0
        } else if datatype >= 7 {
            2.5
        } else {
            2.0
        };
        assert_eq!(cloud.points, vec![[expected; 3]]);
    }
}
