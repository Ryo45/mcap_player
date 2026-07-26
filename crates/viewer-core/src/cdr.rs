use crate::MeasurementTime;
use std::fmt;

const MAX_FIELD_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompressedImage {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    pub format: String,
    pub jpeg: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PathMessage {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    /// ROS coordinates in metres: +x forward, +y left.
    pub points: Vec<[f64; 2]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Odometry {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    pub child_frame_id: String,
    pub position: [f64; 3],
    /// Quaternion in ROS x, y, z, w order.
    pub orientation: [f64; 4],
    pub linear_velocity: [f64; 3],
    pub angular_velocity: [f64; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct LaserScan {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    pub angle_min: f32,
    pub angle_increment: f32,
    pub range_min: f32,
    pub range_max: f32,
    pub ranges: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransformStamped {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    pub child_frame_id: String,
    pub translation: [f64; 3],
    /// Quaternion in ROS x, y, z, w order.
    pub rotation: [f64; 4],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Truncated,
    InvalidEncapsulation,
    InvalidLength,
    InvalidUtf8,
    InvalidTimestamp,
    UnsupportedFormat(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "truncated CDR payload"),
            Self::InvalidEncapsulation => write!(f, "unsupported CDR encapsulation"),
            Self::InvalidLength => write!(f, "invalid or excessive CDR field length"),
            Self::InvalidUtf8 => write!(f, "CDR string is not UTF-8"),
            Self::InvalidTimestamp => write!(f, "invalid ROS timestamp"),
            Self::UnsupportedFormat(value) => write!(f, "unsupported image format: {value}"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    base: usize,
    endian: Endian,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        if bytes.len() < 4 {
            return Err(DecodeError::Truncated);
        }
        let endian = match bytes[0..2] {
            [0, 0] | [0, 2] => Endian::Big,
            [0, 1] | [0, 3] => Endian::Little,
            _ => return Err(DecodeError::InvalidEncapsulation),
        };
        Ok(Self {
            bytes,
            position: 4,
            base: 4,
            endian,
        })
    }

    fn align(&mut self, alignment: usize) -> Result<(), DecodeError> {
        let relative = self.position - self.base;
        let padding = (alignment - relative % alignment) % alignment;
        self.position = self
            .position
            .checked_add(padding)
            .ok_or(DecodeError::InvalidLength)?;
        if self.position > self.bytes.len() {
            return Err(DecodeError::Truncated);
        }
        Ok(())
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(DecodeError::InvalidLength)?;
        let result = self
            .bytes
            .get(self.position..end)
            .ok_or(DecodeError::Truncated)?;
        self.position = end;
        Ok(result)
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        self.align(4)?;
        let raw: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| DecodeError::Truncated)?;
        Ok(match self.endian {
            Endian::Little => u32::from_le_bytes(raw),
            Endian::Big => u32::from_be_bytes(raw),
        })
    }

    fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(self.u32()? as i32)
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        self.align(8)?;
        let raw: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| DecodeError::Truncated)?;
        Ok(match self.endian {
            Endian::Little => u64::from_le_bytes(raw),
            Endian::Big => u64::from_be_bytes(raw),
        })
    }

    fn f64(&mut self) -> Result<f64, DecodeError> {
        Ok(f64::from_bits(self.u64()?))
    }

    fn f32(&mut self) -> Result<f32, DecodeError> {
        Ok(f32::from_bits(self.u32()?))
    }

    fn length(&mut self) -> Result<usize, DecodeError> {
        let length = usize::try_from(self.u32()?).map_err(|_| DecodeError::InvalidLength)?;
        if length > MAX_FIELD_BYTES {
            return Err(DecodeError::InvalidLength);
        }
        Ok(length)
    }

    fn sequence_length(&mut self, minimum_element_bytes: usize) -> Result<usize, DecodeError> {
        let length = self.length()?;
        let remaining = self.bytes.len().saturating_sub(self.position);
        if length > remaining / minimum_element_bytes {
            return Err(DecodeError::InvalidLength);
        }
        Ok(length)
    }

    fn string(&mut self) -> Result<String, DecodeError> {
        let length = self.length()?;
        if length == 0 {
            return Err(DecodeError::InvalidLength);
        }
        let bytes = self.take(length)?;
        if bytes.last() != Some(&0) {
            return Err(DecodeError::InvalidLength);
        }
        std::str::from_utf8(&bytes[..length - 1])
            .map(str::to_owned)
            .map_err(|_| DecodeError::InvalidUtf8)
    }
}

fn normalize_format(value: &str) -> Result<String, DecodeError> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized
        .split(|character: char| character == ';' || character.is_whitespace())
        .any(|token| matches!(token, "jpeg" | "jpg"))
    {
        Ok("jpeg".to_owned())
    } else {
        Err(DecodeError::UnsupportedFormat(value.to_owned()))
    }
}

pub fn decode_compressed_image(bytes: &[u8]) -> Result<CompressedImage, DecodeError> {
    let mut reader = Reader::new(bytes)?;
    let seconds = reader.i32()?;
    let nanoseconds = reader.u32()?;
    if nanoseconds >= 1_000_000_000 {
        return Err(DecodeError::InvalidTimestamp);
    }
    let measurement = i64::from(seconds)
        .checked_mul(1_000_000_000)
        .and_then(|v| v.checked_add(i64::from(nanoseconds)))
        .ok_or(DecodeError::InvalidTimestamp)?;
    let frame_id = reader.string()?;
    let format = normalize_format(&reader.string()?)?;
    let jpeg_length = reader.length()?;
    let jpeg = reader.take(jpeg_length)?.to_vec();
    Ok(CompressedImage {
        measurement_time: MeasurementTime(measurement),
        frame_id,
        format,
        jpeg,
    })
}

pub fn decode_path(bytes: &[u8]) -> Result<PathMessage, DecodeError> {
    let mut reader = Reader::new(bytes)?;
    let seconds = reader.i32()?;
    let nanoseconds = reader.u32()?;
    if nanoseconds >= 1_000_000_000 {
        return Err(DecodeError::InvalidTimestamp);
    }
    let measurement_time = MeasurementTime(
        i64::from(seconds)
            .checked_mul(1_000_000_000)
            .and_then(|value| value.checked_add(i64::from(nanoseconds)))
            .ok_or(DecodeError::InvalidTimestamp)?,
    );
    let frame_id = reader.string()?;
    let pose_count = reader.sequence_length(64)?;
    let mut points = Vec::with_capacity(pose_count);
    for _ in 0..pose_count {
        let _pose_seconds = reader.i32()?;
        let pose_nanoseconds = reader.u32()?;
        if pose_nanoseconds >= 1_000_000_000 {
            return Err(DecodeError::InvalidTimestamp);
        }
        let _pose_frame_id = reader.string()?;
        let x = reader.f64()?;
        let y = reader.f64()?;
        let _z = reader.f64()?;
        let _orientation_x = reader.f64()?;
        let _orientation_y = reader.f64()?;
        let _orientation_z = reader.f64()?;
        let _orientation_w = reader.f64()?;
        if !x.is_finite() || !y.is_finite() {
            return Err(DecodeError::InvalidLength);
        }
        points.push([x, y]);
    }
    Ok(PathMessage {
        measurement_time,
        frame_id,
        points,
    })
}

pub fn decode_odometry(bytes: &[u8]) -> Result<Odometry, DecodeError> {
    let mut reader = Reader::new(bytes)?;
    let seconds = reader.i32()?;
    let nanoseconds = reader.u32()?;
    if nanoseconds >= 1_000_000_000 {
        return Err(DecodeError::InvalidTimestamp);
    }
    let measurement_time = MeasurementTime(
        i64::from(seconds)
            .checked_mul(1_000_000_000)
            .and_then(|value| value.checked_add(i64::from(nanoseconds)))
            .ok_or(DecodeError::InvalidTimestamp)?,
    );
    let frame_id = reader.string()?;
    let child_frame_id = reader.string()?;
    let position = [reader.f64()?, reader.f64()?, reader.f64()?];
    let orientation = [reader.f64()?, reader.f64()?, reader.f64()?, reader.f64()?];
    for _ in 0..36 {
        let _ = reader.f64()?;
    }
    let linear_velocity = [reader.f64()?, reader.f64()?, reader.f64()?];
    let angular_velocity = [reader.f64()?, reader.f64()?, reader.f64()?];
    for _ in 0..36 {
        let _ = reader.f64()?;
    }
    if position
        .into_iter()
        .chain(orientation)
        .chain(linear_velocity)
        .chain(angular_velocity)
        .any(|value| !value.is_finite())
    {
        return Err(DecodeError::InvalidLength);
    }
    Ok(Odometry {
        measurement_time,
        frame_id,
        child_frame_id,
        position,
        orientation,
        linear_velocity,
        angular_velocity,
    })
}

pub fn decode_laser_scan(bytes: &[u8]) -> Result<LaserScan, DecodeError> {
    let mut reader = Reader::new(bytes)?;
    let seconds = reader.i32()?;
    let nanoseconds = reader.u32()?;
    if nanoseconds >= 1_000_000_000 {
        return Err(DecodeError::InvalidTimestamp);
    }
    let measurement_time = MeasurementTime(
        i64::from(seconds)
            .checked_mul(1_000_000_000)
            .and_then(|value| value.checked_add(i64::from(nanoseconds)))
            .ok_or(DecodeError::InvalidTimestamp)?,
    );
    let frame_id = reader.string()?;
    let angle_min = reader.f32()?;
    let _angle_max = reader.f32()?;
    let angle_increment = reader.f32()?;
    let _time_increment = reader.f32()?;
    let _scan_time = reader.f32()?;
    let range_min = reader.f32()?;
    let range_max = reader.f32()?;
    let range_count = reader.sequence_length(std::mem::size_of::<f32>())?;
    let mut ranges = Vec::with_capacity(range_count);
    for _ in 0..range_count {
        ranges.push(reader.f32()?);
    }
    let intensity_count = reader.sequence_length(std::mem::size_of::<f32>())?;
    for _ in 0..intensity_count {
        let _ = reader.f32()?;
    }
    if !angle_min.is_finite()
        || !angle_increment.is_finite()
        || angle_increment == 0.0
        || !range_min.is_finite()
        || !range_max.is_finite()
        || range_min < 0.0
        || range_max < range_min
    {
        return Err(DecodeError::InvalidLength);
    }
    Ok(LaserScan {
        measurement_time,
        frame_id,
        angle_min,
        angle_increment,
        range_min,
        range_max,
        ranges,
    })
}

pub fn decode_tf_message(bytes: &[u8]) -> Result<Vec<TransformStamped>, DecodeError> {
    let mut reader = Reader::new(bytes)?;
    let count = reader.sequence_length(64)?;
    if count > 1_000_000 {
        return Err(DecodeError::InvalidLength);
    }
    let mut transforms = Vec::with_capacity(count);
    for _ in 0..count {
        let seconds = reader.i32()?;
        let nanoseconds = reader.u32()?;
        if nanoseconds >= 1_000_000_000 {
            return Err(DecodeError::InvalidTimestamp);
        }
        let measurement_time = MeasurementTime(
            i64::from(seconds)
                .checked_mul(1_000_000_000)
                .and_then(|value| value.checked_add(i64::from(nanoseconds)))
                .ok_or(DecodeError::InvalidTimestamp)?,
        );
        let frame_id = reader.string()?;
        let child_frame_id = reader.string()?;
        let translation = [reader.f64()?, reader.f64()?, reader.f64()?];
        let rotation = [reader.f64()?, reader.f64()?, reader.f64()?, reader.f64()?];
        if translation
            .into_iter()
            .chain(rotation)
            .any(|value| !value.is_finite())
        {
            return Err(DecodeError::InvalidLength);
        }
        transforms.push(TransformStamped {
            measurement_time,
            frame_id,
            child_frame_id,
            translation,
            rotation,
        });
    }
    Ok(transforms)
}

fn align_output(output: &mut Vec<u8>, alignment: usize) {
    let relative = output.len() - 4;
    output.resize(
        output.len() + (alignment - relative % alignment) % alignment,
        0,
    );
}

fn push_u32(output: &mut Vec<u8>, value: u32) {
    align_output(output, 4);
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_string(output: &mut Vec<u8>, value: &str) -> Result<(), DecodeError> {
    let length = value
        .len()
        .checked_add(1)
        .ok_or(DecodeError::InvalidLength)?;
    push_u32(
        output,
        u32::try_from(length).map_err(|_| DecodeError::InvalidLength)?,
    );
    output.extend_from_slice(value.as_bytes());
    output.push(0);
    Ok(())
}

fn push_f64(output: &mut Vec<u8>, value: f64) {
    align_output(output, 8);
    output.extend_from_slice(&value.to_le_bytes());
}

pub fn encode_compressed_image_cdr(image: &CompressedImage) -> Result<Vec<u8>, DecodeError> {
    let seconds = image.measurement_time.0.div_euclid(1_000_000_000);
    let nanos = image.measurement_time.0.rem_euclid(1_000_000_000);
    let seconds = i32::try_from(seconds).map_err(|_| DecodeError::InvalidTimestamp)?;
    let mut output = vec![0, 1, 0, 0];
    push_u32(&mut output, seconds as u32);
    push_u32(&mut output, nanos as u32);
    push_string(&mut output, &image.frame_id)?;
    push_string(&mut output, &image.format)?;
    push_u32(
        &mut output,
        u32::try_from(image.jpeg.len()).map_err(|_| DecodeError::InvalidLength)?,
    );
    output.extend_from_slice(&image.jpeg);
    Ok(output)
}

pub fn encode_tf_message_cdr(transforms: &[TransformStamped]) -> Result<Vec<u8>, DecodeError> {
    let mut output = vec![0, 1, 0, 0];
    push_u32(
        &mut output,
        u32::try_from(transforms.len()).map_err(|_| DecodeError::InvalidLength)?,
    );
    for transform in transforms {
        let seconds = transform.measurement_time.0.div_euclid(1_000_000_000);
        let nanos = transform.measurement_time.0.rem_euclid(1_000_000_000);
        let seconds = i32::try_from(seconds).map_err(|_| DecodeError::InvalidTimestamp)?;
        push_u32(&mut output, seconds as u32);
        push_u32(&mut output, nanos as u32);
        push_string(&mut output, &transform.frame_id)?;
        push_string(&mut output, &transform.child_frame_id)?;
        for value in transform.translation {
            if !value.is_finite() {
                return Err(DecodeError::InvalidLength);
            }
            push_f64(&mut output, value);
        }
        for value in transform.rotation {
            if !value.is_finite() {
                return Err(DecodeError::InvalidLength);
            }
            push_f64(&mut output, value);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 5, b'j', b'p',
            b'e', b'g', 0, 0, 0, 0, 0, 0, 0, 0,
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
}
