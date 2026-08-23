use super::common::{DecodeError, Reader};
use crate::MeasurementTime;

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
