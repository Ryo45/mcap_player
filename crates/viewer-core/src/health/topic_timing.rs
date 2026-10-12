use crate::ArrivalTime;
use std::time::Duration;

#[derive(Debug, Default, PartialEq)]
pub struct TopicTimingFacts {
    pub sample_count: u64,
    pub observed_rate_hz: Option<f64>,
    pub latest_arrival: Option<ArrivalTime>,
    pub latest_age: Option<Duration>,
    pub recent_gap: Option<Duration>,
    pub max_gap: Option<Duration>,
}

/// Samples must be sorted log times from the requested window, all <= cursor.
/// `latest_before` is an exact predecessor, possibly before the sample window.
/// This function neither scans history nor infers the predecessor from an empty window.
pub fn summarize_topic_timing(
    arrivals: &[ArrivalTime],
    latest_before: Option<ArrivalTime>,
    cursor: ArrivalTime,
) -> TopicTimingFacts {
    let mut facts = TopicTimingFacts {
        sample_count: arrivals.len() as u64,
        latest_arrival: latest_before,
        latest_age: latest_before.map(|last| elapsed(cursor, last)),
        ..Default::default()
    };
    if arrivals.len() < 2 {
        return facts;
    }
    let span = elapsed(*arrivals.last().unwrap(), arrivals[0]);
    if !span.is_zero() {
        facts.observed_rate_hz = Some((arrivals.len() - 1) as f64 / span.as_secs_f64());
    }
    facts.recent_gap = Some(elapsed(
        arrivals[arrivals.len() - 1],
        arrivals[arrivals.len() - 2],
    ));
    facts.max_gap = arrivals
        .windows(2)
        .map(|pair| elapsed(pair[1], pair[0]))
        .max();
    facts
}
fn elapsed(later: ArrivalTime, earlier: ArrivalTime) -> Duration {
    Duration::from_nanos(later.0.abs_diff(earlier.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_window_keeps_exact_predecessor_age_and_absence() {
        let facts = summarize_topic_timing(&[], Some(ArrivalTime(1)), ArrivalTime(2_000_000_000));
        assert_eq!(facts.sample_count, 0);
        assert_eq!(facts.latest_age, Some(Duration::from_nanos(1_999_999_999)));
        assert_eq!(facts.observed_rate_hz, None);
        assert_eq!(
            summarize_topic_timing(&[], None, ArrivalTime(10)),
            TopicTimingFacts::default()
        );
    }
    #[test]
    fn duplicates_and_nanoseconds_preserve_multiplicity_without_zero_division() {
        let times = [ArrivalTime(1), ArrivalTime(1), ArrivalTime(3)];
        let facts = summarize_topic_timing(&times, Some(ArrivalTime(3)), ArrivalTime(4));
        assert_eq!(facts.sample_count, 3);
        assert!((facts.observed_rate_hz.unwrap() - 1_000_000_000.0).abs() < 0.001);
        assert_eq!(facts.latest_age, Some(Duration::from_nanos(1)));
        assert_eq!(facts.recent_gap, Some(Duration::from_nanos(2)));
        assert_eq!(facts.max_gap, facts.recent_gap);
        let zero = summarize_topic_timing(&times[..2], Some(ArrivalTime(1)), ArrivalTime(1));
        assert_eq!(zero.observed_rate_hz, None);
        assert_eq!(zero.max_gap, Some(Duration::ZERO));
        let single = summarize_topic_timing(&times[..1], Some(ArrivalTime(1)), ArrivalTime(1));
        assert_eq!(single.recent_gap, None);
    }
    #[test]
    fn elapsed_supports_full_signed_time_range_without_overflow() {
        let facts = summarize_topic_timing(&[], Some(ArrivalTime(i64::MIN)), ArrivalTime(i64::MAX));
        assert_eq!(facts.latest_age, Some(Duration::from_nanos(u64::MAX)));
    }
}
