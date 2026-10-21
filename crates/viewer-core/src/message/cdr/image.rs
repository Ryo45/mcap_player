use super::wire::{DecodeError, Reader, push_string, push_u32};
use crate::MeasurementTime;
use bytes::Bytes;
use std::ops::Range;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompressedImage {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    pub format: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedCompressedImage {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    pub format: String,
    pub data: Bytes,
}

fn normalize_format(value: &str) -> Result<String, DecodeError> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.contains("compresseddepth") {
        return Err(DecodeError::UnsupportedFormat(value.to_owned()));
    }
    if normalized.is_empty() {
        return Ok("jpeg".into());
    }
    if normalized
        .split(|character: char| character == ';' || character.is_whitespace())
        .any(|token| matches!(token, "jpeg" | "jpg"))
    {
        Ok("jpeg".to_owned())
    } else if normalized
        .split(|c: char| c == ';' || c.is_whitespace())
        .any(|t| t == "png")
    {
        Ok("png".into())
    } else {
        Err(DecodeError::UnsupportedFormat(value.to_owned()))
    }
}

struct CompressedImageParts {
    measurement_time: MeasurementTime,
    frame_id: String,
    format: String,
    data_range: Range<usize>,
}

fn compressed_image_parts(bytes: &[u8]) -> Result<CompressedImageParts, DecodeError> {
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
    if frame_id.len() > 4096 {
        return Err(DecodeError::InvalidLength);
    }
    let format = normalize_format(&reader.string()?)?;
    let data_length = reader.length()?;
    if data_length > 32 * 1024 * 1024 {
        return Err(DecodeError::InvalidLength);
    }
    let data_start = reader.position();
    reader.take(data_length)?;
    let data_range = data_start..reader.position();
    Ok(CompressedImageParts {
        measurement_time: MeasurementTime(measurement),
        frame_id,
        format,
        data_range,
    })
}

pub fn decode_compressed_image(bytes: &[u8]) -> Result<CompressedImage, DecodeError> {
    let parts = compressed_image_parts(bytes)?;
    Ok(CompressedImage {
        measurement_time: parts.measurement_time,
        frame_id: parts.frame_id,
        format: parts.format,
        data: bytes[parts.data_range].to_vec(),
    })
}

/// Decodes metadata while retaining encoded pixels as a shared slice of the CDR payload.
pub fn decode_compressed_image_bytes(bytes: Bytes) -> Result<DecodedCompressedImage, DecodeError> {
    let parts = compressed_image_parts(&bytes)?;
    Ok(DecodedCompressedImage {
        measurement_time: parts.measurement_time,
        frame_id: parts.frame_id,
        format: parts.format,
        data: bytes.slice(parts.data_range),
    })
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
        u32::try_from(image.data.len()).map_err(|_| DecodeError::InvalidLength)?,
    );
    output.extend_from_slice(&image.data);
    Ok(output)
}

/// Decodes sensor_msgs/msg/Image without converting or copying its pixels.
pub fn decode_raw_image_bytes(bytes: Bytes) -> Result<DecodedRawImage, DecodeError> {
    use crate::{RawImageEncoding, RawImageLayout};
    let mut r = Reader::new(&bytes)?;
    let seconds = r.i32()?;
    let nanos = r.u32()?;
    if nanos >= 1_000_000_000 {
        return Err(DecodeError::InvalidTimestamp);
    }
    let frame_id = r.string()?;
    if frame_id.len() > 4096 {
        return Err(DecodeError::InvalidLength);
    }
    let height = r.u32()?;
    let width = r.u32()?;
    let encoding = match r.string()?.as_str() {
        "rgb8" => RawImageEncoding::Rgb8,
        "bgr8" => RawImageEncoding::Bgr8,
        "rgba8" => RawImageEncoding::Rgba8,
        "bgra8" => RawImageEncoding::Bgra8,
        "mono8" => RawImageEncoding::Mono8,
        other => return Err(DecodeError::UnsupportedFormat(other.into())),
    };
    if r.take(1)?[0] > 1 {
        return Err(DecodeError::InvalidLength);
    }
    let step = r.u32()?;
    let len = r.length()?;
    let layout = RawImageLayout {
        width,
        height,
        step,
        encoding,
    };
    layout.validate(len)?;
    let start = r.position();
    r.take(len)?;
    Ok(DecodedRawImage {
        measurement_time: MeasurementTime(i64::from(seconds) * 1_000_000_000 + i64::from(nanos)),
        frame_id,
        layout,
        data: bytes.slice(start..start + len),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedRawImage {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    pub layout: crate::RawImageLayout,
    pub data: Bytes,
}
