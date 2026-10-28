//! ROS 2 vision_msgs 4.x Detection3DArray, plain CDR1. No rendering projection.
use super::wire::{DecodeError, Reader, push_f64, push_string, push_u32};
use crate::MeasurementTime;

pub const MAX_DETECTIONS: usize = 256;
pub const MAX_DETECTION_RESULTS: usize = 8;
pub const MAX_DETECTION_MESSAGE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct MessageHeader {
    pub stamp: MeasurementTime,
    pub frame_id: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Pose3D {
    pub position: [f64; 3],
    pub orientation: [f64; 4],
}
#[derive(Clone, Debug, PartialEq)]
pub struct PoseWithCovariance {
    pub pose: Pose3D,
    pub covariance: [f64; 36],
}
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectHypothesis {
    pub class_id: String,
    pub score: f64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectHypothesisWithPose {
    pub hypothesis: ObjectHypothesis,
    pub pose: PoseWithCovariance,
}
#[derive(Clone, Debug, PartialEq)]
pub struct BoundingBox3D {
    pub center: Pose3D,
    pub size: [f64; 3],
}
#[derive(Clone, Debug, PartialEq)]
pub struct Detection3D {
    pub header: MessageHeader,
    pub results: Vec<ObjectHypothesisWithPose>,
    pub bbox: BoundingBox3D,
    pub id: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Detection3DArray {
    pub header: MessageHeader,
    pub detections: Vec<Detection3D>,
}

fn header(reader: &mut Reader<'_>) -> Result<MessageHeader, DecodeError> {
    let seconds = reader.i32()?;
    let nanos = reader.u32()?;
    if nanos >= 1_000_000_000 {
        return Err(DecodeError::InvalidTimestamp);
    }
    Ok(MessageHeader {
        stamp: MeasurementTime(i64::from(seconds) * 1_000_000_000 + i64::from(nanos)),
        frame_id: reader.string_bounded(256)?,
    })
}
fn array<const N: usize>(reader: &mut Reader<'_>) -> Result<[f64; N], DecodeError> {
    let mut values = [0.; N];
    for v in &mut values {
        *v = reader.f64()?;
        if !v.is_finite() {
            return Err(DecodeError::InvalidLength);
        }
    }
    Ok(values)
}
fn pose(reader: &mut Reader<'_>) -> Result<Pose3D, DecodeError> {
    Ok(Pose3D {
        position: array(reader)?,
        orientation: array(reader)?,
    })
}
fn count(reader: &mut Reader<'_>, minimum: usize, maximum: usize) -> Result<usize, DecodeError> {
    let count = reader.sequence_length(minimum)?;
    if count > maximum {
        return Err(DecodeError::InvalidLength);
    }
    Ok(count)
}
pub fn decode_detection3d_array(bytes: &[u8]) -> Result<Detection3DArray, DecodeError> {
    if bytes.len() > MAX_DETECTION_MESSAGE_BYTES {
        return Err(DecodeError::InvalidLength);
    }
    if !matches!(bytes.get(..2), Some([0, 0] | [0, 1])) {
        return Err(DecodeError::InvalidEncapsulation);
    }
    let mut reader = Reader::new(bytes)?;
    let header = header(&mut reader)?;
    let n = count(&mut reader, 100, MAX_DETECTIONS)?;
    let mut detections = Vec::with_capacity(n);
    for _ in 0..n {
        let detection_header = self::header(&mut reader)?;
        let n = count(&mut reader, 356, MAX_DETECTION_RESULTS)?;
        let mut results = Vec::with_capacity(n);
        for _ in 0..n {
            let class_id = reader.string_bounded(128)?;
            let score = reader.f64()?;
            if !score.is_finite() {
                return Err(DecodeError::InvalidLength);
            }
            results.push(ObjectHypothesisWithPose {
                hypothesis: ObjectHypothesis { class_id, score },
                pose: PoseWithCovariance {
                    pose: pose(&mut reader)?,
                    covariance: array(&mut reader)?,
                },
            });
        }
        let bbox = BoundingBox3D {
            center: pose(&mut reader)?,
            size: array(&mut reader)?,
        };
        if bbox.size.iter().any(|v| *v < 0.) {
            return Err(DecodeError::InvalidLength);
        }
        detections.push(Detection3D {
            header: detection_header,
            results,
            bbox,
            id: reader.string_bounded(256)?,
        });
    }
    if reader.position() != bytes.len() {
        return Err(DecodeError::UnsupportedFormat(
            "Detection3DArray has trailing fields; expected vision_msgs 4.x plain CDR1".into(),
        ));
    }
    Ok(Detection3DArray { header, detections })
}

fn push_header(output: &mut Vec<u8>, header: &MessageHeader) -> Result<(), DecodeError> {
    let seconds = i32::try_from(header.stamp.0.div_euclid(1_000_000_000))
        .map_err(|_| DecodeError::InvalidTimestamp)?;
    push_u32(output, seconds as u32);
    push_u32(output, header.stamp.0.rem_euclid(1_000_000_000) as u32);
    push_string(output, &header.frame_id)
}
fn push_values(output: &mut Vec<u8>, values: &[f64]) {
    for v in values {
        push_f64(output, *v);
    }
}
fn push_pose(output: &mut Vec<u8>, pose: &Pose3D) {
    push_values(output, &pose.position);
    push_values(output, &pose.orientation);
}
/// Fixture/tool encoding. Validate the same complete bounded model as the decoder.
pub fn encode_detection3d_array_cdr(message: &Detection3DArray) -> Result<Vec<u8>, DecodeError> {
    if message.detections.len() > MAX_DETECTIONS || message.header.frame_id.len() > 256 {
        return Err(DecodeError::InvalidLength);
    }
    let mut output = vec![0, 1, 0, 0];
    push_header(&mut output, &message.header)?;
    push_u32(&mut output, message.detections.len() as u32);
    for d in &message.detections {
        if d.results.len() > MAX_DETECTION_RESULTS
            || d.header.frame_id.len() > 256
            || d.id.len() > 256
        {
            return Err(DecodeError::InvalidLength);
        }
        push_header(&mut output, &d.header)?;
        push_u32(&mut output, d.results.len() as u32);
        for result in &d.results {
            if result.hypothesis.class_id.len() > 128 {
                return Err(DecodeError::InvalidLength);
            }
            push_string(&mut output, &result.hypothesis.class_id)?;
            push_f64(&mut output, result.hypothesis.score);
            push_pose(&mut output, &result.pose.pose);
            push_values(&mut output, &result.pose.covariance);
        }
        push_pose(&mut output, &d.bbox.center);
        push_values(&mut output, &d.bbox.size);
        push_string(&mut output, &d.id)?;
    }
    decode_detection3d_array(&output)?;
    Ok(output)
}
