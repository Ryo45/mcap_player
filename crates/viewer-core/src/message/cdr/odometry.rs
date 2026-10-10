use super::common::{DecodeError, Reader, push_f64, push_string, push_u32};
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

pub fn encode_odometry_cdr(odometry: &Odometry) -> Result<Vec<u8>, DecodeError> {
    let seconds = i32::try_from(odometry.measurement_time.0.div_euclid(1_000_000_000))
        .map_err(|_| DecodeError::InvalidTimestamp)?;
    let nanoseconds = odometry.measurement_time.0.rem_euclid(1_000_000_000) as u32;
    if odometry
        .position
        .into_iter()
        .chain(odometry.orientation)
        .chain(odometry.linear_velocity)
        .chain(odometry.angular_velocity)
        .any(|value| !value.is_finite())
    {
        return Err(DecodeError::InvalidLength);
    }

    let mut output = vec![0, 1, 0, 0];
    push_u32(&mut output, seconds as u32);
    push_u32(&mut output, nanoseconds);
    push_string(&mut output, &odometry.frame_id)?;
    push_string(&mut output, &odometry.child_frame_id)?;
    for value in odometry.position.into_iter().chain(odometry.orientation) {
        push_f64(&mut output, value);
    }
    for _ in 0..36 {
        push_f64(&mut output, 0.0);
    }
    for value in odometry
        .linear_velocity
        .into_iter()
        .chain(odometry.angular_velocity)
    {
        push_f64(&mut output, value);
    }
    for _ in 0..36 {
        push_f64(&mut output, 0.0);
    }
    Ok(output)
}
