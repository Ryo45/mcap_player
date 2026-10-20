//! Decoded point-cloud state.

use crate::{ArrivalTime, MeasurementTime};

#[derive(Clone, Debug, PartialEq)]
pub struct PointCloudFrame {
    pub measurement_time: MeasurementTime,
    pub arrival_time: ArrivalTime,
    pub frame_id: String,
    pub attributes: PointCloudAttributes,
    /// ROS `frame_id` metres: +x forward, +y left, +z up.
    pub points: Vec<[f32; 3]>,
}

/// Source attributes, aligned with finite XYZ positions. Colors are byte values, not a colormap.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PointCloudAttributes {
    pub rgba: Option<Vec<[u8; 4]>>,
    pub intensity: Option<Vec<f32>>,
}

impl PointCloudAttributes {
    pub fn matches_points(&self, count: usize) -> bool {
        self.rgba.as_ref().is_none_or(|v| v.len() == count)
            && self.intensity.as_ref().is_none_or(|v| v.len() == count)
    }
}

#[derive(Clone, Debug, Default)]
pub struct PointCloudState {
    revision: u64,
    latest: Option<PointCloudFrame>,
}

impl PointCloudState {
    pub fn apply(&mut self, frame: PointCloudFrame) -> bool {
        if !frame.attributes.matches_points(frame.points.len()) {
            return false;
        }
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
            attributes: PointCloudAttributes::default(),
            points: vec![[2.0, 1.0, 0.0]],
        }
    }

    #[test]
    fn rejects_misaligned_attributes_without_mutating_state() {
        let mut state = PointCloudState::default();
        assert!(state.apply(frame(1)));
        let revision = state.revision();
        let mut malformed = frame(2);
        malformed.attributes.rgba = Some(vec![]);
        assert!(!state.apply(malformed));
        assert_eq!(state.latest().unwrap().arrival_time, ArrivalTime(1));
        assert_eq!(state.revision(), revision);
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
