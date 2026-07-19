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
    generation: u64,
    revision: u64,
    latest: Option<PointCloudFrame>,
}

impl PointCloudState {
    pub fn apply(&mut self, generation: u64, frame: PointCloudFrame) -> bool {
        if generation != self.generation {
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

    pub fn cold_seek(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
        self.latest = None;
        self.generation
    }

    pub fn generation(&self) -> u64 {
        self.generation
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
        assert!(state.apply(0, frame(2)));
        assert!(!state.apply(0, frame(1)));
        let revision = state.revision();
        let generation = state.cold_seek();
        assert!(state.latest().is_none());
        assert_ne!(state.revision(), revision);
        assert!(!state.apply(generation - 1, frame(3)));
    }
}
