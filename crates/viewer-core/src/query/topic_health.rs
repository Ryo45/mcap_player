use super::{DEFAULT_QUERY_WINDOW_SECONDS, WindowQueryError, cursor_window, elapsed};
use crate::{ArrivalTime, McapSource, MessageTimestampReader, StreamId, TimestampQueryLimits};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TopicHealthRequirement {
    pub window_seconds: u32,
}
impl Default for TopicHealthRequirement {
    fn default() -> Self {
        Self {
            window_seconds: DEFAULT_QUERY_WINDOW_SECONDS,
        }
    }
}

#[derive(Debug)]
pub struct TopicHealthSnapshot {
    pub stream_ids: Vec<StreamId>,
    pub topic: String,
    pub schema: String,
    pub recording_message_count: Option<u64>,
    pub sample_count: u64,
    pub observed_rate_hz: Option<f64>,
    pub latest_arrival: Option<ArrivalTime>,
    pub latest_age: Option<Duration>,
    pub recent_gap: Option<Duration>,
    pub max_gap: Option<Duration>,
    pub error: Option<Arc<WindowQueryError>>,
}

const MAX_TOPICS: usize = 2048;
const MAX_STREAMS: usize = 4096;

pub fn query_topic_health(
    source: &McapSource,
    requirement: TopicHealthRequirement,
    cursor: ArrivalTime,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Vec<TopicHealthSnapshot>, WindowQueryError> {
    let range = cursor_window(cursor, requirement.window_seconds)?;
    if source.catalog().streams.len() > MAX_STREAMS {
        return Err(WindowQueryError::Limit {
            resource: "streams",
            maximum: MAX_STREAMS,
        });
    }
    let mut topics = BTreeMap::<&str, Vec<_>>::new();
    for stream in &source.catalog().streams {
        topics.entry(&stream.topic).or_default().push(stream);
    }
    if topics.len() > MAX_TOPICS {
        return Err(WindowQueryError::Limit {
            resource: "topics",
            maximum: MAX_TOPICS,
        });
    }
    let mut reader = MessageTimestampReader::new(
        source.fork_reader(),
        TimestampQueryLimits {
            max_messages: 200_000,
            max_index_bytes: 32 * 1024 * 1024,
            max_index_reads: 4096,
        },
    );
    let mut snapshots = Vec::with_capacity(topics.len());
    for (topic, streams) in topics {
        if cancelled() {
            return Err(crate::McapOpenError::QueryCancelled.into());
        }
        let mut snapshot = TopicHealthSnapshot {
            stream_ids: streams.iter().map(|s| s.id).collect(),
            topic: topic.to_owned(),
            schema: streams
                .iter()
                .map(|s| s.schema.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", "),
            recording_message_count: streams.iter().try_fold(0_u64, |count, stream| {
                count.checked_add(stream.timing.message_count?)
            }),
            sample_count: 0,
            observed_rate_hz: None,
            latest_arrival: None,
            latest_age: None,
            recent_gap: None,
            max_gap: None,
            error: None,
        };
        let mut arrivals = Vec::new();
        for stream in streams {
            match reader.read_range(stream.id, range, &mut cancelled) {
                Ok(result) => {
                    arrivals.extend(result.arrivals);
                    snapshot.latest_arrival = snapshot.latest_arrival.max(result.latest_arrival);
                }
                Err(crate::McapOpenError::QueryCancelled) => {
                    return Err(crate::McapOpenError::QueryCancelled.into());
                }
                Err(error) => {
                    snapshot.error = Some(Arc::new(error.into()));
                    break;
                }
            }
        }
        if snapshot.error.is_none() {
            arrivals.sort_unstable();
            summarize(&mut snapshot, &arrivals, cursor);
        } else {
            // Never present partial timing facts as a complete topic measurement.
            snapshot.latest_arrival = None;
        }
        snapshots.push(snapshot);
    }
    Ok(snapshots)
}

fn summarize(snapshot: &mut TopicHealthSnapshot, arrivals: &[ArrivalTime], cursor: ArrivalTime) {
    snapshot.sample_count = arrivals.len() as u64;
    snapshot.latest_age = snapshot.latest_arrival.map(|last| elapsed(cursor, last));
    if arrivals.len() < 2 {
        return;
    }
    let span = elapsed(*arrivals.last().unwrap(), arrivals[0]);
    if !span.is_zero() {
        snapshot.observed_rate_hz = Some((arrivals.len() - 1) as f64 / span.as_secs_f64());
    }
    snapshot.recent_gap = Some(elapsed(
        arrivals[arrivals.len() - 1],
        arrivals[arrivals.len() - 2],
    ));
    snapshot.max_gap = arrivals
        .windows(2)
        .map(|pair| elapsed(pair[1], pair[0]))
        .max();
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcap::{WriteOptions, Writer, records::MessageHeader};
    use std::{collections::BTreeMap, io::Cursor};

    pub(crate) fn source(times: &[u64]) -> McapSource {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer =
                Writer::with_options(&mut output, WriteOptions::new().compression(None)).unwrap();
            let schema = writer
                .add_schema("demo/msg/Undecodable", "ros2msg", b"invalid schema")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/a", "cdr", &BTreeMap::new())
                .unwrap();
            for (index, time) in times.iter().enumerate() {
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence: index as u32,
                            log_time: *time,
                            publish_time: 123,
                        },
                        b"not CDR",
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        McapSource::from_bytes(bytes::Bytes::from(output.into_inner())).unwrap()
    }
    #[test]
    fn timing_facts_use_only_log_times_through_cursor_without_decoding() {
        let source = source(&[0, 100_000_000, 300_000_000, 500_000_000]);
        let snapshots = query_topic_health(
            &source,
            TopicHealthRequirement::default(),
            ArrivalTime(450_000_000),
            || false,
        )
        .unwrap();
        let row = &snapshots[0];
        assert!(row.error.is_none());
        assert_eq!(row.recording_message_count, Some(4));
        assert_eq!(row.sample_count, 3);
        assert!((row.observed_rate_hz.unwrap() - 2.0 / 0.3).abs() < 1e-10);
        assert_eq!(row.latest_age, Some(Duration::from_millis(150)));
        assert_eq!(row.max_gap, Some(Duration::from_millis(200)));
        assert_eq!(row.recent_gap, row.max_gap);
        let earlier = query_topic_health(
            &source,
            TopicHealthRequirement::default(),
            ArrivalTime(100_000_000),
            || false,
        )
        .unwrap();
        assert_eq!(earlier[0].sample_count, 2);
        assert_eq!(earlier[0].latest_arrival, Some(ArrivalTime(100_000_000)));
    }
    #[test]
    fn single_duplicate_and_out_of_window_arrivals_have_safe_statistics() {
        let single = source(&[100]);
        let rows = query_topic_health(
            &single,
            TopicHealthRequirement::default(),
            ArrivalTime(100),
            || false,
        )
        .unwrap();
        assert_eq!(rows[0].sample_count, 1);
        assert_eq!(rows[0].observed_rate_hz, None);
        assert_eq!(rows[0].max_gap, None);
        let rows = query_topic_health(
            &single,
            TopicHealthRequirement { window_seconds: 1 },
            ArrivalTime(2_000_000_000),
            || false,
        )
        .unwrap();
        assert_eq!(rows[0].sample_count, 0);
        assert_eq!(rows[0].latest_arrival, Some(ArrivalTime(100)));
        assert_eq!(
            rows[0].latest_age,
            Some(Duration::from_nanos(1_999_999_900))
        );
        let duplicate = source(&[100, 100]);
        let rows = query_topic_health(
            &duplicate,
            TopicHealthRequirement::default(),
            ArrivalTime(100),
            || false,
        )
        .unwrap();
        assert_eq!(rows[0].observed_rate_hz, None);
        assert_eq!(rows[0].max_gap, Some(Duration::ZERO));
    }
    #[test]
    fn channels_sharing_a_topic_contribute_one_combined_timing_series() {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer =
                Writer::with_options(&mut output, WriteOptions::new().compression(None)).unwrap();
            let one = writer
                .add_schema("demo/msg/One", "ros2msg", b"uint8 value")
                .unwrap();
            let two = writer
                .add_schema("demo/msg/Two", "ros2msg", b"uint8 value")
                .unwrap();
            let a = writer
                .add_channel(one, "/combined", "cdr", &BTreeMap::new())
                .unwrap();
            let b = writer
                .add_channel(two, "/combined", "cdr", &BTreeMap::new())
                .unwrap();
            for (channel, time) in [(a, 0), (b, 100_000_000), (a, 200_000_000)] {
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence: 0,
                            log_time: time,
                            publish_time: 0,
                        },
                        b"invalid",
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        let source = McapSource::from_bytes(bytes::Bytes::from(output.into_inner())).unwrap();
        let result = query_topic_health(
            &source,
            TopicHealthRequirement::default(),
            ArrivalTime(200_000_000),
            || false,
        )
        .unwrap();
        assert_eq!(result.len(), 1);
        let row = &result[0];
        assert_eq!(row.stream_ids.len(), 2);
        assert_eq!(row.recording_message_count, Some(3));
        assert_eq!(row.sample_count, 3);
        assert_eq!(row.observed_rate_hz, Some(10.0));
        assert_eq!(row.max_gap, Some(Duration::from_millis(100)));
        assert_eq!(row.schema, "demo/msg/One, demo/msg/Two");
    }

    #[test]
    fn timestamp_reads_obey_budget_and_cancellation_before_io() {
        let source = source(&[100, 200]);
        let stream = source.catalog().streams[0].id;
        let range = crate::DataWindowTimeRange::new(ArrivalTime(0), ArrivalTime(201)).unwrap();
        let mut reader = MessageTimestampReader::new(
            source,
            TimestampQueryLimits {
                max_messages: 1,
                max_index_bytes: 1024,
                max_index_reads: 4,
            },
        );
        assert!(matches!(
            reader.read_range(stream, range, || true),
            Err(crate::McapOpenError::QueryCancelled)
        ));
        assert!(matches!(
            reader.read_range(stream, range, || false),
            Err(crate::McapOpenError::QueryLimit {
                resource: "timestamp samples",
                ..
            })
        ));
    }
}
