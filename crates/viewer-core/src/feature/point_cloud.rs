//! Decoded point-cloud state.

use crate::{ArrivalTime, MeasurementTime};

#[derive(Clone, Debug, PartialEq)]
pub struct PointCloudFrame {
    pub measurement_time: MeasurementTime,
    pub arrival_time: ArrivalTime,
    pub frame_id: String,
    /// ROS `frame_id` metres: +x forward, +y left, +z up.
    pub points: Vec<[f32; 3]>,
}

#[derive(Clone, Debug, Default)]
pub struct PointCloudState {
    revision: u64,
    latest: Option<PointCloudFrame>,
}

impl PointCloudState {
    pub fn apply(&mut self, frame: PointCloudFrame) -> bool {
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

    pub fn latest(&self) -> Option<&PointCloudFrame> {
        self.latest.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(arrival: i64) -> PointCloudFrame {
        PointCloudFrame {
            measurement_time: MeasurementTime(arrival - 1),
            arrival_time: ArrivalTime(arrival),
            frame_id: "base_scan".into(),
            points: vec![[2.0, 1.0, 0.0]],
        }
    }

    #[test]
    fn keeps_latest_scan_and_clears_on_seek() {
        let mut state = PointCloudState::default();
        assert!(state.apply(frame(2)));
        assert!(!state.apply(frame(1)));
        let revision = state.revision();
        state.cold_seek();
        assert!(state.latest().is_none());
        assert_ne!(state.revision(), revision);
        assert!(state.apply(frame(3)));
    }
}
