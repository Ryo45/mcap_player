use crate::{ArrivalTime, McapOpenError, MeasurementTime, decode_odometry};
use mcap::{MessageStream, Summary};
use serde::{Deserialize, Serialize};

const NANOS_PER_SECOND: f64 = 1_000_000_000.0;
const ODOMETRY_PROGRESS_INTERVAL: usize = 65_536;

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

/// Scans the configured Odometry topic in an MCAP and builds the speed signal.
///
/// This function is synchronous by design; native callers run it on their plot-loading worker.
pub fn load_speed_signal(
    backing: &[u8],
    origin: ArrivalTime,
    max_display_points: usize,
    odometry_topic: &str,
) -> Result<Option<LoadedSignal>, McapOpenError> {
    Ok(load_odometry_signals(backing, origin, max_display_points, odometry_topic)?.speed)
}

/// Scans the configured Odometry topic in an MCAP and builds the yaw-rate signal.
///
/// This remains a plot query: it does not add history to continuous feature-controller state.
pub fn load_yaw_rate_signal(
    backing: &[u8],
    origin: ArrivalTime,
    max_display_points: usize,
    odometry_topic: &str,
) -> Result<Option<LoadedSignal>, McapOpenError> {
    Ok(load_odometry_signals(backing, origin, max_display_points, odometry_topic)?.yaw_rate)
}

/// Scans the configured Odometry topic once and derives both Native plot signals.
pub fn load_odometry_signals(
    backing: &[u8],
    origin: ArrivalTime,
    max_display_points: usize,
    odometry_topic: &str,
) -> Result<LoadedOdometrySignals, McapOpenError> {
    load_odometry_signals_for_topic_with_progress(
        backing,
        origin,
        max_display_points,
        odometry_topic,
        |_| {},
    )
}

/// Scans the configured Odometry topic once, periodically publishing bounded snapshots while
/// retaining the exact final result.
///
/// Native uses the progress callback to avoid keeping large compressed recordings behind a
/// loading placeholder until the complete sequential query finishes. The callback remains a
/// concrete plot-query concern; it does not feed signal history into continuous controller state.
pub fn load_odometry_signals_for_topic_with_progress(
    backing: &[u8],
    origin: ArrivalTime,
    max_display_points: usize,
    odometry_topic: &str,
    mut on_progress: impl FnMut(LoadedOdometrySignals),
) -> Result<LoadedOdometrySignals, McapOpenError> {
    let end_exclusive = Summary::read(backing)?
        .and_then(|summary| summary.stats)
        .and_then(|stats| stats.message_end_time.checked_add(1))
        .and_then(|time| i64::try_from(time).ok())
        .map(ArrivalTime)
        .unwrap_or(ArrivalTime(origin.0.saturating_add(1)));
    let mut speed = SignalOverviewReducer::new(origin, end_exclusive, max_display_points);
    let mut yaw_rate = SignalOverviewReducer::new(origin, end_exclusive, max_display_points);
    let mut odometry_count = 0_usize;
    for message in MessageStream::new(backing)? {
        let message = message?;
        if message.channel.topic != odometry_topic
            || message
                .channel
                .schema
                .as_ref()
                .is_none_or(|schema| schema.name != "nav_msgs/msg/Odometry")
        {
            continue;
        }
        let Ok(arrival_ns) = i64::try_from(message.log_time) else {
            return Err(McapOpenError::TimestampOverflow);
        };
        let Ok(odometry) = decode_odometry(&message.data) else {
            continue;
        };
        let [vx, vy, _] = odometry.linear_velocity;
        let speed_value = vx.hypot(vy);
        let yaw_rate_value = odometry.angular_velocity[2];
        let sample = |value| SignalSample {
            measurement_time: Some(odometry.measurement_time),
            arrival_time: ArrivalTime(arrival_ns),
            value,
        };
        if speed_value.is_finite() {
            speed.push(sample(speed_value));
        }
        if yaw_rate_value.is_finite() {
            yaw_rate.push(sample(yaw_rate_value));
        }
        odometry_count = odometry_count.saturating_add(1);
        if odometry_count == 1 || odometry_count.is_multiple_of(ODOMETRY_PROGRESS_INTERVAL) {
            on_progress(snapshot_odometry_signals(&speed, &yaw_rate));
        }
    }
    Ok(LoadedOdometrySignals {
        speed: speed.finish(SignalId::Speed),
        yaw_rate: yaw_rate.finish(SignalId::YawRate),
    })
}

fn snapshot_odometry_signals(
    speed: &SignalOverviewReducer,
    yaw_rate: &SignalOverviewReducer,
) -> LoadedOdometrySignals {
    LoadedOdometrySignals {
        speed: speed.finish(SignalId::Speed),
        yaw_rate: yaw_rate.finish(SignalId::YawRate),
    }
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

    pub fn finish(&self, signal_id: SignalId) -> Option<LoadedSignal> {
        if self.input_sample_count == 0 {
            return None;
        }
        let mut display_samples = Vec::with_capacity(self.retained_points());
        for extrema in self.buckets.iter().flatten() {
            if self.max_points == 1 || extrema.min == extrema.max {
                display_samples.push(extrema.min);
            } else if extrema.min.arrival_time <= extrema.max.arrival_time {
                display_samples.extend([extrema.min, extrema.max]);
            } else {
                display_samples.extend([extrema.max, extrema.min]);
            }
        }
        debug_assert!(display_samples.len() <= self.max_points);
        Some(LoadedSignal {
            display: PlotSeries {
                signal_id,
                origin: self.origin,
                x_seconds: display_samples
                    .iter()
                    .map(|sample| cursor_seconds(sample.arrival_time, self.origin))
                    .collect(),
                values: display_samples.iter().map(|sample| sample.value).collect(),
            },
            input_sample_count: self.input_sample_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcap::{WriteOptions, Writer, records::MessageHeader};
    use std::{collections::BTreeMap, io::Cursor};

    fn sample(arrival: i64, value: f64) -> SignalSample {
        SignalSample {
            measurement_time: Some(MeasurementTime(arrival - 1)),
            arrival_time: ArrivalTime(arrival),
            value,
        }
    }

    fn align_cdr(output: &mut Vec<u8>, alignment: usize) {
        let relative = output.len() - 4;
        output.resize(
            output.len() + (alignment - relative % alignment) % alignment,
            0,
        );
    }

    fn push_u32(output: &mut Vec<u8>, value: u32) {
        align_cdr(output, 4);
        output.extend_from_slice(&value.to_le_bytes());
    }

    fn push_string(output: &mut Vec<u8>, value: &str) {
        push_u32(output, (value.len() + 1) as u32);
        output.extend_from_slice(value.as_bytes());
        output.push(0);
    }

    fn push_f64(output: &mut Vec<u8>, value: f64) {
        align_cdr(output, 8);
        output.extend_from_slice(&value.to_le_bytes());
    }

    fn odometry_cdr(measurement_ns: i64, vx: f64, vy: f64, yaw_rate: f64) -> Vec<u8> {
        let mut output = vec![0, 1, 0, 0];
        push_u32(&mut output, measurement_ns.div_euclid(1_000_000_000) as u32);
        push_u32(&mut output, measurement_ns.rem_euclid(1_000_000_000) as u32);
        push_string(&mut output, "odom");
        push_string(&mut output, "base_link");
        for value in [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0] {
            push_f64(&mut output, value);
        }
        for _ in 0..36 {
            push_f64(&mut output, 0.0);
        }
        for value in [vx, vy, 0.0, 0.0, 0.0, yaw_rate] {
            push_f64(&mut output, value);
        }
        for _ in 0..36 {
            push_f64(&mut output, 0.0);
        }
        output
    }

    fn odometry_mcap(messages: &[(i64, i64, f64, f64, f64)]) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer =
                Writer::with_options(&mut bytes, WriteOptions::new().use_chunks(false)).unwrap();
            let schema = writer
                .add_schema("nav_msgs/msg/Odometry", "ros2msg", b"")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/odom", "cdr", &BTreeMap::new())
                .unwrap();
            for (sequence, &(arrival, measurement, vx, vy, yaw_rate)) in messages.iter().enumerate()
            {
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence: sequence as u32,
                            log_time: arrival as u64,
                            publish_time: measurement as u64,
                        },
                        &odometry_cdr(measurement, vx, vy, yaw_rate),
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        bytes.into_inner()
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
    fn loads_speed_by_arrival_time_and_keeps_measurement_time_separate() {
        let origin = ArrivalTime(1_000_000_000);
        let bytes = odometry_mcap(&[
            (1_500_000_000, 10_000_000_000, 3.0, 4.0, 0.25),
            (2_500_000_000, 11_000_000_000, 5.0, 12.0, -0.5),
        ]);
        let loaded = load_speed_signal(&bytes, origin, 4, "/odom")
            .unwrap()
            .expect("speed signal");
        assert_eq!(loaded.input_sample_count, 2);
        assert_eq!(loaded.display.values, vec![5.0, 13.0]);
        assert_eq!(loaded.display.x_seconds, vec![0.5, 1.5]);
    }

    #[test]
    fn loads_yaw_rate_as_a_panel_query_without_changing_speed_semantics() {
        let origin = ArrivalTime(1_000_000_000);
        let bytes = odometry_mcap(&[
            (1_500_000_000, 10_000_000_000, 3.0, 4.0, 0.25),
            (2_500_000_000, 11_000_000_000, 5.0, 12.0, -0.5),
        ]);
        let loaded = load_yaw_rate_signal(&bytes, origin, 4, "/odom")
            .unwrap()
            .expect("yaw-rate signal");
        assert_eq!(loaded.display.signal_id, SignalId::YawRate);
        assert_eq!(loaded.input_sample_count, 2);
        assert_eq!(loaded.display.values, vec![0.25, -0.5]);
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
