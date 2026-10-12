//! Bounded source-neutral diagnostics aggregation from raw CDR and log times.
use crate::{
    ArrivalTime, DecodeError, DiagnosticKeyValue, DiagnosticLevel, decode_diagnostic_array,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticsReduceError {
    Limit {
        resource: &'static str,
        maximum: usize,
    },
}
impl std::fmt::Display for DiagnosticsReduceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Limit { resource, maximum } => {
                write!(f, "query exceeds {resource} limit ({maximum})")
            }
        }
    }
}
impl std::error::Error for DiagnosticsReduceError {}

#[derive(Debug)]
pub struct DiagnosticEvent {
    pub level: DiagnosticLevel,
    pub name: String,
    pub message: String,
    /// Details from the latest occurrence; equal-time ties favor the last admitted input.
    pub hardware_id: String,
    pub values: Vec<DiagnosticKeyValue>,
    pub count: usize,
    pub first_seen: ArrivalTime,
    pub last_seen: ArrivalTime,
}
#[derive(Debug, Default)]
pub struct DiagnosticsSnapshot {
    pub events: Vec<DiagnosticEvent>,
    pub malformed_messages: usize,
    pub first_decode_error: Option<DecodeError>,
}
const MAX_MESSAGES: usize = 10_000;
const MAX_READ_BYTES: usize = 16 * 1024 * 1024;
const MAX_EVENTS: usize = 4096;
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
/// Reduces at most 10,000 messages / 16 MiB of input into at most 4,096 groups
/// and 4 MiB of retained strings (including keys). No raw payloads are retained.
#[derive(Default)]
pub struct DiagnosticAggregator {
    snapshot: DiagnosticsSnapshot,
    failure: Option<DiagnosticsReduceError>,
    events: BTreeMap<(DiagnosticLevel, String, String), DiagnosticEvent>,
    messages: usize,
    input_bytes: usize,
    retained_bytes: usize,
}
impl DiagnosticAggregator {
    /// Admit raw CDR in deterministic backend order. Equal-time detail ties favor the last input.
    /// A failed admission poisons the reducer: finish never publishes a partial success.
    pub fn admit(&mut self, raw: &crate::RawMessage) -> Result<(), DiagnosticsReduceError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.admit_checked(raw);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }

    fn admit_checked(&mut self, raw: &crate::RawMessage) -> Result<(), DiagnosticsReduceError> {
        if self.messages >= MAX_MESSAGES {
            return Err(DiagnosticsReduceError::Limit {
                resource: "diagnostics input messages",
                maximum: MAX_MESSAGES,
            });
        }
        if raw.payload.len() > MAX_READ_BYTES.saturating_sub(self.input_bytes) {
            return Err(DiagnosticsReduceError::Limit {
                resource: "diagnostics input payload bytes",
                maximum: MAX_READ_BYTES,
            });
        }
        self.messages += 1;
        self.input_bytes += raw.payload.len();
        let array = match decode_diagnostic_array(&raw.payload) {
            Ok(array) => array,
            Err(error) => {
                self.snapshot.malformed_messages += 1;
                if self.snapshot.first_decode_error.is_none() {
                    self.snapshot.first_decode_error = Some(error);
                }
                return Ok(());
            }
        };
        for status in array.statuses {
            self.aggregate(status, raw.arrival_time)?;
        }
        Ok(())
    }
    fn aggregate(
        &mut self,
        status: crate::DiagnosticStatus,
        arrival: ArrivalTime,
    ) -> Result<(), DiagnosticsReduceError> {
        let key = (status.level, status.name.clone(), status.message.clone());
        if let Some(event) = self.events.get_mut(&key) {
            event.count += 1;
            event.first_seen = event.first_seen.min(arrival);
            if arrival >= event.last_seen {
                self.retained_bytes = self
                    .retained_bytes
                    .saturating_sub(detail_bytes(&event.hardware_id, &event.values))
                    .saturating_add(detail_bytes(&status.hardware_id, &status.values));
                event.last_seen = arrival;
                event.hardware_id = status.hardware_id;
                event.values = status.values;
            }
        } else {
            if self.events.len() >= MAX_EVENTS {
                return Err(DiagnosticsReduceError::Limit {
                    resource: "diagnostic events",
                    maximum: MAX_EVENTS,
                });
            }
            // Include strings duplicated in the aggregation key as well as result details.
            self.retained_bytes += 2 * (status.name.len() + status.message.len())
                + detail_bytes(&status.hardware_id, &status.values);
            self.events.insert(
                key,
                DiagnosticEvent {
                    level: status.level,
                    name: status.name,
                    message: status.message,
                    hardware_id: status.hardware_id,
                    values: status.values,
                    count: 1,
                    first_seen: arrival,
                    last_seen: arrival,
                },
            );
        }
        if self.retained_bytes > MAX_EVENT_BYTES {
            return Err(DiagnosticsReduceError::Limit {
                resource: "diagnostic result bytes",
                maximum: MAX_EVENT_BYTES,
            });
        }
        Ok(())
    }
    pub fn finish(mut self) -> Result<DiagnosticsSnapshot, DiagnosticsReduceError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        self.snapshot.events = self.events.into_values().collect();
        self.snapshot.events.sort_by(|a, b| {
            b.last_seen
                .cmp(&a.last_seen)
                .then_with(|| (a.level, &a.name, &a.message).cmp(&(b.level, &b.name, &b.message)))
        });
        Ok(self.snapshot)
    }
}
fn detail_bytes(hardware: &str, values: &[DiagnosticKeyValue]) -> usize {
    hardware.len()
        + values
            .iter()
            .map(|value| value.key.len() + value.value.len())
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RawMessage, StreamId};
    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        while !(bytes.len() - 4).is_multiple_of(4) {
            bytes.push(0);
        }
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn string(bytes: &mut Vec<u8>, value: &str) {
        push_u32(bytes, (value.len() + 1) as u32);
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0);
    }
    fn raw(name: &str, hardware: &str, time: i64) -> RawMessage {
        let mut bytes = vec![0, 1, 0, 0];
        // Measurement time must not influence first/last seen.
        push_u32(&mut bytes, 9999);
        push_u32(&mut bytes, 0);
        string(&mut bytes, "");
        push_u32(&mut bytes, 1);
        bytes.push(2);
        string(&mut bytes, name);
        string(&mut bytes, "timeout");
        string(&mut bytes, hardware);
        push_u32(&mut bytes, 1);
        string(&mut bytes, "source");
        string(&mut bytes, hardware);
        RawMessage {
            stream_id: StreamId(1),
            arrival_time: ArrivalTime(time),
            publish_time_ns: Some(999),
            sequence: Some(0),
            payload: bytes.into(),
        }
    }
    #[test]
    fn log_time_extrema_and_equal_time_details_follow_explicit_input_order() {
        let mut reducer = DiagnosticAggregator::default();
        for input in [
            raw("camera", "first", 200),
            raw("camera", "earlier", 0),
            raw("camera", "tie", 200),
            raw("lidar", "other", 300),
        ] {
            reducer.admit(&input).unwrap();
        }
        let snapshot = reducer.finish().unwrap();
        assert_eq!(snapshot.events[0].name, "lidar");
        let event = &snapshot.events[1];
        assert_eq!(event.level, DiagnosticLevel::Error);
        assert_eq!(event.count, 3);
        assert_eq!(event.first_seen, ArrivalTime(0));
        assert_eq!(event.last_seen, ArrivalTime(200));
        assert_eq!(event.hardware_id, "tie");
        assert_eq!(event.values[0].value, "tie");
    }
    #[test]
    fn malformed_cdr_does_not_stop_valid_inputs_and_first_error_is_preserved() {
        let mut reducer = DiagnosticAggregator::default();
        let mut broken = raw("camera", "ok", 10);
        broken.payload = bytes::Bytes::from_static(&[0]);
        reducer.admit(&broken).unwrap();
        broken.payload = bytes::Bytes::from_static(&[1, 2, 3, 4]);
        reducer.admit(&broken).unwrap();
        reducer.admit(&raw("camera", "ok", 20)).unwrap();
        let snapshot = reducer.finish().unwrap();
        assert_eq!(snapshot.malformed_messages, 2);
        assert!(matches!(
            snapshot.first_decode_error,
            Some(DecodeError::Truncated)
        ));
        assert_eq!(snapshot.events.len(), 1);
        assert_eq!(snapshot.events[0].count, 1);
    }
    #[test]
    fn input_message_and_payload_limits_poison_the_result_instead_of_publishing_partial_data() {
        let mut reducer = DiagnosticAggregator::default();
        let mut broken = raw("camera", "ok", 0);
        broken.payload = bytes::Bytes::new();
        for _ in 0..MAX_MESSAGES {
            reducer.admit(&broken).unwrap();
        }
        let error = reducer.admit(&broken).unwrap_err();
        assert!(matches!(
            error,
            DiagnosticsReduceError::Limit {
                resource: "diagnostics input messages",
                ..
            }
        ));
        assert_eq!(reducer.finish().unwrap_err(), error);
        let mut reducer = DiagnosticAggregator::default();
        broken.payload = vec![0; MAX_READ_BYTES + 1].into();
        let error = reducer.admit(&broken).unwrap_err();
        assert!(matches!(
            error,
            DiagnosticsReduceError::Limit {
                resource: "diagnostics input payload bytes",
                ..
            }
        ));
        assert_eq!(reducer.finish().unwrap_err(), error);
    }
    #[test]
    fn group_limit_stops_new_events_and_cannot_be_ignored() {
        let mut reducer = DiagnosticAggregator::default();
        for index in 0..MAX_EVENTS {
            reducer.admit(&raw(&index.to_string(), "hw", 0)).unwrap();
        }
        let error = reducer.admit(&raw("overflow", "hw", 0)).unwrap_err();
        assert!(matches!(
            error,
            DiagnosticsReduceError::Limit {
                resource: "diagnostic events",
                ..
            }
        ));
        assert_eq!(
            reducer.admit(&raw("0", "still failed", 1)).unwrap_err(),
            error
        );
        assert_eq!(reducer.finish().unwrap_err(), error);
    }
    #[test]
    fn retained_strings_limit_counts_keys_and_replaced_details() {
        let mut reducer = DiagnosticAggregator::default();
        let hardware = "x".repeat(60_000);
        // Each event retains hardware and a diagnostic value; input stays below 16 MiB.
        for index in 0..100 {
            match reducer.admit(&raw(&index.to_string(), &hardware, 0)) {
                Ok(()) => (),
                Err(error) => {
                    assert!(matches!(
                        error,
                        DiagnosticsReduceError::Limit {
                            resource: "diagnostic result bytes",
                            ..
                        }
                    ));
                    assert_eq!(reducer.finish().unwrap_err(), error);
                    return;
                }
            }
        }
        panic!("retained string budget must be enforced");
    }
}
