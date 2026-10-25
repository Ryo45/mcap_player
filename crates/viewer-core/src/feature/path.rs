//! Canonical path positions in the source ROS frame.

use crate::{ArrivalTime, MeasurementTime};

#[derive(Clone, Debug, PartialEq)]
pub struct PathFrame {
    pub measurement_time: MeasurementTime,
    pub arrival_time: ArrivalTime,
    pub frame_id: String,
    /// Metres in frame_id; no BEV conversion or ground projection.
    pub points: Vec<[f64; 3]>,
}

#[derive(Clone, Debug, Default)]
pub struct PathState {
    revision: u64,
    latest: Option<PathFrame>,
}

impl PathState {
    pub(crate) fn restore_candidate(&self) -> Self {
        let mut candidate = Self {
            revision: self.revision,
            ..Self::default()
        };
        candidate.cold_seek();
        candidate
    }

    pub fn apply(&mut self, frame: PathFrame) -> bool {
        if self
            .latest
            .as_ref()
            .is_some_and(|current| current.arrival_time > frame.arrival_time)
        {
            return false;
        }
        self.latest = Some(frame);
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub fn cold_seek(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.latest = None;
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn latest(&self) -> Option<&PathFrame> {
        self.latest.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(arrival: i64) -> PathFrame {
        PathFrame {
            measurement_time: MeasurementTime(arrival - 1),
            arrival_time: ArrivalTime(arrival),
            frame_id: "base_link".into(),
            points: vec![[2.0, -(1.0), 0.0]],
        }
    }

    #[test]
    fn restore_candidate_preserves_revision_wrap_without_mutating_current_state() {
        let state = PathState {
            revision: u64::MAX,
            ..PathState::default()
        };
        let candidate = state.restore_candidate();
        assert_eq!(candidate.revision(), 0);
        assert_eq!(state.revision(), u64::MAX);
    }

    #[test]
    fn rejects_old_path_and_clears_on_seek() {
        let mut state = PathState::default();
        assert!(state.apply(frame(2)));
        assert!(!state.apply(frame(1)));
        let revision = state.revision();
        state.cold_seek();
        assert!(state.latest().is_none());
        assert_ne!(state.revision(), revision);
        assert!(state.apply(frame(3)));
    }
}
