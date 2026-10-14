use super::wire::{DecodeError, Reader, push_f64, push_string, push_u32};
use crate::MeasurementTime;

#[derive(Clone, Debug, PartialEq)]
pub struct PathMessage {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    /// Positions in the named ROS frame, in metres; body axes are forward/left/up.
    pub points: Vec<[f64; 3]>,
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
        let pose_frame_id = reader.string()?;
        if !pose_frame_id.is_empty()
            && pose_frame_id.trim_start_matches('/') != frame_id.trim_start_matches('/')
        {
            return Err(DecodeError::UnsupportedFormat(
                "Path poses must share the Path header frame".into(),
            ));
        }
        let x = reader.f64()?;
        let y = reader.f64()?;
        let z = reader.f64()?;
        let _orientation_x = reader.f64()?;
        let _orientation_y = reader.f64()?;
        let _orientation_z = reader.f64()?;
        let _orientation_w = reader.f64()?;
        if !x.is_finite() || !y.is_finite() || !z.is_finite() {
            return Err(DecodeError::InvalidLength);
        }
        points.push([x, y, z]);
    }
    Ok(PathMessage {
        measurement_time,
        frame_id,
        points,
    })
}

pub fn encode_path_cdr(path: &PathMessage) -> Result<Vec<u8>, DecodeError> {
    let seconds = i32::try_from(path.measurement_time.0.div_euclid(1_000_000_000))
        .map_err(|_| DecodeError::InvalidTimestamp)?;
    let nanoseconds = path.measurement_time.0.rem_euclid(1_000_000_000) as u32;
    let mut output = vec![0, 1, 0, 0];
    push_u32(&mut output, seconds as u32);
    push_u32(&mut output, nanoseconds);
    push_string(&mut output, &path.frame_id)?;
    push_u32(
        &mut output,
        u32::try_from(path.points.len()).map_err(|_| DecodeError::InvalidLength)?,
    );
    for [x, y, z] in &path.points {
        if !x.is_finite() || !y.is_finite() || !z.is_finite() {
            return Err(DecodeError::InvalidLength);
        }
        push_u32(&mut output, seconds as u32);
        push_u32(&mut output, nanoseconds);
        push_string(&mut output, &path.frame_id)?;
        for value in [*x, *y, *z, 0.0, 0.0, 0.0, 1.0] {
            push_f64(&mut output, value);
        }
    }
    Ok(output)
}
