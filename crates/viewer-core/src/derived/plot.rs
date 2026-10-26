//! Bounded plot overview reduction and exact visible-range samples.

use crate::{ArrivalTime, MeasurementTime};
use serde::{Deserialize, Serialize};

const NANOS_PER_SECOND: f64 = 1_000_000_000.0;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignalId {
    Speed,
    YawRate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlotSeries {
    pub signal_id: SignalId,
    /// Absolute arrival time used as the relative X-axis origin.
    pub origin: ArrivalTime,
    /// Seconds relative to `origin`.
    pub x_seconds: Vec<f64>,
    pub values: Vec<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SignalSample {
    pub measurement_time: Option<MeasurementTime>,
    pub arrival_time: ArrivalTime,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadedSignal {
    /// Bounded, display-oriented series. Playback never rebuilds this value.
    pub display: PlotSeries,
    /// Number of finite exact inputs reduced into `display`; no exact sample vector is retained.
    pub input_sample_count: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadedOdometrySignals {
    pub speed: Option<LoadedSignal>,
    pub yaw_rate: Option<LoadedSignal>,
}

pub fn cursor_seconds(cursor: ArrivalTime, origin: ArrivalTime) -> f64 {
    cursor.0.saturating_sub(origin.0) as f64 / NANOS_PER_SECOND
}

pub fn arrival_time_from_plot_x(origin: ArrivalTime, x_seconds: f64) -> ArrivalTime {
    let offset = if x_seconds.is_finite() {
        (x_seconds * NANOS_PER_SECOND).round()
    } else {
        0.0
    };
    let offset = offset.clamp(i64::MIN as f64, i64::MAX as f64) as i64;
    ArrivalTime(origin.0.saturating_add(offset))
}

#[derive(Clone, Copy, Debug)]
struct SignalExtrema {
    min: SignalSample,
    max: SignalSample,
}

/// Fixed-size time-bucket reducer for recording-wide Plot overview data.
///
/// Its working set is `O(max_points)` regardless of recording duration or message count.
pub struct SignalOverviewReducer {
    origin: ArrivalTime,
    end_exclusive: ArrivalTime,
    max_points: usize,
    buckets: Vec<Option<SignalExtrema>>,
    input_sample_count: u64,
}

impl SignalOverviewReducer {
    pub fn new(origin: ArrivalTime, end_exclusive: ArrivalTime, max_points: usize) -> Self {
        let bucket_count = match max_points {
            0 => 0,
            1 => 1,
            value => value / 2,
        };
        Self {
            origin,
            end_exclusive: ArrivalTime(end_exclusive.0.max(origin.0.saturating_add(1))),
            max_points,
            buckets: vec![None; bucket_count],
            input_sample_count: 0,
        }
    }

    pub fn push(&mut self, sample: SignalSample) {
        if !sample.value.is_finite() || self.buckets.is_empty() {
            return;
        }
        self.input_sample_count = self.input_sample_count.saturating_add(1);
        let duration = self.end_exclusive.0.saturating_sub(self.origin.0).max(1) as i128;
        let offset = sample.arrival_time.0.saturating_sub(self.origin.0).max(0) as i128;
        let scaled = offset.saturating_mul(self.buckets.len() as i128) / duration;
        let bucket = usize::try_from(scaled)
            .unwrap_or(usize::MAX)
            .min(self.buckets.len() - 1);
        match &mut self.buckets[bucket] {
            Some(extrema) => {
                if sample.value < extrema.min.value {
                    extrema.min = sample;
                }
                if sample.value > extrema.max.value {
                    extrema.max = sample;
                }
            }
            slot @ None => {
                *slot = Some(SignalExtrema {
                    min: sample,
                    max: sample,
                });
            }
        }
    }

    pub fn retained_points(&self) -> usize {
        self.buckets
            .iter()
            .flatten()
            .map(|extrema| usize::from(self.max_points > 1 && extrema.min != extrema.max) + 1)
            .sum()
    }

    pub fn input_sample_count(&self) -> u64 {
        self.input_sample_count
    }

    /// Iterates over the bounded extrema in arrival order within each bucket.
    /// No intermediate sample buffer is allocated; collect only when ownership is needed.
    pub fn samples(&self) -> impl Iterator<Item = SignalSample> + '_ {
        self.buckets.iter().flatten().flat_map(|extrema| {
            let (first, second) = if self.max_points == 1 || extrema.min == extrema.max {
                (extrema.min, None)
            } else if extrema.min.arrival_time <= extrema.max.arrival_time {
                (extrema.min, Some(extrema.max))
            } else {
                (extrema.max, Some(extrema.min))
            };
            std::iter::once(first).chain(second)
        })
    }

    pub fn finish(&self, signal_id: SignalId) -> Option<LoadedSignal> {
        if self.input_sample_count == 0 {
            return None;
        }
        let retained = self.retained_points();
        let mut x_seconds = vec![0.0; retained];
        let mut values = vec![0.0; retained];
        let outputs = x_seconds
            .iter_mut()
            .zip(values.iter_mut())
            .zip(self.samples());
        for ((x, value), sample) in outputs {
            *x = cursor_seconds(sample.arrival_time, self.origin);
            *value = sample.value;
        }
        debug_assert!(values.len() <= self.max_points);
        Some(LoadedSignal {
            display: PlotSeries {
                signal_id,
                origin: self.origin,
                x_seconds,
                values,
            },
            input_sample_count: self.input_sample_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(arrival: i64, value: f64) -> SignalSample {
        SignalSample {
            measurement_time: Some(MeasurementTime(arrival - 1)),
            arrival_time: ArrivalTime(arrival),
            value,
        }
    }

    #[test]
    fn converts_only_against_arrival_origin() {
        let origin = ArrivalTime(1_000_000_000);
        let cursor = ArrivalTime(3_500_000_000);
        let x = cursor_seconds(cursor, origin);
        assert_eq!(x, 2.5);
        assert_eq!(arrival_time_from_plot_x(origin, x), cursor);
    }

    #[test]
    fn streaming_overview_keeps_bucket_extrema_in_time_order() {
        let mut reducer = SignalOverviewReducer::new(ArrivalTime(0), ArrivalTime(8), 4);
        for (arrival, value) in [
            (0, 3.0),
            (1, -2.0),
            (2, 7.0),
            (3, 4.0),
            (4, 9.0),
            (5, 1.0),
            (6, 8.0),
            (7, 2.0),
        ] {
            reducer.push(sample(arrival, value));
        }
        let display = reducer.finish(SignalId::Speed).unwrap().display;
        assert_eq!(display.x_seconds, vec![1e-9, 2e-9, 4e-9, 5e-9]);
        assert_eq!(display.values, vec![-2.0, 7.0, 9.0, 1.0]);
    }

    #[test]
    fn sample_iteration_preserves_order_metadata_and_can_be_consumed_partially() {
        let mut reducer = SignalOverviewReducer::new(ArrivalTime(0), ArrivalTime(8), 4);
        for (arrival, value) in [
            (0, 3.0),
            (1, -2.0),
            (2, 7.0),
            (3, 4.0),
            (4, 9.0),
            (5, 1.0),
            (6, 8.0),
            (7, 2.0),
        ] {
            reducer.push(sample(arrival, value));
        }
        let expected = [
            sample(1, -2.0),
            sample(2, 7.0),
            sample(4, 9.0),
            sample(5, 1.0),
        ];
        assert_eq!(reducer.samples().take(2).collect::<Vec<_>>(), expected[..2]);
        assert_eq!(reducer.samples().collect::<Vec<_>>(), expected);
        assert_eq!(reducer.input_sample_count(), 8);
        assert_eq!(reducer.retained_points(), expected.len());
    }

    #[test]
    fn sample_iteration_respects_zero_one_and_odd_budgets_and_equal_times() {
        for budget in 0..=9 {
            let mut reducer = SignalOverviewReducer::new(ArrivalTime(0), ArrivalTime(10), budget);
            for item in [
                sample(2, 9.0),
                sample(2, -1.0),
                sample(2, 4.0),
                sample(8, 3.0),
            ] {
                reducer.push(item);
            }
            let samples: Vec<_> = reducer.samples().collect();
            assert!(
                samples.len() <= budget,
                "overview exceeded point budget {budget}"
            );
            assert_eq!(samples.len(), reducer.retained_points());
            assert!(
                samples
                    .windows(2)
                    .all(|pair| pair[0].arrival_time <= pair[1].arrival_time)
            );
            if budget == 0 {
                assert!(reducer.finish(SignalId::Speed).is_none());
            } else if budget <= 3 {
                let expected = if budget == 1 {
                    vec![sample(2, -1.0)]
                } else {
                    vec![sample(2, -1.0), sample(2, 9.0)]
                };
                assert_eq!(samples, expected);
            }
        }
        let mut constant = SignalOverviewReducer::new(ArrivalTime(0), ArrivalTime(10), 2);
        constant.push(sample(2, 3.0));
        constant.push(sample(3, 3.0));
        assert_eq!(constant.samples().collect::<Vec<_>>(), [sample(2, 3.0)]);
    }

    #[test]
    fn overview_reducer_memory_is_bounded_independently_of_input_count() {
        let max_points = 400;
        let mut reducer =
            SignalOverviewReducer::new(ArrivalTime(0), ArrivalTime(1_000_001), max_points);
        for arrival in 0..1_000_000 {
            reducer.push(sample(arrival, (arrival % 997) as f64));
        }
        let loaded = reducer.finish(SignalId::Speed).unwrap();
        assert_eq!(loaded.input_sample_count, 1_000_000);
        assert!(reducer.retained_points() <= max_points);
        assert!(loaded.display.values.len() <= max_points);
    }
}
