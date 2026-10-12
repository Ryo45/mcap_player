//! Query-owned multi-stream traversal over physical chunk Message Index spans.
use super::WindowQueryError;
use crate::{McapSource, McapSourceError};
use std::collections::{BTreeMap, BinaryHeap, HashSet};
use viewer_core::{ArrivalTime, DataWindowTimeRange, StreamId};

#[derive(Clone, Copy)]
pub(super) struct TimestampQueryLimits {
    pub max_samples: usize,
    pub max_index_bytes: usize,
    pub max_index_reads: usize,
}
impl Default for TimestampQueryLimits {
    fn default() -> Self {
        Self {
            max_samples: 200_000,
            max_index_bytes: 32 * 1024 * 1024,
            max_index_reads: 4096,
        }
    }
}

#[derive(Default)]
pub(super) struct StreamTimestampRange {
    pub arrivals: Vec<ArrivalTime>,
    pub latest_arrival: Option<ArrivalTime>,
}

struct TimestampQueryBudget {
    limits: TimestampQueryLimits,
    samples: usize,
    index_bytes: usize,
    index_reads: usize,
}
impl TimestampQueryBudget {
    fn new(limits: TimestampQueryLimits) -> Self {
        Self {
            limits,
            samples: 0,
            index_bytes: 0,
            index_reads: 0,
        }
    }
    fn reserve_index(&mut self, length: usize) -> Result<(), WindowQueryError> {
        if length > self.limits.max_index_bytes.saturating_sub(self.index_bytes) {
            return Err(WindowQueryError::Limit {
                resource: "Message Index bytes",
                maximum: self.limits.max_index_bytes,
            });
        }
        if self.index_reads >= self.limits.max_index_reads {
            return Err(WindowQueryError::Limit {
                resource: "Message Index reads",
                maximum: self.limits.max_index_reads,
            });
        }
        // Attempts consume budget even when the physical read or parse fails.
        self.index_bytes += length;
        self.index_reads += 1;
        Ok(())
    }
    fn admit_sample(&mut self) -> Result<(), WindowQueryError> {
        if self.samples >= self.limits.max_samples {
            return Err(WindowQueryError::Limit {
                resource: "timestamp samples",
                maximum: self.limits.max_samples,
            });
        }
        self.samples += 1;
        Ok(())
    }
}

// One frontier item per selected stream; chunk start and ID put shared chunks together.
type Frontier = BinaryHeap<(u64, usize, StreamId, usize)>;

pub(super) fn collect_timestamp_window(
    source: &McapSource,
    streams: &[StreamId],
    range: DataWindowTimeRange,
    limits: TimestampQueryLimits,
    mut cancelled: impl FnMut() -> bool,
) -> Result<BTreeMap<StreamId, StreamTimestampRange>, WindowQueryError> {
    if range.start >= range.end_exclusive {
        return Err(WindowQueryError::InvalidWindow);
    }
    if cancelled() {
        return Err(McapSourceError::QueryCancelled.into());
    }
    source.validate_unchanged()?;
    let mut results = BTreeMap::<StreamId, StreamTimestampRange>::new();
    let mut frontier = Frontier::new();
    for stream in streams.iter().copied() {
        if results.contains_key(&stream) {
            continue;
        }
        results.insert(stream, StreamTimestampRange::default());
        if let Some(timeline) = source.stream_chunk_timeline(stream) {
            let end = timeline.chunks.partition_point(|chunk| {
                source.summary().chunk_indexes[*chunk].message_start_time
                    < range.end_exclusive.0.max(0) as u64
            });
            if let Some(position) = end.checked_sub(1) {
                enqueue(source, &mut frontier, stream, position);
            }
        } else {
            let descriptor = source
                .catalog()
                .by_id(stream)
                .ok_or(mcap::McapError::BadIndex)
                .map_err(McapSourceError::from)?;
            if descriptor.timing.message_count != Some(0) {
                return Err(
                    McapSourceError::MessageIndexRequired(crate::IndexedPlanError { stream })
                        .into(),
                );
            }
        }
    }
    let mut visited = HashSet::new();
    let mut budget = TimestampQueryBudget::new(limits);
    while let Some((_, chunk_index, stream, position)) = frontier.pop() {
        if cancelled() {
            return Err(McapSourceError::QueryCancelled.into());
        }
        let timeline = source
            .stream_chunk_timeline(stream)
            .expect("frontier stream has indexed chunks");
        let latest = results[&stream].latest_arrival;
        // A predecessor in a later-starting chunk is not necessarily the latest.
        // Only the maximum end of ALL remaining chunks proves we can stop this stream.
        if let Some(latest) = latest
            && timeline.prefix_max_end[position] < range.start
            && timeline.prefix_max_end[position] <= latest
        {
            continue;
        }
        let chunk = &source.summary().chunk_indexes[chunk_index];
        let window_candidate = chunk.message_end_time >= range.start.0.max(0) as u64;
        let predecessor_candidate =
            latest.is_none_or(|last| chunk.message_end_time > last.0 as u64);
        if !visited.contains(&chunk_index) && (window_candidate || predecessor_candidate) {
            read_and_reduce(
                source,
                chunk_index,
                range,
                &mut results,
                &mut budget,
                &mut cancelled,
            )?;
            visited.insert(chunk_index);
        }
        if let Some(previous) = position.checked_sub(1) {
            enqueue(source, &mut frontier, stream, previous);
        }
    }
    if cancelled() {
        return Err(McapSourceError::QueryCancelled.into());
    }
    source.validate_unchanged()?;
    Ok(results)
}

fn enqueue(source: &McapSource, frontier: &mut Frontier, stream: StreamId, position: usize) {
    let chunk = source
        .stream_chunk_timeline(stream)
        .expect("indexed stream")
        .chunks[position];
    frontier.push((
        source.summary().chunk_indexes[chunk].message_start_time,
        chunk,
        stream,
        position,
    ));
}

fn read_and_reduce(
    source: &McapSource,
    chunk_index: usize,
    range: DataWindowTimeRange,
    results: &mut BTreeMap<StreamId, StreamTimestampRange>,
    budget: &mut TimestampQueryBudget,
    mut cancelled: impl FnMut() -> bool,
) -> Result<(), WindowQueryError> {
    if cancelled() {
        return Err(McapSourceError::QueryCancelled.into());
    }
    let length = usize::try_from(source.summary().chunk_indexes[chunk_index].message_index_length)
        .map_err(|_| McapSourceError::ReadOverflow)?;
    budget.reserve_index(length)?;
    let indexes = source.read_chunk_message_indexes(chunk_index, &mut cancelled)?;
    for (channel, entries) in &indexes.channels {
        if cancelled() {
            return Err(McapSourceError::QueryCancelled.into());
        }
        let Some(result) = results.get_mut(&StreamId(u32::from(*channel))) else {
            continue;
        };
        let end =
            entries.partition_point(|entry| entry.log_time < range.end_exclusive.0.max(0) as u64);
        if let Some(last) = end.checked_sub(1) {
            let arrival = ArrivalTime(
                i64::try_from(entries[last].log_time)
                    .map_err(|_| McapSourceError::TimestampOverflow)?,
            );
            result.latest_arrival = result.latest_arrival.max(Some(arrival));
        }
        let start =
            entries[..end].partition_point(|entry| entry.log_time < range.start.0.max(0) as u64);
        for entry in &entries[start..end] {
            if cancelled() {
                return Err(McapSourceError::QueryCancelled.into());
            }
            budget.admit_sample()?;
            result.arrivals.push(ArrivalTime(
                i64::try_from(entry.log_time).map_err(|_| McapSourceError::TimestampOverflow)?,
            ));
        }
    }
    // Parsed entries and their backing allocations are released before another chunk is read.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcap::{Summary, WriteOptions, Writer, records::MessageHeader};
    use std::{collections::BTreeMap, io::Cursor, sync::atomic::Ordering::Relaxed};

    fn fixture(stream_count: usize, chunks: &[Vec<(usize, u64)>]) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = Writer::with_options(
                &mut output,
                WriteOptions::new().compression(None).chunk_size(None),
            )
            .unwrap();
            let schema = writer
                .add_schema("demo/msg/Sample", "ros2msg", b"uint8 value")
                .unwrap();
            let channels = (0..stream_count)
                .map(|stream| {
                    writer
                        .add_channel(
                            schema,
                            &format!("/topic/{stream:03}"),
                            "cdr",
                            &BTreeMap::new(),
                        )
                        .unwrap()
                })
                .collect::<Vec<_>>();
            for chunk in chunks {
                for (sequence, (stream, time)) in chunk.iter().enumerate() {
                    writer
                        .write_to_known_channel(
                            &MessageHeader {
                                channel_id: channels[*stream],
                                sequence: sequence as u32,
                                log_time: *time,
                                publish_time: 999,
                            },
                            &[1],
                        )
                        .unwrap();
                }
                writer.flush().unwrap();
            }
            writer.finish().unwrap();
        }
        output.into_inner()
    }
    fn source(bytes: Vec<u8>) -> McapSource {
        McapSource::from_bytes(bytes::Bytes::from(bytes)).unwrap()
    }
    fn streams(source: &McapSource) -> Vec<StreamId> {
        source
            .catalog()
            .streams
            .iter()
            .map(|stream| stream.id)
            .collect()
    }
    fn range(start: i64, end: i64) -> DataWindowTimeRange {
        DataWindowTimeRange::new(ArrivalTime(start), ArrivalTime(end)).unwrap()
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn three_streams_in_three_chunks_use_three_physical_index_reads() {
        let chunks = (0..3)
            .map(|chunk| {
                (0..3)
                    .map(|stream| (stream, chunk * 100 + stream as u64))
                    .collect()
            })
            .collect::<Vec<_>>();
        let path = std::env::temp_dir().join(format!(
            "mcap-timestamp-three-by-three-{}.mcap",
            std::process::id()
        ));
        std::fs::write(&path, fixture(3, &chunks)).unwrap();
        let source = McapSource::open_file(&path).unwrap();
        assert_eq!(source.summary().chunk_indexes.len(), 3);
        let result = collect_timestamp_window(
            &source,
            &streams(&source),
            range(0, 203),
            TimestampQueryLimits::default(),
            || false,
        )
        .unwrap();
        for samples in result.values() {
            assert_eq!(samples.arrivals.len(), 3);
        }
        let metrics = source.index_read_metrics();
        assert_eq!(
            metrics.reads.load(Relaxed),
            3,
            "one physical span read per chunk, not per stream"
        );
        assert_eq!(metrics.peak_resident_spans.load(Relaxed), 1);
        assert_eq!(metrics.resident_spans.load(Relaxed), 0);
        drop(source);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn overlapping_chunk_ranges_do_not_stop_at_the_first_predecessor() {
        const S: u64 = 1_000_000_000;
        let chunks = vec![
            vec![(0, S), (0, 90 * S), (1, 100 * S), (2, S)],
            vec![(0, 55 * S), (1, 50 * S), (2, 50 * S)],
            vec![(0, 65 * S), (1, 60 * S), (2, 60 * S)],
        ];
        let source = source(fixture(3, &chunks));
        let result = collect_timestamp_window(
            &source,
            &streams(&source),
            range(110 * S as i64, 120 * S as i64 + 1),
            TimestampQueryLimits::default(),
            || false,
        )
        .unwrap();
        let stream = source.catalog().by_topic("/topic/000").unwrap().id;
        assert_eq!(
            result[&stream].latest_arrival,
            Some(ArrivalTime(90 * S as i64))
        );
        assert!(result.values().all(|samples| samples.arrivals.is_empty()));
        assert_eq!(
            source.index_read_metrics().reads.load(Relaxed),
            2,
            "middle chunk cannot improve the predecessor"
        );
        let result = collect_timestamp_window(
            &source,
            &streams(&source),
            range(90 * S as i64, 95 * S as i64 + 1),
            TimestampQueryLimits::default(),
            || false,
        )
        .unwrap();
        assert_eq!(result[&stream].arrivals, vec![ArrivalTime(90 * S as i64)]);
        assert_eq!(
            result[&stream].latest_arrival,
            Some(ArrivalTime(90 * S as i64))
        );
    }
    #[test]
    fn many_topics_do_not_retain_more_index_cache_as_chunk_count_grows() {
        const STREAMS: usize = 64;
        let mut peak_bytes = Vec::new();
        for chunk_count in [3, 128] {
            let chunks = (0..chunk_count)
                .map(|chunk| {
                    (0..STREAMS)
                        .map(|stream| (stream, chunk as u64 * 1_000_000 + stream as u64))
                        .collect()
                })
                .collect::<Vec<_>>();
            let source = source(fixture(STREAMS, &chunks));
            let cursor = source.time_range().end_inclusive();
            let rows = crate::query_topic_health(
                &source,
                crate::TopicHealthRequirement::default(),
                cursor,
                || false,
            )
            .unwrap();
            assert_eq!(rows.len(), STREAMS);
            assert!(
                rows.iter()
                    .all(|row| row.error.is_none() && row.sample_count == chunk_count as u64)
            );
            let metrics = source.index_read_metrics();
            assert_eq!(metrics.reads.load(Relaxed), chunk_count);
            assert_eq!(metrics.peak_resident_spans.load(Relaxed), 1);
            assert_eq!(metrics.resident_spans.load(Relaxed), 0);
            assert_eq!(metrics.resident_entry_bytes.load(Relaxed), 0);
            peak_bytes.push(metrics.peak_resident_entry_bytes.load(Relaxed));
        }
        assert!(peak_bytes[0] > 0);
        assert_eq!(
            peak_bytes[0], peak_bytes[1],
            "allocated index Vec backing remains one chunk, not 128 chunks"
        );
    }
    #[test]
    fn failed_index_parse_consumes_budget_and_is_not_retried_across_topics() {
        let mut bytes = fixture(
            8,
            &[vec![
                (0, 100),
                (1, 101),
                (2, 102),
                (3, 103),
                (4, 104),
                (5, 105),
                (6, 106),
                (7, 107),
            ]],
        );
        let summary = Summary::read(&bytes).unwrap().unwrap();
        let offset = *summary.chunk_indexes[0]
            .message_index_offsets
            .values()
            .min()
            .unwrap() as usize;
        bytes[offset] = 0;
        let source = source(bytes);
        let mut budget = TimestampQueryBudget::new(TimestampQueryLimits {
            max_index_reads: 1,
            ..Default::default()
        });
        let mut results = streams(&source)
            .into_iter()
            .map(|stream| (stream, StreamTimestampRange::default()))
            .collect();
        assert!(matches!(
            read_and_reduce(&source, 0, range(0, 108), &mut results, &mut budget, || {
                false
            }),
            Err(WindowQueryError::Source(_))
        ));
        assert_eq!(budget.index_reads, 1);
        assert_eq!(
            budget.index_bytes,
            source.summary().chunk_indexes[0].message_index_length as usize
        );
        assert!(matches!(
            read_and_reduce(&source, 0, range(0, 108), &mut results, &mut budget, || {
                false
            }),
            Err(WindowQueryError::Limit {
                resource: "Message Index reads",
                ..
            })
        ));
        assert_eq!(source.index_read_metrics().reads.load(Relaxed), 1);
        assert!(
            crate::query_topic_health(
                &source,
                crate::TopicHealthRequirement::default(),
                ArrivalTime(107),
                || false
            )
            .is_err()
        );
        assert_eq!(
            source.index_read_metrics().reads.load(Relaxed),
            2,
            "the entire query stops after one malformed shared span"
        );
        assert_eq!(source.index_read_metrics().resident_spans.load(Relaxed), 0);
    }
    #[test]
    fn sample_and_index_budgets_are_enforced_and_cancelled_queries_read_nothing() {
        let source = source(fixture(
            3,
            &[
                vec![(0, 100), (1, 100), (2, 100)],
                vec![(0, 200), (1, 200), (2, 200)],
            ],
        ));
        let streams = streams(&source);
        assert!(matches!(
            collect_timestamp_window(
                &source,
                &streams,
                range(0, 201),
                TimestampQueryLimits::default(),
                || true
            ),
            Err(WindowQueryError::Source(McapSourceError::QueryCancelled))
        ));
        assert_eq!(source.index_read_metrics().reads.load(Relaxed), 0);
        assert!(matches!(
            collect_timestamp_window(
                &source,
                &streams,
                range(0, 201),
                TimestampQueryLimits {
                    max_index_bytes: 1,
                    ..Default::default()
                },
                || false
            ),
            Err(WindowQueryError::Limit {
                resource: "Message Index bytes",
                ..
            })
        ));
        assert_eq!(source.index_read_metrics().reads.load(Relaxed), 0);
        assert!(matches!(
            collect_timestamp_window(
                &source,
                &streams,
                range(0, 201),
                TimestampQueryLimits {
                    max_samples: 2,
                    ..Default::default()
                },
                || false
            ),
            Err(WindowQueryError::Limit {
                resource: "timestamp samples",
                ..
            })
        ));
        assert_eq!(source.index_read_metrics().resident_spans.load(Relaxed), 0);
    }
    #[test]
    fn timestamps_do_not_read_message_payloads_or_admit_future_samples() {
        let mut bytes = fixture(
            3,
            &[
                vec![(0, 100), (1, 100), (2, 300)],
                vec![(0, 200), (1, 200), (2, 400)],
            ],
        );
        let summary = Summary::read(&bytes).unwrap().unwrap();
        for chunk in &summary.chunk_indexes {
            bytes[chunk.compressed_data_offset().unwrap() as usize] = 0;
        }
        let source = source(bytes);
        let result = collect_timestamp_window(
            &source,
            &streams(&source),
            range(0, 201),
            TimestampQueryLimits::default(),
            || false,
        )
        .unwrap();
        let future = source.catalog().by_topic("/topic/002").unwrap().id;
        assert!(result[&future].arrivals.is_empty());
        assert_eq!(result[&future].latest_arrival, None);
        assert!(
            result
                .values()
                .flat_map(|samples| &samples.arrivals)
                .all(|time| time.0 <= 200)
        );
        assert_eq!(source.index_read_metrics().reads.load(Relaxed), 2);
    }
    #[test]
    fn unusual_chunk_times_match_a_flat_timestamp_reference() {
        const STREAMS: usize = 12;
        const S: u64 = 1_000_000_000;
        let mut seed = 17_u64;
        let chunks = (0..37)
            .map(|_| {
                (0..STREAMS)
                    .map(|stream| {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                        (stream, (seed % 100) * S)
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        for cursor in [0, 19, 60, 120] {
            let source = source(fixture(STREAMS, &chunks));
            let query_range = range((cursor - 10) * S as i64, cursor * S as i64 + 1);
            let mut result = collect_timestamp_window(
                &source,
                &streams(&source),
                query_range,
                TimestampQueryLimits::default(),
                || false,
            )
            .unwrap();
            for stream in 0..STREAMS {
                let id = source
                    .catalog()
                    .by_topic(&format!("/topic/{stream:03}"))
                    .unwrap()
                    .id;
                let times = chunks
                    .iter()
                    .flat_map(|chunk| chunk.iter())
                    .filter(|(channel, _)| *channel == stream)
                    .map(|(_, time)| ArrivalTime(*time as i64))
                    .collect::<Vec<_>>();
                let mut expected = times
                    .iter()
                    .copied()
                    .filter(|time| query_range.contains(*time))
                    .collect::<Vec<_>>();
                expected.sort_unstable();
                result.get_mut(&id).unwrap().arrivals.sort_unstable();
                assert_eq!(result[&id].arrivals, expected);
                assert_eq!(
                    result[&id].latest_arrival,
                    times
                        .into_iter()
                        .filter(|time| time.0 <= cursor * S as i64)
                        .max()
                );
            }
            assert!(source.index_read_metrics().reads.load(Relaxed) <= chunks.len());
            assert!(
                source
                    .index_read_metrics()
                    .peak_resident_spans
                    .load(Relaxed)
                    <= 1
            );
        }
    }

    #[test]
    fn cancellation_during_index_parsing_releases_data_and_stops_further_reads() {
        let source = source(fixture(
            3,
            &[
                vec![(0, 100), (1, 100), (2, 100)],
                vec![(0, 200), (1, 200), (2, 200)],
            ],
        ));
        let result = collect_timestamp_window(
            &source,
            &streams(&source),
            range(0, 201),
            TimestampQueryLimits::default(),
            || source.index_read_metrics().reads.load(Relaxed) > 0,
        );
        assert!(matches!(
            result,
            Err(WindowQueryError::Source(McapSourceError::QueryCancelled))
        ));
        assert_eq!(source.index_read_metrics().reads.load(Relaxed), 1);
        assert_eq!(source.index_read_metrics().resident_spans.load(Relaxed), 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn source_mutation_after_the_last_index_read_rejects_the_entire_query() {
        let bytes = fixture(3, &[vec![(0, 100), (1, 100), (2, 100)]]);
        let path = std::env::temp_dir().join(format!(
            "mcap-timestamp-mutation-{}.mcap",
            std::process::id()
        ));
        std::fs::write(&path, &bytes).unwrap();
        let source = McapSource::open_file(&path).unwrap();
        let mut changed = false;
        let result = crate::query_topic_health(
            &source,
            crate::TopicHealthRequirement::default(),
            ArrivalTime(100),
            || {
                if !changed && source.index_read_metrics().reads.load(Relaxed) > 0 {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(&path)
                        .unwrap()
                        .set_len(bytes.len() as u64 + 1)
                        .unwrap();
                    changed = true;
                }
                false
            },
        );
        assert!(changed);
        assert!(matches!(
            result,
            Err(WindowQueryError::Source(McapSourceError::SourceChanged))
        ));
        assert_eq!(source.index_read_metrics().resident_spans.load(Relaxed), 0);
        drop(source);
        std::fs::remove_file(path).unwrap();
    }
}
