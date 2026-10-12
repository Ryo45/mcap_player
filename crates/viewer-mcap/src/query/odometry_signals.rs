//! MCAP-backed plot loading; reduction stays in viewer-core.
use crate::{
    ArrivalTime, LoadedOdometrySignals, LoadedSignal, McapSourceError, SignalId,
    SignalOverviewReducer, SignalSample, decode_odometry,
};
use mcap::{MessageStream, Summary};
const ODOMETRY_PROGRESS_INTERVAL: usize = 65_536;
/// Scans the configured Odometry topic in an MCAP and builds the speed signal.
///
/// This function is synchronous by design; native callers run it on their plot-loading worker.
pub fn load_speed_signal(
    backing: &[u8],
    origin: ArrivalTime,
    max_display_points: usize,
    odometry_topic: &str,
) -> Result<Option<LoadedSignal>, McapSourceError> {
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
) -> Result<Option<LoadedSignal>, McapSourceError> {
    Ok(load_odometry_signals(backing, origin, max_display_points, odometry_topic)?.yaw_rate)
}

/// Scans the configured Odometry topic once and derives both Native plot signals.
pub fn load_odometry_signals(
    backing: &[u8],
    origin: ArrivalTime,
    max_display_points: usize,
    odometry_topic: &str,
) -> Result<LoadedOdometrySignals, McapSourceError> {
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
) -> Result<LoadedOdometrySignals, McapSourceError> {
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
            return Err(McapSourceError::TimestampOverflow);
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

impl crate::McapSource {
    pub fn load_odometry_overview(
        &self,
        origin: ArrivalTime,
        max_points: usize,
        topic: &str,
        mut on_progress: impl FnMut(LoadedOdometrySignals),
    ) -> Result<LoadedOdometrySignals, McapSourceError> {
        let Some(stream) = self.catalog().streams.iter().find(|s| {
            s.topic == topic && s.schema == "nav_msgs/msg/Odometry" && s.message_encoding == "cdr"
        }) else {
            return Ok(LoadedOdometrySignals::default());
        };
        let range = self.time_range();
        let mut speed = SignalOverviewReducer::new(origin, range.end_exclusive, max_points);
        let mut yaw = SignalOverviewReducer::new(origin, range.end_exclusive, max_points);
        let mut count = 0usize;
        self.visit_stream_range(
            stream.id,
            crate::DataWindowTimeRange::new(range.start, range.end_exclusive)
                .expect("validated recording range"),
            || false,
            |message| {
                if let Ok(odometry) = decode_odometry(&message.payload) {
                    let [vx, vy, _] = odometry.linear_velocity;
                    let sample = |value| SignalSample {
                        measurement_time: Some(odometry.measurement_time),
                        arrival_time: message.arrival_time,
                        value,
                    };
                    speed.push(sample(vx.hypot(vy)));
                    yaw.push(sample(odometry.angular_velocity[2]));
                    count += 1;
                    if count == 1 || count.is_multiple_of(ODOMETRY_PROGRESS_INTERVAL) {
                        on_progress(snapshot_odometry_signals(&speed, &yaw));
                    }
                }
            },
        )?;
        Ok(snapshot_odometry_signals(&speed, &yaw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcap::{WriteOptions, Writer, records::MessageHeader};
    use std::{collections::BTreeMap, io::Cursor};
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
}
