//! Deterministic geometry-only recordings for actual Desktop display comparisons.
use super::BASE;
use anyhow::{Result, ensure};
use mcap::{WriteOptions, Writer, records::MessageHeader};
use std::{collections::BTreeMap, fs::File, io::BufWriter};
use viewer_core::{LaserScan, MeasurementTime, encode_laser_scan_cdr};

fn u32_cdr(out: &mut Vec<u8>, value: u32) {
    while !(out.len() - 4).is_multiple_of(4) {
        out.push(0);
    }
    out.extend_from_slice(&value.to_le_bytes());
}
fn string(out: &mut Vec<u8>, value: &str) {
    u32_cdr(out, (value.len() + 1) as u32);
    out.extend_from_slice(value.as_bytes());
    out.push(0);
}
fn cloud(points: u32, frame: u32) -> Vec<u8> {
    let mut out = vec![0, 1, 0, 0];
    let time = BASE + u64::from(frame) * 100_000_000;
    u32_cdr(&mut out, (time / 1_000_000_000) as u32);
    u32_cdr(&mut out, (time % 1_000_000_000) as u32);
    string(&mut out, "base_scan");
    u32_cdr(&mut out, 1);
    u32_cdr(&mut out, points);
    u32_cdr(&mut out, 5);
    for (name, offset, datatype) in [
        ("x", 0, 7),
        ("y", 4, 7),
        ("z", 8, 7),
        ("rgb", 12, 6),
        ("intensity", 16, 7),
    ] {
        string(&mut out, name);
        u32_cdr(&mut out, offset);
        out.push(datatype);
        u32_cdr(&mut out, 1);
    }
    out.push(0);
    u32_cdr(&mut out, 20);
    u32_cdr(&mut out, points * 20);
    u32_cdr(&mut out, points * 20);
    let side = (points as f32).sqrt().ceil() as u32;
    for i in 0..points {
        let x = (i % side) as f32 / side as f32 * 8.0 - 4.0;
        let y = (i / side) as f32 / side as f32 * 8.0 - 4.0;
        let z = 1.0 + (x + y + frame as f32 * 0.2).sin() * 0.5;
        let color = match (i / side / 8 + frame) % 3 {
            0 => 0xff3020u32,
            1 => 0x20ff40,
            _ => 0x3050ff,
        };
        for value in [x, y, z] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&color.to_le_bytes());
        out.extend_from_slice(&((i % 256) as f32).to_le_bytes());
    }
    out.push(1);
    out
}

pub(super) fn generate(path: &str, kind: &str, points: u32, seconds: u32) -> Result<()> {
    ensure!(
        matches!(kind, "scan" | "cloud"),
        "kind must be scan or cloud"
    );
    ensure!(
        (1..=1_000_000).contains(&points) && (2..=60).contains(&seconds),
        "points 1..1000000, seconds 2..60 required"
    );
    let mut writer = Writer::with_options(
        BufWriter::new(File::create_new(path)?),
        WriteOptions::new()
            .profile("ros2")
            .compression(None)
            .chunk_size(Some(8 * 1024 * 1024)),
    )?;
    let (schema, topic) = if kind == "scan" {
        ("sensor_msgs/msg/LaserScan", "/scan")
    } else {
        ("sensor_msgs/msg/PointCloud2", "/points")
    };
    let definition: &[u8] = if kind == "scan" {
        b"std_msgs/Header header\nfloat32 angle_min\nfloat32 angle_max\nfloat32 angle_increment\nfloat32 time_increment\nfloat32 scan_time\nfloat32 range_min\nfloat32 range_max\nfloat32[] ranges\nfloat32[] intensities\n================================================================================\nMSG: std_msgs/Header\nbuiltin_interfaces/Time stamp\nstring frame_id\n================================================================================\nMSG: builtin_interfaces/Time\nint32 sec\nuint32 nanosec"
    } else {
        include_bytes!("../../../tests/fixtures/point-cloud2/schema.ros2msg")
    };
    let schema_id = writer.add_schema(schema, "ros2msg", definition)?;
    let channel_id = writer.add_channel(schema_id, topic, "cdr", &BTreeMap::new())?;
    for frame in 0..seconds * 10 {
        let log_time = BASE + u64::from(frame) * 100_000_000;
        let payload = match kind {
            "scan" => encode_laser_scan_cdr(&LaserScan {
                measurement_time: MeasurementTime(log_time as i64),
                frame_id: "base_scan".into(),
                angle_min: 0.0,
                angle_increment: std::f32::consts::TAU / points as f32,
                range_min: 0.1,
                range_max: 10.0,
                ranges: (0..points)
                    .map(|i| {
                        3.0 + 0.7
                            * (std::f32::consts::TAU * i as f32 / points as f32 * 3.0
                                + frame as f32 * 0.2)
                                .sin()
                    })
                    .collect(),
            })?,
            _ => cloud(points, frame),
        };
        writer.write_to_known_channel(
            &MessageHeader {
                channel_id,
                sequence: frame,
                log_time,
                publish_time: log_time,
            },
            &payload,
        )?;
    }
    writer.finish()?;
    Ok(())
}
