//! Decoded path state used by BEV features.

use crate::{ArrivalTime, MeasurementTime};

#[derive(Clone, Debug, PartialEq)]
pub struct BevPathFrame {
    pub measurement_time: MeasurementTime,
    pub arrival_time: ArrivalTime,
    pub frame_id: String,
    /// Ego-relative path in metres: +x right, +y forward.
    pub points: Vec<[f32; 2]>,
}

#[derive(Clone, Debug, Default)]
pub struct BevState {
    revision: u64,
    latest: Option<BevPathFrame>,
}

impl BevState {
    pub fn apply(&mut self, frame: BevPathFrame) -> bool {
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

    pub fn latest(&self) -> Option<&BevPathFrame> {
        self.latest.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(arrival: i64) -> BevPathFrame {
        BevPathFrame {
            measurement_time: MeasurementTime(arrival - 1),
            arrival_time: ArrivalTime(arrival),
            frame_id: "base_link".into(),
            points: vec![[1.0, 2.0]],
        }
    }

    #[test]
    fn rejects_old_path_and_clears_on_seek() {
        let mut state = BevState::default();
        assert!(state.apply(frame(2)));
        assert!(!state.apply(frame(1)));
        let revision = state.revision();
        state.cold_seek();
        assert!(state.latest().is_none());
        assert_ne!(state.revision(), revision);
        assert!(state.apply(frame(3)));
    }
}
