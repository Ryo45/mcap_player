use super::common::{DecodeError, Reader};
use crate::MeasurementTime;

#[derive(Clone, Debug, PartialEq)]
pub struct PathMessage {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    /// ROS coordinates in metres: +x forward, +y left.
    pub points: Vec<[f64; 2]>,
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
