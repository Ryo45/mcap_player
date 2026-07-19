use anyhow::{Context, Result, anyhow, bail, ensure};
use image::{
    ImageBuffer, Rgb, RgbImage,
    codecs::jpeg::JpegEncoder,
    imageops::{FilterType, resize},
};
use mcap::{MessageStream, Summary, WriteOptions, Writer, records::MessageHeader};
use memmap2::MmapOptions;
use std::{
    collections::BTreeMap,
    fs,
    io::BufWriter,
    path::{Path, PathBuf},
};
use viewer_core::{
    ArrivalTime, CompressedImage, MeasurementTime, TransformBatch, TransformState,
    decode_compressed_image, decode_laser_scan, decode_odometry, decode_path, decode_tf_message,
    encode_compressed_image_cdr,
};

const DEFAULT_OUTPUT: &str = "tests/fixtures/camera-jpeg/camera_front_3s.mcap";
const BASE_TIME: u64 = 1_735_689_600_000_000_000;
const RAW_TOPIC: &str = "/camera/image_raw";
const JPEG_TOPIC: &str = "/camera/front/image/compressed";
const PATH_TOPIC: &str = "/planning/path";
const ODOM_TOPIC: &str = "/odom";
const SCAN_TOPIC: &str = "/scan";
const TF_TOPIC: &str = "/tf";
const TF_STATIC_TOPIC: &str = "/tf_static";
const OUTPUT_WIDTH: u32 = 320;
const OUTPUT_HEIGHT: u32 = 240;
const DEFAULT_DURATION_SECONDS: f64 = 5.0;
const MAX_RAW_BYTES: usize = 256 * 1024 * 1024;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "generate".to_owned());
    match command.as_str() {
        "generate" => {
            let output = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_OUTPUT.to_owned()));
            ensure_no_more_args(args)?;
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            generate(&output)?;
            println!("generated {} (30 JPEG frames)", output.display());
        }
        "convert" => {
            let input = args.next().map(PathBuf::from).ok_or_else(usage)?;
            let output = args.next().map(PathBuf::from).ok_or_else(usage)?;
            let duration = args
                .next()
                .map(|value| value.parse::<f64>())
                .transpose()
                .context("duration must be seconds, for example 5 or 5.5")?
                .unwrap_or(DEFAULT_DURATION_SECONDS);
            ensure_no_more_args(args)?;
            ensure!(
                duration.is_finite() && duration > 0.0,
                "duration must be positive"
            );
            let stats = convert_raw_camera(&input, &output, duration)?;
            println!(
                "converted {} camera + {} path + {} odometry + {} scan + {}/{} dynamic/static TF messages ({:.3} s), {} {} {}x{} -> {} {}x{}\noutput: {}",
                stats.frames,
                stats.path_messages,
                stats.odometry_messages,
                stats.scan_messages,
                stats.tf_messages,
                stats.tf_static_messages,
                stats.duration_ns as f64 / 1_000_000_000.0,
                RAW_TOPIC,
                stats.input_encoding,
                stats.input_width,
                stats.input_height,
                JPEG_TOPIC,
                OUTPUT_WIDTH,
                OUTPUT_HEIGHT,
                output.display()
            );
        }
        "verify" => {
            let input = args.next().map(PathBuf::from).ok_or_else(usage)?;
            ensure_no_more_args(args)?;
            let stats = verify_jpeg_mcap(&input)?;
            println!(
                "verified {} JPEG + {} path + {} odometry + {} scan + {}/{} dynamic/static TF messages ({:.3} s), {}x{}, measurement/arrival differ: {}, base_scan -> base_footprint: {}\ninput: {}",
                stats.frames,
                stats.path_messages,
                stats.odometry_messages,
                stats.scan_messages,
                stats.tf_messages,
                stats.tf_static_messages,
                stats.duration_ns as f64 / 1_000_000_000.0,
                stats.width,
                stats.height,
                stats.distinct_time_domains,
                stats.scan_tf_resolves,
                input.display()
            );
        }
        _ => bail!(usage()),
    }
    Ok(())
}

fn usage() -> anyhow::Error {
    anyhow!(
        "usage:\n  cargo run -p ros-fixture -- generate [output.mcap]\n  cargo run -p ros-fixture -- convert <input.mcap> <output.mcap> [duration-seconds]\n  cargo run -p ros-fixture -- verify <input.mcap>"
    )
}

fn ensure_no_more_args(mut args: impl Iterator<Item = String>) -> Result<()> {
    if args.next().is_some() {
        bail!(usage());
    }
    Ok(())
}

fn generate(output: &PathBuf) -> Result<()> {
    let file = fs::File::create(output).with_context(|| format!("create {}", output.display()))?;
    let options = WriteOptions::new()
        .profile("ros2")
        .library("mcap-player ros-fixture")
        .compression(None)
        .chunk_size(Some(96 * 1024));
    let mut writer = Writer::with_options(BufWriter::new(file), options)?;
    let schema = writer.add_schema(
        "sensor_msgs/msg/CompressedImage",
        "ros2msg",
        b"std_msgs/Header header\nstring format\nuint8[] data\n",
    )?;
    let channel = writer.add_channel(
        schema,
        "/camera/front/image/compressed",
        "cdr",
        &BTreeMap::new(),
    )?;
    for sequence in 0..30_u32 {
        let arrival = BASE_TIME + u64::from(sequence) * 100_000_000;
        // Deliberately differs from arrival time so tests catch timestamp-domain confusion.
        let measurement = arrival - 37_000_000 - u64::from(sequence % 3) * 1_000_000;
        let jpeg = make_jpeg(sequence)?;
        let payload = encode_compressed_image_cdr(&CompressedImage {
            measurement_time: MeasurementTime(i64::try_from(measurement)?),
            frame_id: "camera_front_optical_frame".to_owned(),
            format: "jpeg compressed rgb8".to_owned(),
            jpeg,
        })?;
        writer.write_to_known_channel(
            &MessageHeader {
                channel_id: channel,
                sequence,
                log_time: arrival,
                publish_time: measurement,
            },
            &payload,
        )?;
    }
    writer.finish()?;
    Ok(())
}

fn make_jpeg(frame: u32) -> Result<Vec<u8>> {
    let image = ImageBuffer::from_fn(320, 240, |x, y| {
        let r = ((x + frame * 7) % 256) as u8;
        let g = ((y + frame * 11) % 256) as u8;
        let checker = if (x / 32 + y / 32 + frame).is_multiple_of(2) {
            48
        } else {
            190
        };
        Rgb([r, g, checker])
    });
    let mut bytes = vec![];
    JpegEncoder::new_with_quality(&mut bytes, 82).encode_image(&image)?;
    Ok(bytes)
}

#[derive(Debug)]
struct ConversionStats {
    frames: u32,
    path_messages: u32,
    odometry_messages: u32,
    scan_messages: u32,
    tf_messages: u32,
    tf_static_messages: u32,
    duration_ns: u64,
    input_width: u32,
    input_height: u32,
    input_encoding: String,
}

fn convert_raw_camera(
    input: &Path,
    output: &Path,
    duration_seconds: f64,
) -> Result<ConversionStats> {
    ensure!(input != output, "input and output paths must differ");
    ensure!(
        !output.exists(),
        "refusing to overwrite {}",
        output.display()
    );
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    let input_file = fs::File::open(input).with_context(|| format!("open {}", input.display()))?;
    // SAFETY: the converter keeps the input file open and never mutates it while the map is live.
    let mapped = unsafe { MmapOptions::new().map(&input_file) }
        .with_context(|| format!("map {}", input.display()))?;
    let temp_output = output.with_extension("mcap.part");
    ensure!(
        !temp_output.exists(),
        "temporary output already exists: {}",
        temp_output.display()
    );
    let output_file = fs::File::create(&temp_output)
        .with_context(|| format!("create {}", temp_output.display()))?;

    let result = convert_stream(&mapped, BufWriter::new(output_file), duration_seconds);
    match result {
        Ok(stats) => {
            fs::rename(&temp_output, output).with_context(|| {
                format!(
                    "move completed output {} to {}",
                    temp_output.display(),
                    output.display()
                )
            })?;
            Ok(stats)
        }
        Err(error) => {
            let _ = fs::remove_file(&temp_output);
            Err(error)
        }
    }
}

fn convert_stream(
    input: &[u8],
    output: BufWriter<fs::File>,
    duration_seconds: f64,
) -> Result<ConversionStats> {
    let duration_ns = (duration_seconds * 1_000_000_000.0).round() as u64;
    let options = WriteOptions::new()
        .profile("ros2")
        .library("mcap-player ros-fixture raw-to-jpeg")
        .compression(None)
        .chunk_size(Some(1024 * 1024));
    let mut writer = Writer::with_options(output, options)?;
    let schema = writer.add_schema(
        "sensor_msgs/msg/CompressedImage",
        "ros2msg",
        b"std_msgs/Header header\nstring format\nuint8[] data\n",
    )?;
    let camera_channel = writer.add_channel(schema, JPEG_TOPIC, "cdr", &BTreeMap::new())?;
    let path_schema = writer.add_schema(
        "nav_msgs/msg/Path",
        "ros2msg",
        b"std_msgs/Header header\ngeometry_msgs/PoseStamped[] poses\n",
    )?;
    let path_channel = writer.add_channel(path_schema, PATH_TOPIC, "cdr", &BTreeMap::new())?;
    let odometry_schema = writer.add_schema(
        "nav_msgs/msg/Odometry",
        "ros2msg",
        b"std_msgs/Header header\nstring child_frame_id\ngeometry_msgs/PoseWithCovariance pose\ngeometry_msgs/TwistWithCovariance twist\n",
    )?;
    let odometry_channel =
        writer.add_channel(odometry_schema, ODOM_TOPIC, "cdr", &BTreeMap::new())?;
    let scan_schema = writer.add_schema(
        "sensor_msgs/msg/LaserScan",
        "ros2msg",
        b"std_msgs/Header header\nfloat32 angle_min\nfloat32 angle_max\nfloat32 angle_increment\nfloat32 time_increment\nfloat32 scan_time\nfloat32 range_min\nfloat32 range_max\nfloat32[] ranges\nfloat32[] intensities\n",
    )?;
    let scan_channel = writer.add_channel(scan_schema, SCAN_TOPIC, "cdr", &BTreeMap::new())?;
    let tf_schema = writer.add_schema(
        "tf2_msgs/msg/TFMessage",
        "ros2msg",
        b"geometry_msgs/TransformStamped[] transforms\n",
    )?;
    let tf_channel = writer.add_channel(tf_schema, TF_TOPIC, "cdr", &BTreeMap::new())?;
    let tf_static_schema = writer.add_schema(
        "tf2_msgs/msg/TFMessage",
        "ros2msg",
        b"geometry_msgs/TransformStamped[] transforms\n",
    )?;
    let tf_static_channel =
        writer.add_channel(tf_static_schema, TF_STATIC_TOPIC, "cdr", &BTreeMap::new())?;

    let mut first_arrival = None;
    let mut last_arrival = None;
    let mut frames = 0_u32;
    let mut path_messages = 0_u32;
    let mut odometry_messages = 0_u32;
    let mut scan_messages = 0_u32;
    let mut tf_messages = 0_u32;
    let mut tf_static_messages = 0_u32;
    let mut input_shape = None;
    for message in MessageStream::new(input).context("read MCAP records")? {
        let message = message.context("read MCAP message")?;
        if first_arrival.is_none() {
            if message.channel.topic == TF_STATIC_TOPIC {
                let transforms = decode_tf_message(&message.data)?;
                ensure!(!transforms.is_empty(), "empty static TF message");
                writer.write_to_known_channel(
                    &MessageHeader {
                        channel_id: tf_static_channel,
                        sequence: tf_static_messages,
                        log_time: message.log_time,
                        publish_time: message.publish_time,
                    },
                    &message.data,
                )?;
                tf_static_messages = tf_static_messages
                    .checked_add(1)
                    .context("too many static TF messages")?;
                continue;
            }
            if message.channel.topic != RAW_TOPIC {
                continue;
            }
        }
        let start = *first_arrival.get_or_insert(message.log_time);
        if message.log_time.saturating_sub(start) > duration_ns {
            break;
        }
        if message.channel.topic == ODOM_TOPIC {
            ensure!(
                message.channel.message_encoding == "cdr",
                "{} uses unsupported message encoding {:?}",
                ODOM_TOPIC,
                message.channel.message_encoding
            );
            let odometry = decode_odometry(&message.data)
                .with_context(|| format!("decode odometry at log_time {}", message.log_time))?;
            writer.write_to_known_channel(
                &MessageHeader {
                    channel_id: odometry_channel,
                    sequence: odometry_messages,
                    log_time: message.log_time,
                    publish_time: u64::try_from(odometry.measurement_time.0)
                        .unwrap_or(message.publish_time),
                },
                &message.data,
            )?;
            odometry_messages = odometry_messages
                .checked_add(1)
                .context("too many odometry messages")?;
            last_arrival = Some(message.log_time);
            continue;
        }
        if message.channel.topic == SCAN_TOPIC {
            ensure!(
                message.channel.message_encoding == "cdr",
                "{} uses unsupported message encoding {:?}",
                SCAN_TOPIC,
                message.channel.message_encoding
            );
            let scan = decode_laser_scan(&message.data)
                .with_context(|| format!("decode scan at log_time {}", message.log_time))?;
            writer.write_to_known_channel(
                &MessageHeader {
                    channel_id: scan_channel,
                    sequence: scan_messages,
                    log_time: message.log_time,
                    publish_time: u64::try_from(scan.measurement_time.0)
                        .unwrap_or(message.publish_time),
                },
                &message.data,
            )?;
            scan_messages = scan_messages
                .checked_add(1)
                .context("too many scan messages")?;
            last_arrival = Some(message.log_time);
            continue;
        }
        if message.channel.topic == TF_TOPIC || message.channel.topic == TF_STATIC_TOPIC {
            let transforms = decode_tf_message(&message.data)
                .with_context(|| format!("decode TF at log_time {}", message.log_time))?;
            ensure!(!transforms.is_empty(), "empty TF message");
            let (channel_id, sequence) = if message.channel.topic == TF_STATIC_TOPIC {
                (tf_static_channel, tf_static_messages)
            } else {
                (tf_channel, tf_messages)
            };
            writer.write_to_known_channel(
                &MessageHeader {
                    channel_id,
                    sequence,
                    log_time: message.log_time,
                    publish_time: message.publish_time,
                },
                &message.data,
            )?;
            if message.channel.topic == TF_STATIC_TOPIC {
                tf_static_messages = tf_static_messages
                    .checked_add(1)
                    .context("too many static TF messages")?;
            } else {
                tf_messages = tf_messages
                    .checked_add(1)
                    .context("too many dynamic TF messages")?;
            }
            last_arrival = Some(message.log_time);
            continue;
        }
        if message.channel.topic != RAW_TOPIC {
            continue;
        }
        ensure!(
            message.channel.message_encoding == "cdr",
            "{} uses unsupported message encoding {:?}",
            RAW_TOPIC,
            message.channel.message_encoding
        );
        if let Some(schema) = &message.channel.schema {
            ensure!(
                schema.name == "sensor_msgs/msg/Image",
                "{} has unexpected schema {:?}",
                RAW_TOPIC,
                schema.name
            );
        }
        let raw = decode_raw_image(&message.data)
            .with_context(|| format!("decode raw frame at log_time {}", message.log_time))?;
        let shape = (raw.width, raw.height, raw.encoding.clone());
        if let Some(expected) = &input_shape {
            ensure!(
                expected == &shape,
                "raw image shape changed from {expected:?} to {shape:?}"
            );
        } else {
            input_shape = Some(shape);
        }
        let jpeg = encode_resized_jpeg(&raw)?;
        let measurement = MeasurementTime(raw.measurement_time);
        let payload = encode_compressed_image_cdr(&CompressedImage {
            measurement_time: measurement,
            frame_id: raw.frame_id,
            format: "jpeg compressed rgb8".to_owned(),
            jpeg,
        })?;
        writer.write_to_known_channel(
            &MessageHeader {
                channel_id: camera_channel,
                sequence: frames,
                log_time: message.log_time,
                publish_time: u64::try_from(measurement.0).unwrap_or(message.publish_time),
            },
            &payload,
        )?;
        if frames.is_multiple_of(3) {
            let path_payload = encode_dummy_path(measurement.0, path_messages)?;
            writer.write_to_known_channel(
                &MessageHeader {
                    channel_id: path_channel,
                    sequence: path_messages,
                    log_time: message.log_time,
                    publish_time: u64::try_from(measurement.0).unwrap_or(message.publish_time),
                },
                &path_payload,
            )?;
            path_messages = path_messages
                .checked_add(1)
                .context("too many path messages")?;
        }
        frames = frames.checked_add(1).context("too many output frames")?;
        last_arrival = Some(message.log_time);
    }

    ensure!(frames > 0, "no messages found on {RAW_TOPIC}");
    writer.finish()?;
    let (input_width, input_height, input_encoding) = input_shape.expect("frame count checked");
    Ok(ConversionStats {
        frames,
        path_messages,
        odometry_messages,
        scan_messages,
        tf_messages,
        tf_static_messages,
        duration_ns: last_arrival.expect("frame count checked")
            - first_arrival.expect("frame count checked"),
        input_width,
        input_height,
        input_encoding,
    })
}

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

struct CdrReader<'a> {
    bytes: &'a [u8],
    position: usize,
    endian: Endian,
}

impl<'a> CdrReader<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self> {
        ensure!(bytes.len() >= 4, "truncated CDR encapsulation");
        let endian = match bytes[..2] {
            [0, 0] | [0, 2] => Endian::Big,
            [0, 1] | [0, 3] => Endian::Little,
            _ => bail!("unsupported CDR encapsulation {:02x?}", &bytes[..2]),
        };
        Ok(Self {
            bytes,
            position: 4,
            endian,
        })
    }

    fn align(&mut self, alignment: usize) -> Result<()> {
        let relative = self.position - 4;
        let padding = (alignment - relative % alignment) % alignment;
        self.position = self
            .position
            .checked_add(padding)
            .context("CDR offset overflow")?;
        ensure!(self.position <= self.bytes.len(), "truncated CDR padding");
        Ok(())
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .context("CDR length overflow")?;
        let value = self
            .bytes
            .get(self.position..end)
            .context("truncated CDR field")?;
        self.position = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        self.align(4)?;
        let bytes: [u8; 4] = self.take(4)?.try_into().expect("length checked");
        Ok(match self.endian {
            Endian::Little => u32::from_le_bytes(bytes),
            Endian::Big => u32::from_be_bytes(bytes),
        })
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    fn length(&mut self) -> Result<usize> {
        let length = usize::try_from(self.u32()?).context("CDR length does not fit usize")?;
        ensure!(
            length <= MAX_RAW_BYTES,
            "CDR field is too large: {length} bytes"
        );
        Ok(length)
    }

    fn string(&mut self) -> Result<String> {
        let length = self.length()?;
        ensure!(length > 0, "CDR string has zero length");
        let bytes = self.take(length)?;
        ensure!(bytes.last() == Some(&0), "CDR string is not NUL terminated");
        Ok(std::str::from_utf8(&bytes[..length - 1])?.to_owned())
    }
}

struct RawImage<'a> {
    measurement_time: i64,
    frame_id: String,
    height: u32,
    width: u32,
    encoding: String,
    step: u32,
    data: &'a [u8],
}

fn decode_raw_image(bytes: &[u8]) -> Result<RawImage<'_>> {
    let mut reader = CdrReader::new(bytes)?;
    let seconds = reader.i32()?;
    let nanoseconds = reader.u32()?;
    ensure!(nanoseconds < 1_000_000_000, "invalid header nanoseconds");
    let measurement_time = i64::from(seconds)
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(i64::from(nanoseconds)))
        .context("header timestamp overflow")?;
    let frame_id = reader.string()?;
    let height = reader.u32()?;
    let width = reader.u32()?;
    ensure!(width > 0 && height > 0, "raw image has zero dimensions");
    let encoding = reader.string()?.to_ascii_lowercase();
    let _is_bigendian = reader.u8()?;
    let step = reader.u32()?;
    let data_length = reader.length()?;
    let data = reader.take(data_length)?;
    Ok(RawImage {
        measurement_time,
        frame_id,
        height,
        width,
        encoding,
        step,
        data,
    })
}

fn encode_resized_jpeg(raw: &RawImage<'_>) -> Result<Vec<u8>> {
    let (channels, order) = match raw.encoding.as_str() {
        "rgb8" | "8uc3" => (3_usize, ChannelOrder::Rgb),
        "bgr8" => (3, ChannelOrder::Bgr),
        "rgba8" => (4, ChannelOrder::Rgba),
        "bgra8" => (4, ChannelOrder::Bgra),
        "mono8" | "8uc1" => (1, ChannelOrder::Mono),
        other => bail!("unsupported raw image encoding {other:?}"),
    };
    let width = usize::try_from(raw.width)?;
    let height = usize::try_from(raw.height)?;
    let row_bytes = width
        .checked_mul(channels)
        .context("row byte count overflow")?;
    let step = usize::try_from(raw.step)?;
    ensure!(
        step >= row_bytes,
        "image step {step} is smaller than {row_bytes}"
    );
    let required = step
        .checked_mul(height)
        .context("image byte count overflow")?;
    ensure!(raw.data.len() >= required, "raw image data is truncated");

    let rgb_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .context("RGB buffer size overflow")?;
    let mut rgb = Vec::with_capacity(rgb_len);
    for row in raw.data.chunks(step).take(height) {
        for pixel in row[..row_bytes].chunks_exact(channels) {
            match order {
                ChannelOrder::Rgb | ChannelOrder::Rgba => rgb.extend_from_slice(&pixel[..3]),
                ChannelOrder::Bgr | ChannelOrder::Bgra => {
                    rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
                }
                ChannelOrder::Mono => rgb.extend_from_slice(&[pixel[0]; 3]),
            }
        }
    }
    let image = RgbImage::from_raw(raw.width, raw.height, rgb).context("invalid RGB buffer")?;
    let resized = resize(&image, OUTPUT_WIDTH, OUTPUT_HEIGHT, FilterType::Triangle);
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 82).encode_image(&resized)?;
    Ok(jpeg)
}

enum ChannelOrder {
    Rgb,
    Bgr,
    Rgba,
    Bgra,
    Mono,
}

fn align_cdr_output(output: &mut Vec<u8>, alignment: usize) {
    let relative = output.len() - 4;
    output.resize(
        output.len() + (alignment - relative % alignment) % alignment,
        0,
    );
}

fn push_cdr_u32(output: &mut Vec<u8>, value: u32) {
    align_cdr_output(output, 4);
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_cdr_string(output: &mut Vec<u8>, value: &str) -> Result<()> {
    push_cdr_u32(output, u32::try_from(value.len() + 1)?);
    output.extend_from_slice(value.as_bytes());
    output.push(0);
    Ok(())
}

fn push_cdr_f64(output: &mut Vec<u8>, value: f64) {
    align_cdr_output(output, 8);
    output.extend_from_slice(&value.to_le_bytes());
}

fn encode_dummy_path(measurement_time: i64, sequence: u32) -> Result<Vec<u8>> {
    let seconds = i32::try_from(measurement_time.div_euclid(1_000_000_000))?;
    let nanoseconds = u32::try_from(measurement_time.rem_euclid(1_000_000_000))?;
    let mut output = vec![0, 1, 0, 0];
    push_cdr_u32(&mut output, seconds as u32);
    push_cdr_u32(&mut output, nanoseconds);
    push_cdr_string(&mut output, "base_link")?;
    const POSE_COUNT: u32 = 31;
    push_cdr_u32(&mut output, POSE_COUNT);
    let phase = f64::from(sequence) * 0.08;
    for index in 0..POSE_COUNT {
        push_cdr_u32(&mut output, seconds as u32);
        push_cdr_u32(&mut output, nanoseconds);
        push_cdr_string(&mut output, "base_link")?;
        let forward = f64::from(index) * 0.5;
        let left = 0.9 * ((forward * 0.22 + phase).sin() - phase.sin());
        push_cdr_f64(&mut output, forward);
        push_cdr_f64(&mut output, left);
        push_cdr_f64(&mut output, 0.0);
        push_cdr_f64(&mut output, 0.0);
        push_cdr_f64(&mut output, 0.0);
        push_cdr_f64(&mut output, 0.0);
        push_cdr_f64(&mut output, 1.0);
    }
    Ok(output)
}

#[derive(Debug)]
struct VerificationStats {
    frames: u64,
    path_messages: u64,
    odometry_messages: u64,
    scan_messages: u64,
    tf_messages: u64,
    tf_static_messages: u64,
    duration_ns: u64,
    width: u32,
    height: u32,
    distinct_time_domains: bool,
    scan_tf_resolves: bool,
}

fn verify_jpeg_mcap(input: &Path) -> Result<VerificationStats> {
    let input_file = fs::File::open(input).with_context(|| format!("open {}", input.display()))?;
    // SAFETY: verification keeps the file open and does not mutate it while the map is live.
    let mapped = unsafe { MmapOptions::new().map(&input_file) }
        .with_context(|| format!("map {}", input.display()))?;
    let summary = Summary::read(&mapped)
        .context("read MCAP summary")?
        .context("MCAP has no summary")?;
    let expected_count = summary
        .channels
        .iter()
        .find(|(_, channel)| channel.topic == JPEG_TOPIC)
        .and_then(|(channel_id, _)| {
            summary
                .stats
                .as_ref()?
                .channel_message_counts
                .get(channel_id)
                .copied()
        })
        .context("summary has no camera message count")?;

    let mut frames = 0_u64;
    let mut first_arrival = None;
    let mut last_arrival = None;
    let mut previous_arrival = None;
    let mut dimensions = None;
    let mut distinct_time_domains = false;
    let mut path_messages = 0_u64;
    let mut odometry_messages = 0_u64;
    let mut scan_messages = 0_u64;
    let mut tf_messages = 0_u64;
    let mut tf_static_messages = 0_u64;
    let mut transforms = TransformState::default();
    for message in MessageStream::new(&mapped)? {
        let message = message?;
        if message.channel.topic == PATH_TOPIC {
            let path = decode_path(&message.data)?;
            ensure!(path.points.len() == 31, "dummy path pose count changed");
            ensure!(path.points.first() == Some(&[0.0, 0.0]));
            path_messages += 1;
            continue;
        }
        if message.channel.topic == ODOM_TOPIC {
            let odometry = decode_odometry(&message.data)?;
            ensure!(odometry.frame_id == "odom", "unexpected odometry frame");
            odometry_messages += 1;
            continue;
        }
        if message.channel.topic == SCAN_TOPIC {
            let scan = decode_laser_scan(&message.data)?;
            ensure!(!scan.ranges.is_empty(), "empty laser scan");
            scan_messages += 1;
            continue;
        }
        if message.channel.topic == TF_TOPIC || message.channel.topic == TF_STATIC_TOPIC {
            let decoded = decode_tf_message(&message.data)?;
            ensure!(!decoded.is_empty(), "empty TF message");
            let is_static = message.channel.topic == TF_STATIC_TOPIC;
            transforms.apply(TransformBatch {
                arrival_time: ArrivalTime(
                    i64::try_from(message.log_time).context("TF arrival time exceeds i64")?,
                ),
                is_static,
                transforms: decoded,
            });
            if is_static {
                tf_static_messages += 1;
            } else {
                tf_messages += 1;
            }
            continue;
        }
        if message.channel.topic != JPEG_TOPIC {
            continue;
        }
        if let Some(previous) = previous_arrival {
            ensure!(
                message.log_time >= previous,
                "arrival times are not monotonic"
            );
        }
        let compressed = decode_compressed_image(&message.data)?;
        ensure!(
            compressed.jpeg.starts_with(&[0xff, 0xd8]) && compressed.jpeg.ends_with(&[0xff, 0xd9]),
            "frame {frames} does not have JPEG markers"
        );
        let decoded = image::load_from_memory(&compressed.jpeg)
            .with_context(|| format!("decode JPEG frame {frames}"))?;
        let frame_dimensions = (decoded.width(), decoded.height());
        if let Some(expected) = dimensions {
            ensure!(expected == frame_dimensions, "JPEG dimensions changed");
        } else {
            dimensions = Some(frame_dimensions);
        }
        distinct_time_domains |= compressed.measurement_time.0
            != i64::try_from(message.log_time).context("arrival time exceeds i64")?;
        first_arrival.get_or_insert(message.log_time);
        last_arrival = Some(message.log_time);
        previous_arrival = Some(message.log_time);
        frames += 1;
    }
    ensure!(
        frames == expected_count,
        "summary reports {expected_count} frames but decoded {frames}"
    );
    let (width, height) = dimensions.context("no camera frames found")?;
    Ok(VerificationStats {
        frames,
        path_messages,
        odometry_messages,
        scan_messages,
        tf_messages,
        tf_static_messages,
        duration_ns: last_arrival.expect("dimensions imply frames")
            - first_arrival.expect("dimensions imply frames"),
        width,
        height,
        distinct_time_domains,
        scan_tf_resolves: transforms
            .transform_points("base_scan", "base_footprint", &[[0.0, 0.0, 0.0]])
            .is_some(),
    })
}
