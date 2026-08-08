use crate::{ArrivalTime, RawMessage};
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
    /// Approximate retained allocation size, including batch framing when known.
    pub resident_bytes: usize,
}

impl SerializedWindow {
    pub fn new(
        range: TimeRange,
        messages: Vec<RawMessage>,
        resident_bytes: usize,
    ) -> Result<Self, DataWindowError> {
        let window = Self {
            range,
            messages,
            resident_bytes,
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
pub struct FetchDemand {
    pub cursor: ArrivalTime,
    pub complete_until: ArrivalTime,
    pub end_exclusive: ArrivalTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FetchPlanner {
    window_size: Duration,
    target_ahead: Duration,
}

impl FetchPlanner {
    pub fn new(window_size: Duration, target_ahead: Duration) -> Result<Self, DataWindowError> {
        if window_size.is_zero() {
            return Err(DataWindowError::new("fetch window size must be positive"));
        }
        if target_ahead.is_zero() {
            return Err(DataWindowError::new("fetch target ahead must be positive"));
        }
        Ok(Self {
            window_size,
            target_ahead,
        })
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
        if ahead_ns >= duration_ns(self.target_ahead) {
            return None;
        }
        let end_exclusive = ArrivalTime(
            demand
                .complete_until
                .0
                .saturating_add(duration_ns(self.window_size))
                .min(demand.end_exclusive.0),
        );
        TimeRange::new(demand.complete_until, end_exclusive).ok()
    }

    pub fn window_size(self) -> Duration {
        self.window_size
    }

    pub fn target_ahead(self) -> Duration {
        self.target_ahead
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
        self.resident_bytes = self
            .resident_bytes
            .checked_add(window.resident_bytes)
            .ok_or_else(|| DataWindowError::new("resident byte count overflow"))?;
        self.complete_until = window.range.end_exclusive;
        self.windows.push_back(StoredWindow {
            window,
            next_message: 0,
        });
        Ok(())
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
    pub fn take_messages_through(&mut self, through: ArrivalTime) -> Vec<RawMessage> {
        let mut result = Vec::new();
        for stored in &mut self.windows {
            let additional = stored.window.messages[stored.next_message..]
                .partition_point(|message| message.arrival_time <= through);
            let end = stored.next_message + additional;
            result.extend_from_slice(&stored.window.messages[stored.next_message..end]);
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
        let planner = FetchPlanner::new(Duration::from_secs(1), Duration::from_secs(2)).unwrap();
        let first = planner
            .plan(FetchDemand {
                cursor: ArrivalTime(0),
                complete_until: ArrivalTime(0),
                end_exclusive: ArrivalTime(2_500_000_000),
            })
            .unwrap();
        let second = planner
            .plan(FetchDemand {
                cursor: ArrivalTime(0),
                complete_until: first.end_exclusive,
                end_exclusive: ArrivalTime(2_500_000_000),
            })
            .unwrap();
        assert_eq!(first.end_exclusive, second.start);
        assert!(
            planner
                .plan(FetchDemand {
                    cursor: ArrivalTime(0),
                    complete_until: second.end_exclusive,
                    end_exclusive: ArrivalTime(2_500_000_000),
                })
                .is_none()
        );
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

        let loaded = store.take_messages_through(ArrivalTime(19));
        assert_eq!(loaded[0].payload.as_ptr(), pointer);
        assert!(store.take_messages_through(ArrivalTime(19)).is_empty());
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
