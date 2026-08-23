use super::common::{DecodeError, Reader};
use crate::MeasurementTime;

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
