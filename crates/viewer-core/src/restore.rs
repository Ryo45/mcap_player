use crate::{ArrivalTime, DataWindowTimeRange, SourceCatalog, StreamId};
use std::{collections::BTreeMap, fmt, time::Duration};

pub const RECENT_SAMPLE_PERIODS: u64 = 4;
pub const MIN_RECENT_LOOKBACK: Duration = Duration::from_millis(250);
pub const MAX_RECENT_LOOKBACK: Duration = Duration::from_secs(20);
pub const DEFAULT_RECENT_LOOKBACK: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreSemantics {
    RecentSample,
    History(Duration),
    Persistent,
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RestoreInput {
    pub stream_id: StreamId,
    pub semantics: RestoreSemantics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreRead {
    pub streams: Vec<StreamId>,
    pub range: DataWindowTimeRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestorePlan {
    pub target: ArrivalTime,
    pub reads: Vec<RestoreRead>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestorePlanError(String);

impl fmt::Display for RestorePlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RestorePlanError {}

pub struct RestorePlanner<'a> {
    catalog: &'a SourceCatalog,
}

impl<'a> RestorePlanner<'a> {
    pub fn new(catalog: &'a SourceCatalog) -> Self {
        Self { catalog }
    }

    pub fn plan(
        &self,
        target: ArrivalTime,
        inputs: impl IntoIterator<Item = RestoreInput>,
    ) -> Result<RestorePlan, RestorePlanError> {
        let recording = self.catalog.time_range.ok_or_else(|| {
            RestorePlanError("restore planning requires a recording time range".into())
        })?;
        if target < recording.start || target >= recording.end_exclusive {
            return Err(RestorePlanError(format!(
                "restore target {} is outside recording [{}, {})",
                target.0, recording.start.0, recording.end_exclusive.0
            )));
        }
        let end_exclusive = ArrivalTime(
            target
                .0
                .checked_add(1)
                .ok_or_else(|| RestorePlanError("restore target cannot be made inclusive".into()))?
                .min(recording.end_exclusive.0),
        );
        let mut grouped = BTreeMap::<ArrivalTime, Vec<StreamId>>::new();
        for input in inputs {
            if input.semantics == RestoreSemantics::None {
                continue;
            }
            let start = match input.semantics {
                RestoreSemantics::RecentSample => ArrivalTime(
                    target
                        .0
                        .saturating_sub(duration_ns(self.recent_lookback(input.stream_id)))
                        .max(recording.start.0),
                ),
                RestoreSemantics::History(duration) => ArrivalTime(
                    target
                        .0
                        .saturating_sub(duration_ns(duration))
                        .max(recording.start.0),
                ),
                RestoreSemantics::Persistent => recording.start,
                RestoreSemantics::None => continue,
            };
            grouped.entry(start).or_default().push(input.stream_id);
        }
        let reads = grouped
            .into_iter()
            .map(|(start, mut streams)| {
                streams.sort_by_key(|stream| stream.0);
                streams.dedup();
                let range = DataWindowTimeRange::new(start, end_exclusive)
                    .expect("target-inclusive restore range is ordered");
                RestoreRead { streams, range }
            })
            .collect();
        Ok(RestorePlan { target, reads })
    }

    pub fn recent_lookback(&self, stream_id: StreamId) -> Duration {
        let Some(recording) = self.catalog.time_range else {
            return DEFAULT_RECENT_LOOKBACK;
        };
        let Some(count) = self
            .catalog
            .by_id(stream_id)
            .and_then(|stream| stream.timing.message_count)
            .filter(|count| *count != 0)
        else {
            return DEFAULT_RECENT_LOOKBACK;
        };
        let duration = recording
            .end_exclusive
            .0
            .saturating_sub(recording.start.0)
            .max(0) as u64;
        if duration == 0 {
            return DEFAULT_RECENT_LOOKBACK;
        }
        let period = duration.saturating_add(count - 1) / count;
        let lookback = period.saturating_mul(RECENT_SAMPLE_PERIODS);
        Duration::from_nanos(lookback).clamp(MIN_RECENT_LOOKBACK, MAX_RECENT_LOOKBACK)
    }
}

fn duration_ns(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RecordingTimeRange, StreamDescriptor, StreamTimingSummary};

    fn catalog(count: Option<u64>, duration: Duration) -> SourceCatalog {
        SourceCatalog {
            time_range: Some(RecordingTimeRange {
                start: ArrivalTime(1_000_000_000),
                end_exclusive: ArrivalTime(
                    1_000_000_000 + i64::try_from(duration.as_nanos()).unwrap(),
                ),
            }),
            streams: vec![StreamDescriptor {
                id: StreamId(7),
                topic: "/camera".into(),
                schema: "sensor_msgs/msg/CompressedImage".into(),
                message_encoding: "cdr".into(),
                timing: StreamTimingSummary {
                    message_count: count,
                },
            }],
        }
    }

    #[test]
    fn recent_sample_uses_recording_count_and_minimum_clamp() {
        let catalog = catalog(Some(1_000), Duration::from_secs(10));
        assert_eq!(
            RestorePlanner::new(&catalog).recent_lookback(StreamId(7)),
            MIN_RECENT_LOOKBACK
        );
    }

    #[test]
    fn recent_sample_uses_maximum_clamp_and_missing_fallback() {
        let sparse = catalog(Some(1), Duration::from_secs(100));
        assert_eq!(
            RestorePlanner::new(&sparse).recent_lookback(StreamId(7)),
            MAX_RECENT_LOOKBACK
        );
        let missing = catalog(None, Duration::from_secs(100));
        assert_eq!(
            RestorePlanner::new(&missing).recent_lookback(StreamId(7)),
            DEFAULT_RECENT_LOOKBACK
        );
    }

    #[test]
    fn persistent_and_history_ranges_are_explicit_and_target_inclusive() {
        let catalog = catalog(Some(10), Duration::from_secs(30));
        let target = ArrivalTime(21_000_000_000);
        let plan = RestorePlanner::new(&catalog)
            .plan(
                target,
                [
                    RestoreInput {
                        stream_id: StreamId(7),
                        semantics: RestoreSemantics::Persistent,
                    },
                    RestoreInput {
                        stream_id: StreamId(8),
                        semantics: RestoreSemantics::History(Duration::from_secs(1)),
                    },
                ],
            )
            .unwrap();
        assert_eq!(plan.reads.len(), 2);
        assert_eq!(plan.reads[0].range.start, ArrivalTime(1_000_000_000));
        assert_eq!(plan.reads[0].range.end_exclusive, ArrivalTime(target.0 + 1));
        assert_eq!(plan.reads[1].range.start, ArrivalTime(20_000_000_000));
    }
}
