//! Headless experiment driver. It intentionally calls production paths without optimizing them.
use anyhow::{Context, Result, bail, ensure};
use mcap::{Compression, WriteOptions, Writer, records::MessageHeader};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::BufWriter,
    path::Path,
    time::{Duration, Instant},
};
use viewer_core::{
    ArrivalTime, CameraId, CompressedImage, DataWindowTimeRange, FeatureRuntime, LaserScan,
    MeasurementTime, PlaybackRequirements, SessionPlan, WorkspaceBindings,
    encode_compressed_image_cdr, encode_laser_scan_cdr,
};
use viewer_mcap::{LatestMessageReader, McapSource};

const BASE: u64 = 1_735_689_600_000_000_000;
const STEP: u64 = 100_000_000;
const CAMERA_COUNT: usize = 4;

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("generate") if args.len() == 7 => generate(
            &args[1],
            &args[2],
            args[3].parse()?,
            &args[4],
            args[5].parse()?,
            &args[6],
        ),
        Some("run") if args.len() == 5 => run(&args[1], &args[2], &args[3], args[4].parse()?),
        _ => bail!(
            "generate PATH none|zstd|lz4 CHUNK_BYTES mixed|grouped SECONDS jpeg|raw|scan\nrun PATH scan|all forward|indexed|seek SEEK_COUNT"
        ),
    }
}

fn generate(
    path: &str,
    compression: &str,
    chunk_bytes: u64,
    layout: &str,
    seconds: u64,
    encoding: &str,
) -> Result<()> {
    ensure!((1..=86_400).contains(&seconds), "seconds must be 1..86400");
    ensure!(
        (4096..=128 * 1024 * 1024).contains(&chunk_bytes),
        "invalid chunk size"
    );
    ensure!(matches!(layout, "mixed" | "grouped"), "invalid layout");
    ensure!(
        matches!(encoding, "jpeg" | "raw" | "scan"),
        "invalid encoding"
    );
    let compression = match compression {
        "none" => None,
        "zstd" => Some(Compression::Zstd),
        "lz4" => Some(Compression::Lz4),
        _ => bail!("invalid compression"),
    };
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)?;
    }
    let mut writer = Writer::with_options(
        BufWriter::new(fs::File::create_new(path)?),
        WriteOptions::new()
            .profile("ros2")
            .compression(compression)
            .compression_level(3)
            .chunk_size(Some(chunk_bytes)),
    )?;
    let scan_schema = writer.add_schema("sensor_msgs/msg/LaserScan", "ros2msg",
        b"std_msgs/Header header\nfloat32 angle_min\nfloat32 angle_max\nfloat32 angle_increment\nfloat32 time_increment\nfloat32 scan_time\nfloat32 range_min\nfloat32 range_max\nfloat32[] ranges\nfloat32[] intensities\n================================================================================\nMSG: std_msgs/Header\nbuiltin_interfaces/Time stamp\nstring frame_id\n================================================================================\nMSG: builtin_interfaces/Time\nint32 sec\nuint32 nanosec")?;
    let scan_id = writer.add_channel(scan_schema, "/scan", "cdr", &BTreeMap::new())?;
    let schema = if encoding == "raw" {
        writer.add_schema("sensor_msgs/msg/Image", "ros2msg", b"std_msgs/Header header\nuint32 height\nuint32 width\nstring encoding\nuint8 is_bigendian\nuint32 step\nuint8[] data\n================================================================================\nMSG: std_msgs/Header\nbuiltin_interfaces/Time stamp\nstring frame_id\n================================================================================\nMSG: builtin_interfaces/Time\nint32 sec\nuint32 nanosec")?
    } else {
        writer.add_schema("sensor_msgs/msg/CompressedImage", "ros2msg", b"std_msgs/Header header\nstring format\nuint8[] data\n================================================================================\nMSG: std_msgs/Header\nbuiltin_interfaces/Time stamp\nstring frame_id\n================================================================================\nMSG: builtin_interfaces/Time\nint32 sec\nuint32 nanosec")?
    };
    let mut camera_ids = Vec::new();
    if encoding != "scan" {
        for i in 0..CAMERA_COUNT {
            camera_ids.push(writer.add_channel(
                schema,
                &format!("/camera/{i}"),
                "cdr",
                &BTreeMap::new(),
            )?);
        }
    }
    // Deterministic textured image; camera/time variants avoid a repeated-payload compression oracle.
    let mut images = Vec::new();
    for variant in 0..32u32 {
        let mut seed = 0x12345678u32 ^ variant;
        let image = image::RgbImage::from_fn(320, 240, |x, y| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed % 32) as u8;
            image::Rgb([
                ((x + variant * 7) % 256) as u8 ^ noise,
                (y % 256) as u8 ^ noise,
                ((x / 2 + y / 2) % 256) as u8 ^ noise,
            ])
        });
        if encoding == "raw" {
            images.push(image.into_raw());
        } else {
            let mut jpeg = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
                .encode_image(&image)?;
            images.push(jpeg);
        }
    }
    for second in 0..seconds {
        let mut records = Vec::new();
        for tick in 0..10u64 {
            let sequence = (second * 10 + tick) as u32;
            let time = BASE + second * 1_000_000_000 + tick * STEP;
            let scan = encode_laser_scan_cdr(&LaserScan {
                measurement_time: MeasurementTime(time as i64),
                frame_id: "base_scan".into(),
                angle_min: 0.0,
                angle_increment: 0.01745,
                range_min: 0.12,
                range_max: 3.5,
                ranges: (0..360)
                    .map(|i| 1.0 + ((i + sequence) % 200) as f32 / 100.0)
                    .collect(),
            })?;
            records.push((scan_id, time, sequence, scan));
            if tick % 2 == 0 {
                for (i, &id) in camera_ids.iter().enumerate() {
                    let bytes = &images[(sequence as usize / 2 + i * 7) % images.len()];
                    let payload = if encoding == "raw" {
                        raw_image(time, bytes)
                    } else {
                        encode_compressed_image_cdr(&CompressedImage {
                            measurement_time: MeasurementTime(time as i64),
                            frame_id: "optical".into(),
                            format: "jpeg".into(),
                            jpeg: bytes.clone(),
                        })?
                    };
                    records.push((id, time, sequence / 2, payload));
                }
            }
        }
        if layout == "grouped" {
            records.sort_by_key(|r| (r.0, r.1));
        }
        let mut previous = None;
        for (id, time, sequence, payload) in records {
            if layout == "grouped" && previous != Some(id) {
                writer.flush()?;
            }
            writer.write_to_known_channel(
                &MessageHeader {
                    channel_id: id,
                    sequence,
                    log_time: time,
                    publish_time: time,
                },
                &payload,
            )?;
            previous = Some(id);
        }
        if layout == "grouped" {
            writer.flush()?;
        }
    }
    writer.finish()?;
    println!(
        "{}",
        json!({"file":path,"file_bytes":fs::metadata(path)?.len(),"seconds":seconds,"layout":layout,"encoding":encoding})
    );
    Ok(())
}

// Minimal standard little-endian CDR Image encoding, only for unselected background traffic.
fn raw_image(time: u64, pixels: &[u8]) -> Vec<u8> {
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
    let mut out = vec![0, 1, 0, 0];
    u32_field(&mut out, (time / 1_000_000_000) as u32);
    u32_field(&mut out, (time % 1_000_000_000) as u32);
    string(&mut out, "optical");
    u32_field(&mut out, 240);
    u32_field(&mut out, 320);
    string(&mut out, "rgb8");
    out.push(0);
    u32_field(&mut out, 960);
    u32_field(&mut out, pixels.len() as u32);
    out.extend(pixels);
    out
}

fn proc_values(file: &str) -> BTreeMap<String, u64> {
    fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((
                name.to_owned(),
                value.split_whitespace().next()?.parse().ok()?,
            ))
        })
        .collect()
}
fn memory() -> Value {
    let p = proc_values("/proc/self/status");
    json!({"rss_kib":p.get("VmRSS"),"hwm_kib":p.get("VmHWM")})
}
fn io(source: &McapSource) -> Value {
    let d = source.read_diagnostics();
    json!({"read_calls":d.file_read_calls,"requested_bytes":d.file_read_bytes,"chunks_expanded":d.chunks_expanded,
        "expanded_bytes":d.expanded_bytes,"decompressed_bytes":d.decompressed_bytes,"source_cache_backing_bytes":d.sequential_cache_backing_bytes})
}

fn run(path: &str, selection: &str, mode: &str, seek_count: usize) -> Result<()> {
    ensure!(matches!(selection, "scan" | "all"), "invalid selection");
    ensure!(
        matches!(
            mode,
            "forward" | "indexed" | "seek" | "seek-cached" | "repeat" | "repeat-cached"
        ),
        "invalid mode"
    );
    ensure!(seek_count <= 10_000, "too many seeks");
    let initial_memory = memory();
    let disk_before = proc_values("/proc/self/io");
    let started = Instant::now();
    let mut source = McapSource::open_file(Path::new(path))?;
    let open_ms = started.elapsed().as_secs_f64() * 1000.0;
    let open_memory = memory();
    let open_io = io(&source);
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_point_cloud();
    if selection == "all"
        && source
            .catalog()
            .streams
            .iter()
            .any(|s| s.schema == "sensor_msgs/msg/CompressedImage")
    {
        requirements.require_all_cameras();
    }
    let bindings: WorkspaceBindings =
        serde_json::from_str(include_str!("../../../config/workspace_bindings.json"))?;
    let plan = SessionPlan::build(source.catalog(), "/camera/0", &requirements, &bindings)?;
    let streams = if selection == "all" {
        source.catalog().streams.iter().map(|s| s.id).collect()
    } else {
        plan.selected_stream_ids()
    };
    let expected_messages: u64 = source
        .catalog()
        .streams
        .iter()
        .filter(|s| streams.contains(&s.id))
        .map(|s| s.timing.message_count.unwrap_or(0))
        .sum();
    source.select_streams(streams.iter().copied())?;
    let summary = source.summary();
    let chunk_count = summary.chunk_indexes.len();
    let largest_chunk = summary
        .chunk_indexes
        .iter()
        .map(|c| c.uncompressed_size)
        .max()
        .unwrap_or(0);
    let total_expanded: u64 = summary
        .chunk_indexes
        .iter()
        .map(|c| c.uncompressed_size)
        .sum();
    let index_bytes: u64 = summary
        .chunk_indexes
        .iter()
        .map(|c| c.message_index_length)
        .sum();
    let mut runtime = FeatureRuntime::new(&plan, true);
    let mut pixels: BTreeMap<CameraId, image::DynamicImage> = BTreeMap::new();
    let mut versions = BTreeMap::new();
    let mut jpeg_decodes = 0u64;
    let range = source.time_range();
    let mut delivered = 0u64;
    let mut payload_bytes = 0u64;
    let mut read_ms = 0.0;
    let mut feature_ms = 0.0;
    let mut jpeg_ms = 0.0;
    let mut samples = Vec::new();
    let workload = Instant::now();
    let mut seek_samples = Vec::new();
    let mut seek_live_memory = None;
    if mode.starts_with("seek") || mode.starts_with("repeat") {
        let mut cached = LatestMessageReader::new(source.fork_reader());
        ensure!(
            !mode.ends_with("cached") || streams.len() == 1,
            "cached comparison requires one stream"
        );
        // Production latest-before query with fresh per-query chunk/index caches. This is the
        // restore read primitive, not full GUI seek (TF history, cursor commit and GPU excluded).
        let mut seed = 0x5eed1234u64;
        for _ in 0..seek_count {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut target = ArrivalTime(
                range.start.0 + (seed % (range.end_exclusive.0 - range.start.0) as u64) as i64,
            );
            if mode.starts_with("repeat") {
                target = ArrivalTime(range.start.0 + (range.end_exclusive.0 - range.start.0) / 2);
            }
            let t = Instant::now();
            let messages = if mode.ends_with("cached") {
                cached
                    .read_at(streams[0], target)?
                    .map(|(_, m)| m)
                    .into_iter()
                    .collect::<Vec<_>>()
            } else {
                source.latest_before(&streams, target)?.messages
            };
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            ensure!(
                messages.iter().all(|m| m.arrival_time <= target),
                "invalid predecessor"
            );
            seek_samples
                .push(json!({"target_ns":target.0.to_string(),"ms":ms,"messages":messages.len()}));
            delivered += messages.len() as u64;
        }
        // Sample before the bounded query cache is dropped; final RSS may be lower afterward.
        seek_live_memory = Some(memory());
    } else {
        let mut start = range.start.0;
        while start < range.end_exclusive.0 {
            let end = (start + 1_000_000_000).min(range.end_exclusive.0);
            let t = Instant::now();
            let messages = if mode == "forward" {
                source.read_through(ArrivalTime(end - 1))?
            } else {
                source
                    .indexed_range(
                        &streams,
                        DataWindowTimeRange::new(ArrivalTime(start), ArrivalTime(end))
                            .context("invalid interval")?,
                    )?
                    .messages
            };
            read_ms += t.elapsed().as_secs_f64() * 1000.0;
            delivered += messages.len() as u64;
            payload_bytes += messages.iter().map(|m| m.payload.len() as u64).sum::<u64>();
            let t = Instant::now();
            runtime.process_messages(Duration::from_secs(1), &messages);
            feature_ms += t.elapsed().as_secs_f64() * 1000.0;
            let t = Instant::now();
            for (&id, frame) in runtime.cameras().state().frames() {
                if versions.get(&id) != Some(&frame.arrival_time) {
                    pixels.insert(
                        id,
                        image::load_from_memory_with_format(&frame.jpeg, image::ImageFormat::Jpeg)?,
                    );
                    versions.insert(id, frame.arrival_time);
                    jpeg_decodes += 1;
                }
            }
            jpeg_ms += t.elapsed().as_secs_f64() * 1000.0;
            drop(messages);
            if samples.len() < 8
                && (start - range.start.0) as f64 / (range.end_exclusive.0 - range.start.0) as f64
                    >= samples.len() as f64 / 8.0
            {
                samples.push(json!({"progress":(end-range.start.0) as f64 / (range.end_exclusive.0-range.start.0) as f64,"memory":memory(),"io":io(&source)}));
            }
            start = end;
        }
        ensure!(
            delivered == expected_messages,
            "delivered {delivered}, expected {expected_messages}"
        );
        ensure!(runtime.counters().errors == 0, "feature decode errors");
    }
    let counters = runtime.counters();
    let disk_after = proc_values("/proc/self/io");
    let camera_bytes: usize = runtime
        .cameras()
        .state()
        .frames()
        .map(|(_, f)| f.jpeg.len())
        .sum();
    let point_count = runtime
        .point_cloud()
        .and_then(|c| c.point_cloud().latest())
        .map_or(0, |p| p.points.len());
    println!(
        "{}",
        json!({"path":path,"selection":selection,"mode":mode,"file_bytes":fs::metadata(path)?.len(),
        "open_ms":open_ms,"workload_ms":workload.elapsed().as_secs_f64()*1000.0,"read_ms":read_ms,"feature_ms":feature_ms,"jpeg_ms":jpeg_ms,
        "delivered_messages":delivered,"expected_messages":expected_messages,"payload_bytes":payload_bytes,
        "feature_decoded":counters.decoded,"feature_dropped":counters.dropped,"unknown_messages":counters.unknown_streams,
        "jpeg_decodes":jpeg_decodes,"retained_camera_logical_bytes":camera_bytes,"retained_pixel_bytes":pixels.values().map(|i| i.as_bytes().len()).sum::<usize>(),
        "retained_scan_points":point_count,"chunk_count":chunk_count,"largest_chunk_bytes":largest_chunk,"total_chunk_bytes":total_expanded,"message_index_bytes":index_bytes,
        "initial_memory":initial_memory,"open_memory":open_memory,"final_memory":memory(),"seek_live_memory":seek_live_memory,"open_io":open_io,"io":io(&source),"samples":samples,"seek_samples":seek_samples,
        "os_storage_read_bytes":disk_after.get("read_bytes").copied().unwrap_or(0).saturating_sub(disk_before.get("read_bytes").copied().unwrap_or(0))})
    );
    Ok(())
}
