//! Bounded serialized-message windows and fetch planning.

use crate::{ArrivalTime, PlaybackSpeed, RawMessage};
use std::{collections::VecDeque, error::Error, fmt, time::Duration};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeRange {
    pub start: ArrivalTime,
    pub end_exclusive: ArrivalTime,
}

impl TimeRange {
    pub fn new(start: ArrivalTime, end_exclusive: ArrivalTime) -> Result<Self, DataWindowError> {
        if start >= end_exclusive {
            return Err(DataWindowError::new(
                "data window start must precede endExclusive",
            ));
        }
        Ok(Self {
            start,
            end_exclusive,
        })
    }

    pub fn contains(self, time: ArrivalTime) -> bool {
        self.start <= time && time < self.end_exclusive
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SerializedWindow {
    pub range: TimeRange,
    pub messages: Vec<RawMessage>,
    /// Approximate total size of unique backing allocations pinned by this window.
    pub resident_bytes: usize,
    /// Sum of the message payload lengths, independent of shared backing size.
    pub logical_payload_bytes: usize,
}

impl SerializedWindow {
    pub fn new(
        range: TimeRange,
        messages: Vec<RawMessage>,
        resident_bytes: usize,
    ) -> Result<Self, DataWindowError> {
        let logical_payload_bytes = messages.iter().try_fold(0_usize, |total, message| {
            total.checked_add(message.payload.len())
        });
        let logical_payload_bytes = logical_payload_bytes
            .ok_or_else(|| DataWindowError::new("logical payload byte count overflow"))?;
        let window = Self {
            range,
            messages,
            resident_bytes,
            logical_payload_bytes,
        };
        window.validate()?;
        Ok(window)
    }

    pub fn validate(&self) -> Result<(), DataWindowError> {
        let mut previous = None;
        for message in &self.messages {
            if !self.range.contains(message.arrival_time) {
                return Err(DataWindowError::new(format!(
                    "message time {} is outside data window [{}, {})",
                    message.arrival_time.0, self.range.start.0, self.range.end_exclusive.0
                )));
            }
            let key = (message.arrival_time, message.stream_id.0);
            if previous.is_some_and(|previous| key < previous) {
                return Err(DataWindowError::new(
                    "data window messages must be ordered by time and stream",
                ));
            }
            previous = Some(key);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FetchIntent {
    /// Fetch only when the requested cursor is outside completed coverage.
    RequiredOnly,
    /// Continue filling the profile's speed-adjusted target-ahead reserve.
    PlaybackAhead,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FetchDemand {
    pub cursor: ArrivalTime,
    pub required_through: ArrivalTime,
    pub complete_until: ArrivalTime,
    pub end_exclusive: ArrivalTime,
    pub playback_speed: PlaybackSpeed,
    pub intent: FetchIntent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FetchProfile {
    window_size: Duration,
    realtime_target_ahead: Duration,
    max_resident_bytes: usize,
}

impl FetchProfile {
    pub fn new(
        window_size: Duration,
        realtime_target_ahead: Duration,
        max_resident_bytes: usize,
    ) -> Result<Self, DataWindowError> {
        if window_size.is_zero() {
            return Err(DataWindowError::new("fetch window size must be positive"));
        }
        if realtime_target_ahead.is_zero() {
            return Err(DataWindowError::new("fetch target ahead must be positive"));
        }
        if max_resident_bytes == 0 {
            return Err(DataWindowError::new(
                "fetch profile resident budget must be positive",
            ));
        }
        Ok(Self {
            window_size,
            realtime_target_ahead,
            max_resident_bytes,
        })
    }

    pub fn window_size(self) -> Duration {
        self.window_size
    }

    pub fn realtime_target_ahead(self) -> Duration {
        self.realtime_target_ahead
    }

    pub fn target_ahead(self, speed: PlaybackSpeed) -> Duration {
        scale_duration(self.realtime_target_ahead, speed)
    }

    pub fn max_resident_bytes(self) -> usize {
        self.max_resident_bytes
    }
}

impl Default for FetchProfile {
    fn default() -> Self {
        Self {
            window_size: Duration::from_secs(1),
            realtime_target_ahead: Duration::from_secs(2),
            max_resident_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FetchPlanner {
    profile: FetchProfile,
}

impl FetchPlanner {
    pub fn new(profile: FetchProfile) -> Self {
        Self { profile }
    }

    pub fn plan(self, demand: FetchDemand) -> Option<TimeRange> {
        if demand.complete_until >= demand.end_exclusive {
            return None;
        }
        let ahead_ns = demand
            .complete_until
            .0
            .saturating_sub(demand.cursor.0)
            .max(0);
        let target_ahead = self.profile.target_ahead(demand.playback_speed);
        let requested_is_complete = demand.required_through < demand.complete_until;
        if requested_is_complete
            && (demand.intent == FetchIntent::RequiredOnly || ahead_ns >= duration_ns(target_ahead))
        {
            return None;
        }
        let end_exclusive = ArrivalTime(
            demand
                .complete_until
                .0
                .saturating_add(duration_ns(self.profile.window_size))
                .min(demand.end_exclusive.0),
        );
        TimeRange::new(demand.complete_until, end_exclusive).ok()
    }

    pub fn profile(self) -> FetchProfile {
        self.profile
    }
}

#[derive(Debug)]
struct StoredWindow {
    window: SerializedWindow,
    next_message: usize,
}

#[derive(Debug)]
pub struct MemoryWindowStore {
    windows: VecDeque<StoredWindow>,
    resident_bytes: usize,
    logical_payload_bytes: usize,
    max_bytes: usize,
    complete_until: ArrivalTime,
    eviction_count: u64,
}

impl MemoryWindowStore {
    pub fn new(start: ArrivalTime, max_bytes: usize) -> Result<Self, DataWindowError> {
        if max_bytes == 0 {
            return Err(DataWindowError::new(
                "memory window store budget must be positive",
            ));
        }
        Ok(Self {
            windows: VecDeque::new(),
            resident_bytes: 0,
            logical_payload_bytes: 0,
            max_bytes,
            complete_until: start,
            eviction_count: 0,
        })
    }

    pub fn insert(&mut self, window: SerializedWindow) -> Result<(), DataWindowError> {
        window.validate()?;
        if window.range.start != self.complete_until {
            return Err(DataWindowError::new(format!(
                "data window starts at {}, expected {}",
                window.range.start.0, self.complete_until.0
            )));
        }
        let resident_bytes = self
            .resident_bytes
            .checked_add(window.resident_bytes)
            .ok_or_else(|| DataWindowError::new("resident byte count overflow"))?;
        let logical_payload_bytes = self
            .logical_payload_bytes
            .checked_add(window.logical_payload_bytes)
            .ok_or_else(|| DataWindowError::new("logical payload byte count overflow"))?;
        self.resident_bytes = resident_bytes;
        self.logical_payload_bytes = logical_payload_bytes;
        self.complete_until = window.range.end_exclusive;
        self.windows.push_back(StoredWindow {
            window,
            next_message: 0,
        });
        Ok(())
    }

    /// Drops retained coverage and starts a new contiguous range at `start`.
    /// Capacity-eviction metrics are preserved because a seek reset is not an eviction.
    pub fn reset(&mut self, start: ArrivalTime) {
        self.windows.clear();
        self.resident_bytes = 0;
        self.logical_payload_bytes = 0;
        self.complete_until = start;
    }

    pub fn contains(&self, time: ArrivalTime) -> bool {
        self.windows
            .iter()
            .any(|stored| stored.window.range.contains(time))
    }

    pub fn complete_until(&self) -> ArrivalTime {
        self.complete_until
    }

    pub fn is_complete_through(
        &self,
        target: ArrivalTime,
        recording_end_exclusive: ArrivalTime,
    ) -> bool {
        target < self.complete_until
            || (target == ArrivalTime(recording_end_exclusive.0.saturating_sub(1))
                && self.complete_until == recording_end_exclusive)
    }

    pub fn messages_through(&self, after: ArrivalTime, through: ArrivalTime) -> Vec<RawMessage> {
        self.windows
            .iter()
            .flat_map(|stored| stored.window.messages.iter())
            .filter(|message| message.arrival_time > after && message.arrival_time <= through)
            .cloned()
            .collect()
    }

    /// Returns each retained message at most once while keeping its Bytes allocation resident.
    pub fn take_messages_through(
        &mut self,
        after: ArrivalTime,
        through: ArrivalTime,
        include_after: bool,
    ) -> Vec<RawMessage> {
        let mut result = Vec::new();
        for stored in &mut self.windows {
            let additional = stored.window.messages[stored.next_message..]
                .partition_point(|message| message.arrival_time <= through);
            let end = stored.next_message + additional;
            result.extend(
                stored.window.messages[stored.next_message..end]
                    .iter()
                    .filter(|message| {
                        message.arrival_time > after
                            || (include_after && message.arrival_time == after)
                    })
                    .cloned(),
            );
            stored.next_message = end;
        }
        result
    }

    pub fn evict_before(&mut self, time: ArrivalTime) {
        while self
            .windows
            .front()
            .is_some_and(|stored| stored.window.range.end_exclusive <= time)
        {
            self.evict_front();
        }
    }

    /// Enforces the budget without removing the cursor window or its immediate successor.
    pub fn enforce_budget(&mut self, cursor: ArrivalTime) {
        while self.resident_bytes > self.max_bytes {
            let protected = self
                .windows
                .iter()
                .position(|stored| stored.window.range.contains(cursor));
            if protected == Some(0) {
                break;
            }
            if self.windows.is_empty() {
                break;
            }
            self.evict_front();
        }
    }

    pub fn resident_bytes(&self) -> usize {
        self.resident_bytes
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn logical_payload_bytes(&self) -> usize {
        self.logical_payload_bytes
    }

    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    pub fn eviction_count(&self) -> u64 {
        self.eviction_count
    }

    fn evict_front(&mut self) {
        if let Some(stored) = self.windows.pop_front() {
            self.resident_bytes = self
                .resident_bytes
                .saturating_sub(stored.window.resident_bytes);
            self.logical_payload_bytes = self
                .logical_payload_bytes
                .saturating_sub(stored.window.logical_payload_bytes);
            self.eviction_count = self.eviction_count.saturating_add(1);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataWindowError(String);

impl DataWindowError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for DataWindowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for DataWindowError {}

fn duration_ns(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX)
}

fn scale_duration(duration: Duration, speed: PlaybackSpeed) -> Duration {
    let nanos = duration.as_nanos();
    let scaled = match speed {
        PlaybackSpeed::Quarter => nanos / 4,
        PlaybackSpeed::Half => nanos / 2,
        PlaybackSpeed::Normal => nanos,
        PlaybackSpeed::Double => nanos.saturating_mul(2),
    }
    .max(1);
    Duration::from_nanos(u64::try_from(scaled).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StreamId;
    use bytes::Bytes;

    fn message(time: i64, bytes: Bytes) -> RawMessage {
        RawMessage {
            stream_id: StreamId(1),
            arrival_time: ArrivalTime(time),
            payload: bytes,
        }
    }

    fn window(start: i64, end: i64, resident_bytes: usize) -> SerializedWindow {
        SerializedWindow::new(
            TimeRange::new(ArrivalTime(start), ArrivalTime(end)).unwrap(),
            vec![],
            resident_bytes,
        )
        .unwrap()
    }

    #[test]
    fn planner_uses_adjacent_exclusive_windows_and_target_ahead() {
        let planner = FetchPlanner::new(FetchProfile::default());
        let first = planner
            .plan(FetchDemand {
                cursor: ArrivalTime(0),
                required_through: ArrivalTime(0),
                complete_until: ArrivalTime(0),
                end_exclusive: ArrivalTime(2_500_000_000),
                playback_speed: PlaybackSpeed::Normal,
                intent: FetchIntent::PlaybackAhead,
            })
            .unwrap();
        let second = planner
            .plan(FetchDemand {
                cursor: ArrivalTime(0),
                required_through: ArrivalTime(0),
                complete_until: first.end_exclusive,
                end_exclusive: ArrivalTime(2_500_000_000),
                playback_speed: PlaybackSpeed::Normal,
                intent: FetchIntent::PlaybackAhead,
            })
            .unwrap();
        assert_eq!(first.end_exclusive, second.start);
        assert!(
            planner
                .plan(FetchDemand {
                    cursor: ArrivalTime(0),
                    required_through: ArrivalTime(0),
                    complete_until: second.end_exclusive,
                    end_exclusive: ArrivalTime(2_500_000_000),
                    playback_speed: PlaybackSpeed::Normal,
                    intent: FetchIntent::PlaybackAhead,
                })
                .is_none()
        );
    }

    #[test]
    fn default_profile_scales_log_time_ahead_to_playback_speed() {
        let profile = FetchProfile::default();
        assert_eq!(profile.window_size(), Duration::from_secs(1));
        assert_eq!(profile.realtime_target_ahead(), Duration::from_secs(2));
        assert_eq!(profile.max_resident_bytes(), 256 * 1024 * 1024);
        assert_eq!(
            profile.target_ahead(PlaybackSpeed::Quarter),
            Duration::from_millis(500)
        );
        assert_eq!(
            profile.target_ahead(PlaybackSpeed::Half),
            Duration::from_secs(1)
        );
        assert_eq!(
            profile.target_ahead(PlaybackSpeed::Normal),
            Duration::from_secs(2)
        );
        assert_eq!(
            profile.target_ahead(PlaybackSpeed::Double),
            Duration::from_secs(4)
        );
    }

    #[test]
    fn double_speed_fetches_when_normal_speed_has_enough_log_time_ahead() {
        let planner = FetchPlanner::new(FetchProfile::default());
        let demand = |playback_speed| FetchDemand {
            cursor: ArrivalTime(0),
            required_through: ArrivalTime(0),
            complete_until: ArrivalTime(3_000_000_000),
            end_exclusive: ArrivalTime(10_000_000_000),
            playback_speed,
            intent: FetchIntent::PlaybackAhead,
        };

        assert!(planner.plan(demand(PlaybackSpeed::Normal)).is_none());
        assert!(planner.plan(demand(PlaybackSpeed::Double)).is_some());
    }

    #[test]
    fn required_cursor_at_completeness_boundary_forces_the_next_window() {
        let planner = FetchPlanner::new(FetchProfile::default());
        assert!(
            planner
                .plan(FetchDemand {
                    cursor: ArrivalTime(0),
                    required_through: ArrivalTime(2_000_000_000),
                    complete_until: ArrivalTime(2_000_000_000),
                    end_exclusive: ArrivalTime(4_000_000_000),
                    playback_speed: PlaybackSpeed::Normal,
                    intent: FetchIntent::PlaybackAhead,
                })
                .is_some()
        );
    }

    #[test]
    fn required_only_stops_when_the_requested_cursor_is_complete() {
        let planner = FetchPlanner::new(FetchProfile::default());
        let demand = FetchDemand {
            cursor: ArrivalTime(0),
            required_through: ArrivalTime(0),
            complete_until: ArrivalTime(1_000_000_000),
            end_exclusive: ArrivalTime(10_000_000_000),
            playback_speed: PlaybackSpeed::Double,
            intent: FetchIntent::RequiredOnly,
        };

        assert!(planner.plan(demand).is_none());
        assert!(
            planner
                .plan(FetchDemand {
                    required_through: demand.complete_until,
                    ..demand
                })
                .is_some(),
            "required coverage still wins over disabled prefetch"
        );
    }

    #[test]
    fn fetch_profile_rejects_zero_policy_values() {
        assert!(FetchProfile::new(Duration::ZERO, Duration::from_secs(2), 1).is_err());
        assert!(FetchProfile::new(Duration::from_secs(1), Duration::ZERO, 1).is_err());
        assert!(FetchProfile::new(Duration::from_secs(1), Duration::from_secs(2), 0).is_err());
    }

    #[test]
    fn empty_window_advances_exclusive_completeness() {
        let mut store = MemoryWindowStore::new(ArrivalTime(10), 1024).unwrap();
        store.insert(window(10, 20, 0)).unwrap();
        assert_eq!(store.complete_until(), ArrivalTime(20));
        assert!(store.is_complete_through(ArrivalTime(19), ArrivalTime(30)));
        assert!(!store.is_complete_through(ArrivalTime(20), ArrivalTime(30)));
    }

    #[test]
    fn reset_rebases_coverage_without_counting_a_capacity_eviction() {
        let mut store = MemoryWindowStore::new(ArrivalTime(0), 1024).unwrap();
        store.insert(window(0, 10, 7)).unwrap();

        store.reset(ArrivalTime(100));

        assert_eq!(store.complete_until(), ArrivalTime(100));
        assert_eq!(store.window_count(), 0);
        assert_eq!(store.resident_bytes(), 0);
        assert_eq!(store.logical_payload_bytes(), 0);
        assert_eq!(store.eviction_count(), 0);
    }

    #[test]
    fn cloned_messages_retain_the_same_payload_allocation() {
        let backing = Bytes::from_static(b"header-payload-tail");
        let payload = backing.slice(7..14);
        let pointer = payload.as_ptr();
        let serialized = SerializedWindow::new(
            TimeRange::new(ArrivalTime(10), ArrivalTime(20)).unwrap(),
            vec![message(12, payload)],
            backing.len(),
        )
        .unwrap();
        let mut store = MemoryWindowStore::new(ArrivalTime(10), 1024).unwrap();
        store.insert(serialized).unwrap();

        let loaded = store.take_messages_through(ArrivalTime(10), ArrivalTime(19), true);
        assert_eq!(loaded[0].payload.as_ptr(), pointer);
        assert!(
            store
                .take_messages_through(ArrivalTime(10), ArrivalTime(19), false)
                .is_empty()
        );
        assert_eq!(store.logical_payload_bytes(), 7);
    }

    #[test]
    fn window_distinguishes_logical_payload_from_resident_backing() {
        let backing = Bytes::from(vec![0_u8; 128]);
        let serialized = SerializedWindow::new(
            TimeRange::new(ArrivalTime(10), ArrivalTime(20)).unwrap(),
            vec![
                message(11, backing.slice(8..16)),
                message(12, backing.slice(32..40)),
            ],
            backing.len(),
        )
        .unwrap();

        assert_eq!(serialized.logical_payload_bytes, 16);
        assert_eq!(serialized.resident_bytes, 128);
    }

    #[test]
    fn empty_window_has_no_logical_or_resident_bytes() {
        let serialized = window(10, 20, 0);
        assert_eq!(serialized.logical_payload_bytes, 0);
        assert_eq!(serialized.resident_bytes, 0);
    }

    #[test]
    fn adjacent_windows_deliver_the_boundary_message_once() {
        let mut store = MemoryWindowStore::new(ArrivalTime(0), 1024).unwrap();
        store
            .insert(
                SerializedWindow::new(
                    TimeRange::new(ArrivalTime(0), ArrivalTime(10)).unwrap(),
                    vec![message(9, Bytes::from_static(b"before"))],
                    6,
                )
                .unwrap(),
            )
            .unwrap();
        store
            .insert(
                SerializedWindow::new(
                    TimeRange::new(ArrivalTime(10), ArrivalTime(20)).unwrap(),
                    vec![message(10, Bytes::from_static(b"boundary"))],
                    8,
                )
                .unwrap(),
            )
            .unwrap();

        assert_eq!(
            store
                .take_messages_through(ArrivalTime(0), ArrivalTime(9), true)
                .len(),
            1
        );
        let second = store.take_messages_through(ArrivalTime(9), ArrivalTime(10), false);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].arrival_time, ArrivalTime(10));
        assert!(
            store
                .take_messages_through(ArrivalTime(9), ArrivalTime(10), false)
                .is_empty()
        );
    }

    #[test]
    fn budget_evicts_oldest_but_protects_current_and_next_windows() {
        let mut store = MemoryWindowStore::new(ArrivalTime(0), 8).unwrap();
        store.insert(window(0, 10, 4)).unwrap();
        store.insert(window(10, 20, 4)).unwrap();
        store.insert(window(20, 30, 4)).unwrap();

        store.enforce_budget(ArrivalTime(15));
        assert_eq!(store.window_count(), 2);
        assert_eq!(store.resident_bytes(), 8);
        assert_eq!(store.eviction_count(), 1);
        assert!(store.contains(ArrivalTime(15)));
        assert!(store.contains(ArrivalTime(25)));

        let mut protected = MemoryWindowStore::new(ArrivalTime(0), 1).unwrap();
        protected.insert(window(0, 10, 4)).unwrap();
        protected.insert(window(10, 20, 4)).unwrap();
        protected.enforce_budget(ArrivalTime(5));
        assert_eq!(protected.window_count(), 2);
        assert_eq!(protected.eviction_count(), 0);
    }

    #[test]
    fn rejects_non_adjacent_or_out_of_range_windows() {
        let mut store = MemoryWindowStore::new(ArrivalTime(0), 1024).unwrap();
        assert!(store.insert(window(1, 2, 0)).is_err());
        assert!(
            SerializedWindow::new(
                TimeRange::new(ArrivalTime(0), ArrivalTime(10)).unwrap(),
                vec![message(10, Bytes::new())],
                0,
            )
            .is_err()
        );
    }
}
