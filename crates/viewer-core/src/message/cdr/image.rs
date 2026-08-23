use super::common::{DecodeError, Reader, push_string, push_u32};
use crate::MeasurementTime;
use bytes::Bytes;
use std::ops::Range;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompressedImage {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    pub format: String,
    pub jpeg: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedCompressedImage {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    pub format: String,
    pub jpeg: Bytes,
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

struct CompressedImageParts {
    measurement_time: MeasurementTime,
    frame_id: String,
    format: String,
    jpeg_range: Range<usize>,
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
    let format = normalize_format(&reader.string()?)?;
    let jpeg_length = reader.length()?;
    let jpeg_start = reader.position();
    reader.take(jpeg_length)?;
    let jpeg_range = jpeg_start..reader.position();
    Ok(CompressedImageParts {
        measurement_time: MeasurementTime(measurement),
        frame_id,
        format,
        jpeg_range,
    })
}

pub fn decode_compressed_image(bytes: &[u8]) -> Result<CompressedImage, DecodeError> {
    let parts = compressed_image_parts(bytes)?;
    Ok(CompressedImage {
        measurement_time: parts.measurement_time,
        frame_id: parts.frame_id,
        format: parts.format,
        jpeg: bytes[parts.jpeg_range].to_vec(),
    })
}

/// Decodes metadata while retaining the JPEG as a shared slice of the CDR payload.
pub fn decode_compressed_image_bytes(bytes: Bytes) -> Result<DecodedCompressedImage, DecodeError> {
    let parts = compressed_image_parts(&bytes)?;
    Ok(DecodedCompressedImage {
        measurement_time: parts.measurement_time,
        frame_id: parts.frame_id,
        format: parts.format,
        jpeg: bytes.slice(parts.jpeg_range),
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
        u32::try_from(image.jpeg.len()).map_err(|_| DecodeError::InvalidLength)?,
    );
    output.extend_from_slice(&image.jpeg);
    Ok(output)
}
