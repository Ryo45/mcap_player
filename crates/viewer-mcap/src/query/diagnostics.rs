use super::{DEFAULT_QUERY_WINDOW_SECONDS, WindowQueryError, cursor_window};
use crate::{ArrivalTime, McapSource};
use std::cell::Cell;
use viewer_core::{DiagnosticAggregator, DiagnosticsSnapshot};

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
            return Err(error.into());
        }
        read?;
    }
    aggregator.finish().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    fn push_u32(output: &mut Vec<u8>, value: u32) {
        while !(output.len() - 4).is_multiple_of(4) {
            output.push(0);
        }
        output.extend_from_slice(&value.to_le_bytes());
    }
    fn push_string(output: &mut Vec<u8>, value: &str) -> Result<(), viewer_core::DecodeError> {
        let length = value
            .len()
            .checked_add(1)
            .ok_or(viewer_core::DecodeError::InvalidLength)?;
        push_u32(
            output,
            u32::try_from(length).map_err(|_| viewer_core::DecodeError::InvalidLength)?,
        );
        output.extend_from_slice(value.as_bytes());
        output.push(0);
        Ok(())
    }
    use super::*;
    use mcap::{WriteOptions, Writer, records::MessageHeader};
    use std::collections::BTreeMap;
    use std::io::Cursor;
    use viewer_core::DiagnosticLevel;

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
                crate::McapSourceError::QueryCancelled
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
