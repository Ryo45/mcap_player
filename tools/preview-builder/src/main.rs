use anyhow::{Context, Result, anyhow, bail};
use image::{DynamicImage, codecs::jpeg::JpegEncoder};
use mcap::{MessageStream, Summary};
use memmap2::Mmap;
use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
    time::Instant,
};
use viewer_core::{
    ArrivalTime, CameraId, CameraPreviewFrame, PreviewBuildInfo, PreviewImageEncoding,
    SignalBucket, SignalFidelity, SignalId, SignalOverview, TimedPosition2,
    decode_compressed_image, decode_odometry,
};
use viewer_preview_mcap::{PreviewMcapWriter, source_fingerprint};

const CAMERA_BUCKET_NS: i64 = 1_000_000_000;
const SPEED_BUCKET_NS: i64 = 100_000_000;
const TRAJECTORY_INTERVAL_NS: i64 = 500_000_000;

struct Options {
    input: PathBuf,
    output: PathBuf,
    force: bool,
    jpeg_quality: u8,
    primary_camera_topic: String,
}

#[derive(Default)]
struct Report {
    input_messages: u64,
    detected_cameras: usize,
    written_camera_frames: u64,
    skipped_camera_frames: u64,
    written_signal_buckets: u64,
    written_trajectory_points: u64,
}

struct PendingCamera {
    bucket: i64,
    arrival: ArrivalTime,
    image: viewer_core::CompressedImage,
}

#[derive(Clone, Copy)]
struct SpeedAccumulator {
    bucket: i64,
    first: f64,
    last: f64,
    min: f64,
    max: f64,
    count: u32,
}

fn main() -> Result<()> {
    let options = parse_options()?;
    if options.output.exists() && !options.force {
        bail!(
            "{} already exists; pass --force to overwrite it",
            options.output.display()
        );
    }
    let started = Instant::now();
    let input_file =
        File::open(&options.input).with_context(|| format!("open {}", options.input.display()))?;
    // SAFETY: this read-only mapping lives no longer than the opened file, and this process never
    // writes the input path. Mutating an input MCAP concurrently with generation is unsupported.
    let mapped = unsafe { Mmap::map(&input_file) }
        .with_context(|| format!("memory-map {}", options.input.display()))?;
    let fingerprint = source_fingerprint(&mapped)?;
    let summary =
        Summary::read(&mapped)?.ok_or_else(|| anyhow!("input MCAP must contain a summary"))?;
    let camera_ids = camera_channel_ids(&summary, &options.primary_camera_topic);
    let mut report = Report {
        detected_cameras: camera_ids.len(),
        ..Report::default()
    };

    let output_file = File::create(&options.output)
        .with_context(|| format!("create {}", options.output.display()))?;
    let build_info = PreviewBuildInfo::new(
        "preview-builder",
        env!("CARGO_PKG_VERSION"),
        fingerprint.clone(),
    )?;
    let mut writer = PreviewMcapWriter::new(BufWriter::new(output_file), &build_info)?;
    let mut pending_cameras = BTreeMap::<CameraId, PendingCamera>::new();
    let mut speed = None;
    let mut last_trajectory_time = None;

    for message in MessageStream::new(&mapped)? {
        let message = message?;
        report.input_messages += 1;
        let arrival =
            ArrivalTime(i64::try_from(message.log_time).context("message timestamp exceeds i64")?);
        if let Some(camera_id) = camera_ids.get(&message.channel.id).copied() {
            match decode_compressed_image(&message.data) {
                Ok(image) => {
                    let bucket = arrival.0.div_euclid(CAMERA_BUCKET_NS);
                    if pending_cameras
                        .get(&camera_id)
                        .is_some_and(|pending| pending.bucket != bucket)
                    {
                        let pending = pending_cameras.remove(&camera_id).expect("entry checked");
                        flush_camera(
                            &mut writer,
                            camera_id,
                            pending,
                            options.jpeg_quality,
                            &mut report,
                        )?;
                    }
                    pending_cameras.insert(
                        camera_id,
                        PendingCamera {
                            bucket,
                            arrival,
                            image,
                        },
                    );
                }
                Err(_) => report.skipped_camera_frames += 1,
            }
        } else if message.channel.topic == viewer_core::ODOM_TOPIC {
            let Ok(odometry) = decode_odometry(&message.data) else {
                continue;
            };
            let [vx, vy, _] = odometry.linear_velocity;
            let value = vx.hypot(vy);
            let bucket = arrival.0.div_euclid(SPEED_BUCKET_NS);
            if speed.is_some_and(|current: SpeedAccumulator| current.bucket != bucket) {
                flush_speed(
                    &mut writer,
                    speed.take().expect("value checked"),
                    &mut report,
                )?;
            }
            match &mut speed {
                Some(current) => {
                    current.last = value;
                    current.min = current.min.min(value);
                    current.max = current.max.max(value);
                    current.count = current.count.saturating_add(1);
                }
                None => {
                    speed = Some(SpeedAccumulator {
                        bucket,
                        first: value,
                        last: value,
                        min: value,
                        max: value,
                        count: 1,
                    });
                }
            }
            if last_trajectory_time.is_none_or(|previous: ArrivalTime| {
                arrival.0 - previous.0 >= TRAJECTORY_INTERVAL_NS
            }) {
                writer.write_trajectory_point(TimedPosition2::new(
                    arrival,
                    [odometry.position[0] as f32, odometry.position[1] as f32],
                )?)?;
                last_trajectory_time = Some(arrival);
                report.written_trajectory_points += 1;
            }
        }
    }
    for (camera_id, pending) in pending_cameras {
        flush_camera(
            &mut writer,
            camera_id,
            pending,
            options.jpeg_quality,
            &mut report,
        )?;
    }
    if let Some(speed) = speed {
        flush_speed(&mut writer, speed, &mut report)?;
    }
    writer.finish()?;

    println!("Input file size: {} bytes", mapped.len());
    println!(
        "Source fingerprint: {}:{}",
        fingerprint.algorithm(),
        fingerprint.value()
    );
    println!("Input message count: {}", report.input_messages);
    println!("Detected Camera count: {}", report.detected_cameras);
    println!(
        "Written Camera preview frames: {}",
        report.written_camera_frames
    );
    println!("Skipped Camera frames: {}", report.skipped_camera_frames);
    println!("Written Signal buckets: {}", report.written_signal_buckets);
    println!(
        "Written Trajectory points: {}",
        report.written_trajectory_points
    );
    println!(
        "Output file size: {} bytes",
        fs::metadata(&options.output)?.len()
    );
    println!(
        "Processing duration: {:.3}s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn camera_channel_ids(summary: &Summary, primary_topic: &str) -> BTreeMap<u16, CameraId> {
    let mut channels: Vec<_> = summary
        .channels
        .values()
        .filter(|channel| {
            channel
                .schema
                .as_ref()
                .is_some_and(|schema| schema.name == "sensor_msgs/msg/CompressedImage")
        })
        .collect();
    channels.sort_by_key(|channel| channel.id);
    if let Some(index) = channels
        .iter()
        .position(|channel| channel.topic == primary_topic)
    {
        channels.swap(0, index);
    }
    channels
        .into_iter()
        .enumerate()
        .map(|(index, channel)| (channel.id, CameraId(index as u16)))
        .collect()
}

fn flush_camera<W: std::io::Write + std::io::Seek>(
    writer: &mut PreviewMcapWriter<W>,
    camera_id: CameraId,
    pending: PendingCamera,
    quality: u8,
    report: &mut Report,
) -> Result<()> {
    let decoded = match image::load_from_memory(&pending.image.jpeg) {
        Ok(decoded) => decoded,
        Err(_) => {
            report.skipped_camera_frames += 1;
            return Ok(());
        }
    };
    let thumbnail = thumbnail(decoded);
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, quality).encode_image(&thumbnail)?;
    if jpeg.is_empty() {
        report.skipped_camera_frames += 1;
        return Ok(());
    }
    writer.write_camera_frame(&CameraPreviewFrame::new(
        camera_id,
        Some(pending.image.measurement_time),
        pending.arrival,
        pending.image.frame_id,
        PreviewImageEncoding::Jpeg,
        thumbnail.width(),
        thumbnail.height(),
        jpeg,
    )?)?;
    report.written_camera_frames += 1;
    Ok(())
}

fn thumbnail(image: DynamicImage) -> DynamicImage {
    if image.width() <= 320 && image.height() <= 180 {
        image
    } else {
        image.thumbnail(320, 180)
    }
}

fn flush_speed<W: std::io::Write + std::io::Seek>(
    writer: &mut PreviewMcapWriter<W>,
    value: SpeedAccumulator,
    report: &mut Report,
) -> Result<()> {
    let start = value.bucket * SPEED_BUCKET_NS;
    let bucket = SignalBucket::new(
        ArrivalTime(start),
        ArrivalTime(start + SPEED_BUCKET_NS),
        value.first,
        value.last,
        value.min,
        value.max,
        value.count,
    )?;
    writer.write_signal_overview(&SignalOverview::new(
        SignalId::Speed,
        SignalFidelity::Envelope {
            bucket_ns: SPEED_BUCKET_NS,
        },
        vec![bucket],
    )?)?;
    report.written_signal_buckets += 1;
    Ok(())
}

fn parse_options() -> Result<Options> {
    let mut args = env::args_os().skip(1);
    let input = args.next().map(PathBuf::from).ok_or_else(|| {
        anyhow!("usage: preview-builder INPUT [--output PATH] [--force] [--jpeg-quality 1..100]")
    })?;
    let mut output = None;
    let mut force = false;
    let mut jpeg_quality = 72_u8;
    let mut primary_camera_topic = "/camera/front/image/compressed".to_owned();
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--output") => {
                output = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--output requires a path"))?,
                ));
            }
            Some("--force") => force = true,
            Some("--camera-topic") => {
                primary_camera_topic = args
                    .next()
                    .ok_or_else(|| anyhow!("--camera-topic requires a topic"))?
                    .into_string()
                    .map_err(|_| anyhow!("camera topic must be UTF-8"))?;
            }
            Some("--jpeg-quality") => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--jpeg-quality requires a value"))?;
                jpeg_quality = value
                    .to_str()
                    .ok_or_else(|| anyhow!("JPEG quality must be UTF-8"))?
                    .parse()?;
                if !(1..=100).contains(&jpeg_quality) {
                    bail!("JPEG quality must be between 1 and 100");
                }
            }
            _ => bail!("unknown option: {}", Path::new(&argument).display()),
        }
    }
    let output = output.unwrap_or_else(|| {
        input
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("preview.mcap")
    });
    Ok(Options {
        input,
        output,
        force,
        jpeg_quality,
        primary_camera_topic,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_never_upscales_and_fits_bounds() {
        let small = thumbnail(DynamicImage::new_rgb8(20, 10));
        assert_eq!((small.width(), small.height()), (20, 10));
        let large = thumbnail(DynamicImage::new_rgb8(1920, 1080));
        assert!(large.width() <= 320 && large.height() <= 180);
    }
}
