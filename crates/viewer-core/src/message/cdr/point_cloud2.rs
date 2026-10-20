//! Bounded XYZ materialization of ROS 2 sensor_msgs/msg/PointCloud2 (CDR1).
use super::wire::{DecodeError, Reader};
use crate::{MeasurementTime, PointCloudAttributes};

pub const MAX_POINT_CLOUD2_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_POINT_CLOUD2_POINTS: usize = 1_000_000;
const MAX_FIELDS: usize = 256;

/// Finite physical positions only. Serialized messages remain the source of other fields.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedPointCloud2 {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    /// Metres in the message's ROS frame; no presentation transform or scale.
    pub points: Vec<[f32; 3]>,
    pub attributes: PointCloudAttributes,
}

#[derive(Clone, Copy)]
struct Coordinate {
    offset: usize,
    datatype: u8,
}

fn datatype_size(datatype: u8) -> Result<usize, DecodeError> {
    match datatype {
        1 | 2 => Ok(1),
        3 | 4 => Ok(2),
        5..=7 => Ok(4),
        8 => Ok(8),
        _ => Err(DecodeError::UnsupportedFormat(format!(
            "PointField datatype {datatype}"
        ))),
    }
}

fn boolean(reader: &mut Reader<'_>) -> Result<bool, DecodeError> {
    match reader.take(1)?[0] {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(DecodeError::InvalidLength),
    }
}

impl Coordinate {
    fn read(self, point: &[u8], big_endian: bool) -> f32 {
        // Complete field extents are checked before traversing point data.
        let bytes = &point[self.offset..];
        let bits = |size: usize| {
            let mut raw = [0; 8];
            if big_endian {
                raw[8 - size..].copy_from_slice(&bytes[..size]);
                u64::from_be_bytes(raw)
            } else {
                raw[..size].copy_from_slice(&bytes[..size]);
                u64::from_le_bytes(raw)
            }
        };
        match self.datatype {
            1 => bytes[0] as i8 as f32,
            2 => bytes[0] as f32,
            3 => bits(2) as i16 as f32,
            4 => bits(2) as u16 as f32,
            5 => bits(4) as i32 as f32,
            6 => bits(4) as u32 as f32,
            7 => f32::from_bits(bits(4) as u32),
            8 => f64::from_bits(bits(8)) as f32,
            _ => unreachable!("validated PointField datatype"),
        }
    }
}

pub fn decode_point_cloud2(bytes: &[u8]) -> Result<DecodedPointCloud2, DecodeError> {
    if bytes.len() > MAX_POINT_CLOUD2_MESSAGE_BYTES {
        return Err(DecodeError::InvalidLength);
    }
    let mut reader = Reader::new(bytes)?;
    if !matches!(&bytes[..2], [0, 0] | [0, 1]) {
        return Err(DecodeError::InvalidEncapsulation);
    }
    let seconds = reader.i32()?;
    let nanoseconds = reader.u32()?;
    if nanoseconds >= 1_000_000_000 {
        return Err(DecodeError::InvalidTimestamp);
    }
    let measurement_time =
        MeasurementTime(i64::from(seconds) * 1_000_000_000 + i64::from(nanoseconds));
    let frame_id = reader.string()?;
    let height = reader.u32()? as usize;
    let width = reader.u32()? as usize;
    let point_count = width
        .checked_mul(height)
        .ok_or(DecodeError::InvalidLength)?;
    if point_count > MAX_POINT_CLOUD2_POINTS {
        return Err(DecodeError::InvalidLength);
    }
    let count = reader.sequence_length(13)?;
    if count > MAX_FIELDS {
        return Err(DecodeError::InvalidLength);
    }
    let mut coordinates = [None; 3];
    let mut field_end = 0;
    let mut rgb = None;
    let mut rgba = None;
    let mut intensity = None;
    for _ in 0..count {
        let name = reader.string()?;
        let offset = reader.u32()? as usize;
        let datatype = reader.take(1)?[0];
        let count = reader.u32()? as usize;
        if count == 0 {
            return Err(DecodeError::InvalidLength);
        }
        let end = datatype_size(datatype)?
            .checked_mul(count)
            .and_then(|size| offset.checked_add(size))
            .ok_or(DecodeError::InvalidLength)?;
        field_end = field_end.max(end);
        let attribute = match name.as_str() {
            "rgb" => Some(&mut rgb),
            "rgba" => Some(&mut rgba),
            "intensity" => Some(&mut intensity),
            _ => None,
        };
        if let Some(attribute) = attribute {
            if attribute.is_some()
                || count != 1
                || (name != "intensity" && !matches!(datatype, 6 | 7))
            {
                return Err(DecodeError::UnsupportedFormat(format!(
                    "PointCloud2 {name} must be a unique scalar; rgb/rgba require FLOAT32 or UINT32"
                )));
            }
            *attribute = Some(Coordinate { offset, datatype });
        }
        if let Some(axis) = ["x", "y", "z"].iter().position(|axis| *axis == name) {
            if count != 1 || coordinates[axis].is_some() {
                return Err(DecodeError::UnsupportedFormat(
                    "PointCloud2 requires unique scalar x/y/z fields".into(),
                ));
            }
            coordinates[axis] = Some(Coordinate { offset, datatype });
        }
    }
    // Blob endianness is independent of the enclosing CDR endianness.
    let big_endian = boolean(&mut reader)?;
    let point_step = reader.u32()? as usize;
    let row_step = reader.u32()? as usize;
    let data_length = reader.length()?;
    let data = reader.take(data_length)?;
    let _is_dense = boolean(&mut reader)?;
    if field_end > point_step
        || width.checked_mul(point_step).is_none_or(|n| n > row_step)
        || row_step.checked_mul(height) != Some(data_length)
        || (point_count > 0 && point_step == 0)
    {
        return Err(DecodeError::InvalidLength);
    }
    let mut result = DecodedPointCloud2 {
        measurement_time,
        frame_id,
        points: Vec::new(),
        attributes: PointCloudAttributes {
            rgba: rgba.or(rgb).map(|_| Vec::new()),
            intensity: intensity.map(|_| Vec::new()),
        },
    };
    if point_count == 0 {
        return Ok(result);
    }
    let [Some(x), Some(y), Some(z)] = coordinates else {
        return Err(DecodeError::UnsupportedFormat(
            "PointCloud2 requires x/y/z fields".into(),
        ));
    };
    result.points.reserve(point_count);
    if let Some(colors) = &mut result.attributes.rgba {
        colors.reserve(point_count);
    }
    if let Some(values) = &mut result.attributes.intensity {
        values.reserve(point_count);
    }
    for row in data.chunks_exact(row_step) {
        for point in row[..width * point_step].chunks_exact(point_step) {
            let position = [
                x.read(point, big_endian),
                y.read(point, big_endian),
                z.read(point, big_endian),
            ];
            // Do not trust is_dense; NaN/Inf and f64 values outside f32 range are not drawable.
            if position.iter().all(|v| v.is_finite()) {
                result.points.push(position);
                if let Some(field) = rgba.or(rgb) {
                    let raw: [u8; 4] = point[field.offset..field.offset + 4]
                        .try_into()
                        .expect("validated color field extent");
                    let bits = if big_endian {
                        u32::from_be_bytes(raw)
                    } else {
                        u32::from_le_bytes(raw)
                    };
                    let color = [
                        (bits >> 16) as u8,
                        (bits >> 8) as u8,
                        bits as u8,
                        if rgba.is_some() {
                            (bits >> 24) as u8
                        } else {
                            255
                        },
                    ];
                    result
                        .attributes
                        .rgba
                        .as_mut()
                        .expect("color field selected")
                        .push(color);
                }
                if let Some(field) = intensity {
                    result
                        .attributes
                        .intensity
                        .as_mut()
                        .expect("intensity field selected")
                        .push(field.read(point, big_endian));
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
