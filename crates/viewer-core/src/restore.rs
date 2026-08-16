use crate::{ArrivalTime, DataWindowTimeRange, SourceCatalog, StreamId};
use std::{collections::BTreeMap, fmt, time::Duration};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreSemantics {
    /// Restore the exact predecessor on the MCAP log-time timeline.
    LatestBefore,
    /// Restore the bounded feature history ending at the target.
    History(Duration),
    /// Replay the session's small, source-bootstrapped persistent archive through the target.
    Persistent,
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
    pub latest_before: Vec<StreamId>,
    pub histories: Vec<RestoreRead>,
    pub persistent: Vec<StreamId>,
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
        let mut latest_before = Vec::new();
        let mut persistent = Vec::new();
        let mut histories = BTreeMap::<ArrivalTime, Vec<StreamId>>::new();
        for input in inputs {
            match input.semantics {
                RestoreSemantics::LatestBefore => latest_before.push(input.stream_id),
                RestoreSemantics::History(duration) => {
                    let start = ArrivalTime(
                        target
                            .0
                            .saturating_sub(duration_ns(duration))
                            .max(recording.start.0),
                    );
                    histories.entry(start).or_default().push(input.stream_id);
                }
                RestoreSemantics::Persistent => persistent.push(input.stream_id),
            }
        }
        latest_before.sort_by_key(|stream| stream.0);
        latest_before.dedup();
        persistent.sort_by_key(|stream| stream.0);
        persistent.dedup();
        let histories = histories
            .into_iter()
            .map(|(start, mut streams)| {
                streams.sort_by_key(|stream| stream.0);
                streams.dedup();
                RestoreRead {
                    streams,
                    range: DataWindowTimeRange::new(start, end_exclusive)
                        .expect("target-inclusive history range is ordered"),
                }
            })
            .collect();
        Ok(RestorePlan {
            target,
            latest_before,
            histories,
            persistent,
        })
    }
}

fn duration_ns(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RecordingTimeRange, StreamDescriptor, StreamTimingSummary};

    fn catalog() -> SourceCatalog {
        SourceCatalog {
            time_range: Some(RecordingTimeRange {
                start: ArrivalTime(1_000_000_000),
                end_exclusive: ArrivalTime(31_000_000_000),
            }),
            streams: vec![StreamDescriptor {
                id: StreamId(7),
                topic: "/camera".into(),
                schema: "sensor_msgs/msg/CompressedImage".into(),
                message_encoding: "cdr".into(),
                timing: StreamTimingSummary {
                    message_count: Some(10),
                },
            }],
        }
    }

    #[test]
    fn latest_history_and_persistent_are_separate_physical_requests() {
        let target = ArrivalTime(21_000_000_000);
        let plan = RestorePlanner::new(&catalog())
            .plan(
                target,
                [
                    RestoreInput {
                        stream_id: StreamId(7),
                        semantics: RestoreSemantics::LatestBefore,
                    },
                    RestoreInput {
                        stream_id: StreamId(8),
                        semantics: RestoreSemantics::History(Duration::from_secs(1)),
                    },
                    RestoreInput {
                        stream_id: StreamId(9),
                        semantics: RestoreSemantics::Persistent,
                    },
                ],
            )
            .unwrap();
        assert_eq!(plan.latest_before, vec![StreamId(7)]);
        assert_eq!(plan.persistent, vec![StreamId(9)]);
        assert_eq!(plan.histories[0].range.start, ArrivalTime(20_000_000_000));
        assert_eq!(
            plan.histories[0].range.end_exclusive,
            ArrivalTime(target.0 + 1)
        );
    }
}
