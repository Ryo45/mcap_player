use super::common::{DecodeError, Reader, push_f64, push_string, push_u32};
use crate::MeasurementTime;

#[derive(Clone, Debug, PartialEq)]
pub struct TransformStamped {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    pub child_frame_id: String,
    pub translation: [f64; 3],
    /// Quaternion in ROS x, y, z, w order.
    pub rotation: [f64; 4],
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
