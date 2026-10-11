use super::{DEFAULT_QUERY_WINDOW_SECONDS, WindowQueryError, cursor_window};
use crate::{
    ArrivalTime, DecodeError, DiagnosticKeyValue, DiagnosticLevel, McapSource,
    decode_diagnostic_array,
};
use std::{cell::Cell, collections::BTreeMap};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DiagnosticsRequirement {
    pub topic: String,
    pub window_seconds: u32,
}
impl Default for DiagnosticsRequirement {
    fn default() -> Self {
        Self {
            topic: "/diagnostics".into(),
            window_seconds: DEFAULT_QUERY_WINDOW_SECONDS,
        }
    }
}
#[derive(Debug)]
pub struct DiagnosticEvent {
    pub level: DiagnosticLevel,
    pub name: String,
    pub message: String,
    /// Details from the latest occurrence, deterministic by stream then source order on ties.
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
const MAX_CHUNKS: usize = 4096;
const MAX_CHUNK_BYTES: u64 = 128 * 1024 * 1024;

pub fn query_diagnostics(
    source: &McapSource,
    requirement: &DiagnosticsRequirement,
    cursor: ArrivalTime,
    mut cancelled: impl FnMut() -> bool,
) -> Result<DiagnosticsSnapshot, WindowQueryError> {
    let range = cursor_window(cursor, requirement.window_seconds)?;
    let mut streams = source
        .catalog()
        .streams
        .iter()
        .filter(|s| s.topic == requirement.topic)
        .collect::<Vec<_>>();
    if streams.is_empty() {
        return Err(WindowQueryError::MissingTopic(requirement.topic.clone()));
    }
    streams.sort_by_key(|s| s.id);
    for stream in &streams {
        if stream.schema != "diagnostic_msgs/msg/DiagnosticArray"
            || stream.message_encoding != "cdr"
        {
            return Err(WindowQueryError::UnsupportedDiagnosticStream(stream.id));
        }
    }
    let mut aggregator = DiagnosticAggregator::default();
    let limit_reached = Cell::new(false);
    let mut chunks = 0usize;
    let mut chunk_bytes = 0_u64;
    for stream in streams {
        for chunk in source.summary().chunk_indexes.iter().filter(|chunk| {
            chunk
                .message_index_offsets
                .contains_key(&(stream.id.0 as u16))
                && chunk.message_end_time >= range.start.0.max(0) as u64
                && chunk.message_start_time < range.end_exclusive.0.max(0) as u64
        }) {
            chunks += 1;
            chunk_bytes = chunk_bytes
                .saturating_add(chunk.compressed_size)
                .saturating_add(chunk.uncompressed_size);
            if chunk_bytes > MAX_CHUNK_BYTES {
                return Err(WindowQueryError::Limit {
                    resource: "diagnostics compressed/decompressed chunk bytes",
                    maximum: MAX_CHUNK_BYTES as usize,
                });
            }
        }
        if chunks > MAX_CHUNKS {
            return Err(WindowQueryError::Limit {
                resource: "diagnostics chunks",
                maximum: MAX_CHUNKS,
            });
        }
        let mut aggregation_error = None;
        let read = source.visit_stream_range(
            stream.id,
            range,
            || limit_reached.get() || cancelled(),
            |raw| {
                if !limit_reached.get()
                    && let Err(error) = aggregator.admit(raw)
                {
                    aggregation_error = Some(error);
                    limit_reached.set(true);
                }
            },
        );
        if let Some(error) = aggregation_error {
            return Err(error);
        }
        read?;
    }
    Ok(aggregator.finish())
}

#[derive(Default)]
struct DiagnosticAggregator {
    snapshot: DiagnosticsSnapshot,
    events: BTreeMap<(DiagnosticLevel, String, String), DiagnosticEvent>,
    messages: usize,
    input_bytes: usize,
    retained_bytes: usize,
}
impl DiagnosticAggregator {
    fn admit(&mut self, raw: &crate::RawMessage) -> Result<(), WindowQueryError> {
        if self.messages >= MAX_MESSAGES {
            return Err(WindowQueryError::Limit {
                resource: "diagnostics input messages",
                maximum: MAX_MESSAGES,
            });
        }
        if raw.payload.len() > MAX_READ_BYTES.saturating_sub(self.input_bytes) {
            return Err(WindowQueryError::Limit {
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
    ) -> Result<(), WindowQueryError> {
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
                return Err(WindowQueryError::Limit {
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
            return Err(WindowQueryError::Limit {
                resource: "diagnostic result bytes",
                maximum: MAX_EVENT_BYTES,
            });
        }
        Ok(())
    }
    fn finish(mut self) -> DiagnosticsSnapshot {
        self.snapshot.events = self.events.into_values().collect();
        self.snapshot.events.sort_by(|a, b| {
            b.last_seen
                .cmp(&a.last_seen)
                .then_with(|| (a.level, &a.name, &a.message).cmp(&(b.level, &b.name, &b.message)))
        });
        self.snapshot
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
    use crate::message::cdr::common::{push_string, push_u32};
    use mcap::{WriteOptions, Writer, records::MessageHeader};
    use std::io::Cursor;

    fn diagnostic(hardware: &str) -> Vec<u8> {
        let mut bytes = vec![0, 1, 0, 0];
        // Header measurement time is intentionally unrelated to the query log-time window.
        push_u32(&mut bytes, 9999);
        push_u32(&mut bytes, 0);
        push_string(&mut bytes, "").unwrap();
        push_u32(&mut bytes, 2);
        for (level, name, message) in [(2, "camera", "timeout"), (0, "lidar", "running")] {
            bytes.push(level);
            push_string(&mut bytes, name).unwrap();
            push_string(&mut bytes, message).unwrap();
            push_string(&mut bytes, hardware).unwrap();
            push_u32(&mut bytes, 1);
            push_string(&mut bytes, "source").unwrap();
            push_string(&mut bytes, hardware).unwrap();
        }
        bytes
    }
    fn source() -> McapSource {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer =
                Writer::with_options(&mut output, WriteOptions::new().compression(None)).unwrap();
            let schema = writer
                .add_schema(
                    "diagnostic_msgs/msg/DiagnosticArray",
                    "ros2msg",
                    b"std_msgs/Header header\ndiagnostic_msgs/DiagnosticStatus[] status\n",
                )
                .unwrap();
            let channel = writer
                .add_channel(schema, "/diagnostics", "cdr", &BTreeMap::new())
                .unwrap();
            for (sequence, (time, bytes)) in [
                (0, diagnostic("old")),
                (150, vec![0]),
                (200, diagnostic("latest")),
                (10_000_000_001, diagnostic("future")),
            ]
            .into_iter()
            .enumerate()
            {
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence: sequence as u32,
                            log_time: time,
                            publish_time: 999,
                        },
                        &bytes,
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        McapSource::from_bytes(bytes::Bytes::from(output.into_inner())).unwrap()
    }
    #[test]
    fn aggregates_producer_events_and_skips_malformed_messages_without_stopping() {
        let source = source();
        let result = query_diagnostics(
            &source,
            &DiagnosticsRequirement::default(),
            ArrivalTime(200),
            || false,
        )
        .unwrap();
        assert_eq!(result.events.len(), 2);
        assert_eq!(result.malformed_messages, 1);
        assert!(result.first_decode_error.is_some());
        let camera = result.events.iter().find(|e| e.name == "camera").unwrap();
        assert_eq!(camera.level, DiagnosticLevel::Error);
        assert_eq!(camera.count, 2);
        assert_eq!(camera.first_seen, ArrivalTime(0));
        assert_eq!(camera.last_seen, ArrivalTime(200));
        assert_eq!(camera.hardware_id, "latest");
        assert_eq!(camera.values[0].value, "latest");
    }
    #[test]
    fn inclusive_cursor_backward_seek_and_window_expiration_use_log_time() {
        let source = source();
        let late = query_diagnostics(
            &source,
            &DiagnosticsRequirement::default(),
            ArrivalTime(10_000_000_001),
            || false,
        )
        .unwrap();
        assert!(
            late.events
                .iter()
                .all(|e| e.count == 2 && e.first_seen == ArrivalTime(200))
        );
        let earlier = query_diagnostics(
            &source,
            &DiagnosticsRequirement::default(),
            ArrivalTime(0),
            || false,
        )
        .unwrap();
        assert!(
            earlier
                .events
                .iter()
                .all(|e| e.count == 1 && e.last_seen == ArrivalTime(0) && e.hardware_id == "old")
        );
        assert_eq!(earlier.malformed_messages, 0);
        let empty = query_diagnostics(
            &source,
            &DiagnosticsRequirement::default(),
            ArrivalTime(30_000_000_000),
            || false,
        )
        .unwrap();
        assert!(empty.events.is_empty());
    }
    #[test]
    fn missing_topics_cancellation_and_time_overflow_are_explicit_errors() {
        let source = source();
        assert!(matches!(
            query_diagnostics(
                &source,
                &DiagnosticsRequirement {
                    topic: "/absent".into(),
                    ..Default::default()
                },
                ArrivalTime(0),
                || false
            ),
            Err(WindowQueryError::MissingTopic(_))
        ));
        assert!(matches!(
            query_diagnostics(
                &source,
                &DiagnosticsRequirement::default(),
                ArrivalTime(200),
                || true
            ),
            Err(WindowQueryError::Source(
                crate::McapOpenError::QueryCancelled
            ))
        ));
        assert!(matches!(
            query_diagnostics(
                &source,
                &DiagnosticsRequirement::default(),
                ArrivalTime(i64::MAX),
                || false
            ),
            Err(WindowQueryError::InvalidWindow)
        ));
    }
}
