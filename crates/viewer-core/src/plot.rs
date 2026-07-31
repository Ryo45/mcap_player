use crate::{ArrivalTime, McapOpenError, MeasurementTime, ODOM_TOPIC, decode_odometry};
use mcap::MessageStream;

const NANOS_PER_SECOND: f64 = 1_000_000_000.0;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SignalId {
    Speed,
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
    /// Full-resolution samples used for current-value lookup.
    pub samples: Vec<SignalSample>,
    /// Bounded, display-oriented series. Playback never rebuilds this value.
    pub display: PlotSeries,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum PlotMode {
    #[default]
    Overview,
    Follow {
        history_seconds: f64,
        lookahead_seconds: f64,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlotViewport {
    pub start_seconds: f64,
    pub end_seconds: f64,
}

impl PlotViewport {
    pub fn new(start_seconds: f64, end_seconds: f64) -> Self {
        Self {
            start_seconds,
            end_seconds: end_seconds.max(start_seconds + f64::EPSILON),
        }
    }

    pub fn width(self) -> f64 {
        self.end_seconds - self.start_seconds
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlotPanelState {
    pub mode: PlotMode,
    pub viewport: PlotViewport,
    pub selected_signals: Vec<SignalId>,
}

impl PlotPanelState {
    pub fn overview(start_seconds: f64, end_seconds: f64) -> Self {
        Self {
            mode: PlotMode::Overview,
            viewport: PlotViewport::new(start_seconds, end_seconds),
            selected_signals: vec![SignalId::Speed],
        }
    }

    pub fn follow(&mut self, playhead: f64) {
        self.mode = PlotMode::Follow {
            history_seconds: 8.0,
            lookahead_seconds: 2.0,
        };
        self.viewport = followed_viewport(playhead, 8.0, 2.0);
    }

    pub fn overview_with_viewport(&mut self, viewport: PlotViewport) {
        self.mode = PlotMode::Overview;
        self.viewport = viewport;
    }

    /// Shifts only while playing and only after the playhead crosses a follow boundary.
    pub fn update_follow(&mut self, playhead: f64, playing: bool) -> bool {
        if !playing {
            return false;
        }
        let PlotMode::Follow {
            history_seconds,
            lookahead_seconds,
        } = self.mode
        else {
            return false;
        };
        if !should_shift_viewport(&self.viewport, playhead) {
            return false;
        }
        self.viewport = followed_viewport(playhead, history_seconds, lookahead_seconds);
        true
    }
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

pub fn sample_at_or_before(samples: &[SignalSample], cursor: ArrivalTime) -> Option<&SignalSample> {
    let index = samples.partition_point(|sample| sample.arrival_time <= cursor);
    index.checked_sub(1).and_then(|index| samples.get(index))
}

pub fn should_shift_viewport(viewport: &PlotViewport, playhead: f64) -> bool {
    let threshold = viewport.start_seconds + viewport.width() * 0.8;
    playhead < viewport.start_seconds || playhead > threshold
}

pub fn followed_viewport(
    playhead: f64,
    history_seconds: f64,
    lookahead_seconds: f64,
) -> PlotViewport {
    PlotViewport::new(
        playhead - history_seconds.max(0.0),
        playhead + lookahead_seconds.max(0.0),
    )
}

/// Reduces ordered samples to a min/max envelope while retaining temporal order.
pub fn downsample_min_max(samples: &[SignalSample], max_points: usize) -> Vec<SignalSample> {
    if samples.len() <= max_points {
        return samples.to_vec();
    }
    if max_points == 0 {
        return Vec::new();
    }
    if max_points == 1 {
        return vec![samples[0]];
    }

    let bucket_count = max_points / 2;
    let mut output = Vec::with_capacity(bucket_count * 2);
    for bucket in 0..bucket_count {
        let start = bucket * samples.len() / bucket_count;
        let end = ((bucket + 1) * samples.len() / bucket_count).min(samples.len());
        let slice = &samples[start..end];
        let min_index = slice
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| left.value.total_cmp(&right.value))
            .map(|(index, _)| index)
            .expect("a min/max bucket is never empty");
        let max_index = slice
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| left.value.total_cmp(&right.value))
            .map(|(index, _)| index)
            .expect("a min/max bucket is never empty");
        if min_index <= max_index {
            output.push(slice[min_index]);
            if min_index != max_index {
                output.push(slice[max_index]);
            }
        } else {
            output.push(slice[max_index]);
            output.push(slice[min_index]);
        }
    }
    output
}

/// Scans `/odom` in an MCAP and builds the speed signal.
///
/// This function is synchronous by design; native callers run it on their plot-loading worker.
pub fn load_speed_signal(
    backing: &[u8],
    origin: ArrivalTime,
    max_display_points: usize,
) -> Result<Option<LoadedSignal>, McapOpenError> {
    let mut samples = Vec::new();
    for message in MessageStream::new(backing)? {
        let message = message?;
        if message.channel.topic != ODOM_TOPIC
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
        let speed = vx.hypot(vy);
        if speed.is_finite() {
            samples.push(SignalSample {
                measurement_time: Some(odometry.measurement_time),
                arrival_time: ArrivalTime(arrival_ns),
                value: speed,
            });
        }
    }
    if samples.is_empty() {
        return Ok(None);
    }
    samples.sort_by_key(|sample| sample.arrival_time);
    let display_samples = downsample_min_max(&samples, max_display_points);
    let display = PlotSeries {
        signal_id: SignalId::Speed,
        origin,
        x_seconds: display_samples
            .iter()
            .map(|sample| cursor_seconds(sample.arrival_time, origin))
            .collect(),
        values: display_samples.iter().map(|sample| sample.value).collect(),
    };
    Ok(Some(LoadedSignal { samples, display }))
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

    fn odometry_cdr(measurement_ns: i64, vx: f64, vy: f64) -> Vec<u8> {
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
        for value in [vx, vy, 0.0, 0.0, 0.0, 0.0] {
            push_f64(&mut output, value);
        }
        for _ in 0..36 {
            push_f64(&mut output, 0.0);
        }
        output
    }

    fn odometry_mcap(messages: &[(i64, i64, f64, f64)]) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer =
                Writer::with_options(&mut bytes, WriteOptions::new().use_chunks(false)).unwrap();
            let schema = writer
                .add_schema("nav_msgs/msg/Odometry", "ros2msg", b"")
                .unwrap();
            let channel = writer
                .add_channel(schema, ODOM_TOPIC, "cdr", &BTreeMap::new())
                .unwrap();
            for (sequence, &(arrival, measurement, vx, vy)) in messages.iter().enumerate() {
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence: sequence as u32,
                            log_time: arrival as u64,
                            publish_time: measurement as u64,
                        },
                        &odometry_cdr(measurement, vx, vy),
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
    fn current_value_uses_full_resolution_arrival_samples() {
        let samples = vec![sample(10, 1.0), sample(20, 2.0), sample(30, 3.0)];
        assert!(sample_at_or_before(&samples, ArrivalTime(9)).is_none());
        assert_eq!(
            sample_at_or_before(&samples, ArrivalTime(25)).map(|sample| sample.value),
            Some(2.0)
        );
    }

    #[test]
    fn envelope_keeps_each_bucket_extrema_in_time_order() {
        let samples = vec![
            sample(0, 3.0),
            sample(1, -2.0),
            sample(2, 7.0),
            sample(3, 4.0),
            sample(4, 9.0),
            sample(5, 1.0),
            sample(6, 8.0),
            sample(7, 2.0),
        ];
        let envelope = downsample_min_max(&samples, 4);
        assert_eq!(envelope.len(), 4);
        assert_eq!(
            envelope
                .iter()
                .map(|sample| (sample.arrival_time.0, sample.value))
                .collect::<Vec<_>>(),
            vec![(1, -2.0), (2, 7.0), (4, 9.0), (5, 1.0)]
        );
    }

    #[test]
    fn follow_shifts_at_boundary_but_not_while_paused() {
        let mut panel = PlotPanelState::overview(0.0, 100.0);
        panel.follow(20.0);
        assert_eq!(panel.viewport, PlotViewport::new(12.0, 22.0));
        assert!(!panel.update_follow(21.0, false));
        assert!(!panel.update_follow(19.9, true));
        assert!(panel.update_follow(20.1, true));
        assert!((panel.viewport.start_seconds - 12.1).abs() < 1e-12);
        assert!((panel.viewport.end_seconds - 22.1).abs() < 1e-12);
    }

    #[test]
    fn loads_speed_by_arrival_time_and_keeps_measurement_time_separate() {
        let origin = ArrivalTime(1_000_000_000);
        let bytes = odometry_mcap(&[
            (1_500_000_000, 10_000_000_000, 3.0, 4.0),
            (2_500_000_000, 11_000_000_000, 5.0, 12.0),
        ]);
        let loaded = load_speed_signal(&bytes, origin, 4)
            .unwrap()
            .expect("speed signal");
        assert_eq!(loaded.samples.len(), 2);
        assert_eq!(loaded.samples[0].arrival_time, ArrivalTime(1_500_000_000));
        assert_eq!(
            loaded.samples[0].measurement_time,
            Some(MeasurementTime(10_000_000_000))
        );
        assert_eq!(loaded.samples[0].value, 5.0);
        assert_eq!(loaded.samples[1].value, 13.0);
        assert_eq!(loaded.display.x_seconds, vec![0.5, 1.5]);
    }
}
