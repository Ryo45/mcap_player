//! Deterministic ROS Camera inputs for cross-platform pixel and seek checks.
use anyhow::Result;
use image::{
    ExtendedColorType, ImageEncoder,
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
};
use mcap::{WriteOptions, Writer, records::MessageHeader};
use std::{collections::BTreeMap, fs::File, io::BufWriter, path::Path};
use viewer_core::{CompressedImage, MeasurementTime, encode_compressed_image_cdr};

const HEADER: &str = "\n================================================================================\nMSG: std_msgs/Header\nbuiltin_interfaces/Time stamp\nstring frame_id\n================================================================================\nMSG: builtin_interfaces/Time\nint32 sec\nuint32 nanosec\n";

pub(super) fn generate(path: &Path) -> Result<()> {
    generate_recording(path, false)
}
pub(super) fn generate_alpha(path: &Path) -> Result<()> {
    generate_recording(path, true)
}

fn generate_recording(path: &Path, transparent: bool) -> Result<()> {
    let mut writer = Writer::with_options(
        BufWriter::new(File::create_new(path)?),
        WriteOptions::new().profile("ros2").chunk_size(Some(65536)),
    )?;
    let raw = writer.add_schema("sensor_msgs/msg/Image", "ros2msg", format!("std_msgs/Header header\nuint32 height\nuint32 width\nstring encoding\nuint8 is_bigendian\nuint32 step\nuint8[] data{HEADER}").as_bytes())?;
    let compressed = writer.add_schema(
        "sensor_msgs/msg/CompressedImage",
        "ros2msg",
        format!("std_msgs/Header header\nstring format\nuint8[] data{HEADER}").as_bytes(),
    )?;
    let mut cameras = Vec::new();
    let encodings: &[&str] = if transparent {
        &["rgba8", "bgra8", "png"]
    } else {
        &["rgb8", "bgr8", "rgba8", "bgra8", "mono8", "png", "jpeg"]
    };
    for &encoding in encodings {
        let schema = if matches!(encoding, "png" | "jpeg") {
            compressed
        } else {
            raw
        };
        let id = writer.add_channel(
            schema,
            &format!("/camera/{encoding}"),
            "cdr",
            &BTreeMap::new(),
        )?;
        cameras.push((encoding, id));
    }
    for sequence in 0..30 {
        let time = 1_735_689_600_000_000_000u64 + u64::from(sequence) * 100_000_000;
        for &(encoding, channel_id) in &cameras {
            let payload = payload(encoding, time, sequence, transparent)?;
            writer.write_to_known_channel(
                &MessageHeader {
                    channel_id,
                    sequence,
                    log_time: time,
                    publish_time: time,
                },
                &payload,
            )?;
        }
    }
    writer.finish()?;
    Ok(())
}

fn u32_field(out: &mut Vec<u8>, n: u32) {
    while !(out.len() - 4).is_multiple_of(4) {
        out.push(0);
    }
    out.extend(n.to_le_bytes());
}
fn string(out: &mut Vec<u8>, s: &str) {
    u32_field(out, s.len() as u32 + 1);
    out.extend(s.as_bytes());
    out.push(0);
}
fn payload(encoding: &str, time: u64, sequence: u32, transparent: bool) -> Result<Vec<u8>> {
    let (width, height) = (320u32, 240u32);
    // Whole-image constant changes every frame; independent of resize/filter choice.
    let rgb = [20 + sequence as u8 * 3, 80, 160];
    let alpha = if transparent {
        [128, 0, 255][sequence as usize % 3]
    } else {
        255
    };
    if matches!(encoding, "png" | "jpeg") {
        let mut data = Vec::new();
        let pixels = if transparent {
            [rgb[0], rgb[1], rgb[2], alpha].repeat((width * height) as usize)
        } else {
            rgb.repeat((width * height) as usize)
        };
        if encoding == "png" {
            PngEncoder::new(&mut data).write_image(
                &pixels,
                width,
                height,
                if transparent {
                    ExtendedColorType::Rgba8
                } else {
                    ExtendedColorType::Rgb8
                },
            )?;
        } else {
            JpegEncoder::new_with_quality(&mut data, 95).encode(
                &pixels,
                width,
                height,
                ExtendedColorType::Rgb8,
            )?;
        }
        return Ok(encode_compressed_image_cdr(&CompressedImage {
            measurement_time: MeasurementTime(time as i64),
            frame_id: "camera_optical".into(),
            format: if transparent {
                "rgba8; png compressed".into()
            } else {
                format!("rgb8; {encoding} compressed rgb8")
            },
            data,
        })?);
    }
    let pixel = match encoding {
        "rgb8" => rgb.to_vec(),
        "bgr8" => vec![rgb[2], rgb[1], rgb[0]],
        "rgba8" => vec![rgb[0], rgb[1], rgb[2], alpha],
        "bgra8" => vec![rgb[2], rgb[1], rgb[0], alpha],
        "mono8" => vec![rgb[0]],
        _ => unreachable!(),
    };
    let step = width * pixel.len() as u32 + 3;
    let mut out = vec![0, 1, 0, 0];
    u32_field(&mut out, (time / 1_000_000_000) as u32);
    u32_field(&mut out, (time % 1_000_000_000) as u32);
    string(&mut out, "camera_optical");
    u32_field(&mut out, height);
    u32_field(&mut out, width);
    string(&mut out, encoding);
    out.push((sequence % 2) as u8);
    u32_field(&mut out, step);
    u32_field(&mut out, step * height);
    for _ in 0..height {
        out.extend(pixel.repeat(width as usize));
        out.extend([222, 222, 222]);
    }
    Ok(out)
}
