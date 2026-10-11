//! ROS 2 diagnostic_msgs/msg/DiagnosticArray (CDR1). Levels are producer facts.
use super::common::{DecodeError, Reader};
use crate::MeasurementTime;

pub const MAX_DIAGNOSTIC_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_STATUSES: usize = 4096;
const MAX_VALUES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DiagnosticLevel {
    Ok,
    Warn,
    Error,
    Stale,
    Unknown(u8),
}
impl DiagnosticLevel {
    pub fn from_raw(value: u8) -> Self {
        match value {
            0 => Self::Ok,
            1 => Self::Warn,
            2 => Self::Error,
            3 => Self::Stale,
            other => Self::Unknown(other),
        }
    }
}
impl std::fmt::Display for DiagnosticLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ok => f.write_str("OK"),
            Self::Warn => f.write_str("WARN"),
            Self::Error => f.write_str("ERROR"),
            Self::Stale => f.write_str("STALE"),
            Self::Unknown(value) => write!(f, "UNKNOWN({value})"),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticKeyValue {
    pub key: String,
    pub value: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticStatus {
    pub level: DiagnosticLevel,
    pub name: String,
    pub message: String,
    pub hardware_id: String,
    pub values: Vec<DiagnosticKeyValue>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticArray {
    pub measurement_time: MeasurementTime,
    pub frame_id: String,
    pub statuses: Vec<DiagnosticStatus>,
}

pub fn decode_diagnostic_array(bytes: &[u8]) -> Result<DiagnosticArray, DecodeError> {
    if bytes.len() > MAX_DIAGNOSTIC_MESSAGE_BYTES {
        return Err(DecodeError::InvalidLength);
    }
    let mut reader = Reader::new(bytes)?;
    let seconds = reader.i32()?;
    let nanoseconds = reader.u32()?;
    if nanoseconds >= 1_000_000_000 {
        return Err(DecodeError::InvalidTimestamp);
    }
    let measurement_time =
        MeasurementTime(i64::from(seconds) * 1_000_000_000 + i64::from(nanoseconds));
    let frame_id = reader.string()?;
    let count = reader.sequence_length(20)?;
    if count > MAX_STATUSES {
        return Err(DecodeError::InvalidLength);
    }
    let mut statuses = Vec::with_capacity(count);
    let mut total_values = 0usize;
    for _ in 0..count {
        let level = DiagnosticLevel::from_raw(reader.take(1)?[0]);
        let name = reader.string()?;
        let message = reader.string()?;
        let hardware_id = reader.string()?;
        let count = reader.sequence_length(10)?;
        if count > MAX_VALUES.saturating_sub(total_values) {
            return Err(DecodeError::InvalidLength);
        }
        total_values += count;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(DiagnosticKeyValue {
                key: reader.string()?,
                value: reader.string()?,
            });
        }
        statuses.push(DiagnosticStatus {
            level,
            name,
            message,
            hardware_id,
            values,
        });
    }
    Ok(DiagnosticArray {
        measurement_time,
        frame_id,
        statuses,
    })
}

#[cfg(test)]
mod tests {
    use super::super::common::{push_string, push_u32};
    use super::*;
    fn payload(big_endian: bool) -> Vec<u8> {
        let mut bytes = vec![0, 1, 0, 0];
        push_u32(&mut bytes, 123);
        push_u32(&mut bytes, 456);
        push_string(&mut bytes, "frame").unwrap();
        push_u32(&mut bytes, 5);
        for level in 0..5 {
            bytes.push(level);
            push_string(&mut bytes, "camera").unwrap();
            push_string(&mut bytes, "Running").unwrap();
            push_string(&mut bytes, "hw").unwrap();
            push_u32(&mut bytes, 1);
            push_string(&mut bytes, "key").unwrap();
            push_string(&mut bytes, "value").unwrap();
        }
        if big_endian {
            // Convert aligned u32 fields; byte/string data have no endian conversion.
            let mut reader = Reader::new(&bytes).unwrap();
            let mut positions = vec![4, 8, 12];
            reader.i32().unwrap();
            reader.u32().unwrap();
            reader.string().unwrap();
            reader.align(4).unwrap();
            positions.push(reader.position());
            reader.u32().unwrap();
            for _ in 0..5 {
                reader.take(1).unwrap();
                for _ in 0..3 {
                    reader.align(4).unwrap();
                    positions.push(reader.position());
                    reader.string().unwrap();
                }
                reader.align(4).unwrap();
                positions.push(reader.position());
                reader.u32().unwrap();
                for _ in 0..2 {
                    reader.align(4).unwrap();
                    positions.push(reader.position());
                    reader.string().unwrap();
                }
            }
            bytes[1] = 0;
            for position in positions {
                bytes[position..position + 4].reverse();
            }
        }
        bytes
    }
    #[test]
    fn all_producer_levels_values_and_header_decode_in_both_endiannesses() {
        for big in [false, true] {
            let array = decode_diagnostic_array(&payload(big)).unwrap();
            assert_eq!(array.measurement_time, MeasurementTime(123_000_000_456));
            assert_eq!(array.frame_id, "frame");
            assert_eq!(
                array.statuses.iter().map(|s| s.level).collect::<Vec<_>>(),
                vec![
                    DiagnosticLevel::Ok,
                    DiagnosticLevel::Warn,
                    DiagnosticLevel::Error,
                    DiagnosticLevel::Stale,
                    DiagnosticLevel::Unknown(4)
                ]
            );
            assert_eq!(
                array.statuses[0].values[0],
                DiagnosticKeyValue {
                    key: "key".into(),
                    value: "value".into()
                }
            );
        }
    }
    #[test]
    fn truncated_and_excessive_sequences_are_rejected_without_panicking() {
        let bytes = payload(false);
        for end in 0..bytes.len() {
            assert!(decode_diagnostic_array(&bytes[..end]).is_err());
        }
        let mut bytes = vec![0, 1, 0, 0];
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_string(&mut bytes, "").unwrap();
        push_u32(&mut bytes, u32::MAX);
        assert!(matches!(
            decode_diagnostic_array(&bytes),
            Err(DecodeError::InvalidLength)
        ));
    }
}
