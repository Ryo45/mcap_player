use std::collections::BTreeMap;
use viewer_core::{ArrivalTime, CameraId};

#[derive(Clone, Debug, Default)]
pub struct CameraBaseImageTracker {
    arrivals: BTreeMap<CameraId, ArrivalTime>,
}

impl CameraBaseImageTracker {
    pub fn needs_update(&self, camera_id: CameraId, arrival: ArrivalTime) -> bool {
        self.arrivals.get(&camera_id) != Some(&arrival)
    }

    pub fn mark_updated(&mut self, camera_id: CameraId, arrival: ArrivalTime) {
        self.arrivals.insert(camera_id, arrival);
    }

    pub fn arrival(&self, camera_id: CameraId) -> Option<ArrivalTime> {
        self.arrivals.get(&camera_id).copied()
    }

    pub fn clear(&mut self) {
        self.arrivals.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{CameraFrameMetadata, MeasurementTime};

    fn frame(camera_id: u16, arrival: i64) -> CameraFrameMetadata {
        CameraFrameMetadata {
            camera_id: CameraId(camera_id),
            measurement_time: MeasurementTime(arrival),
            arrival_time: ArrivalTime(arrival),
            frame_id: "camera".to_owned(),
        }
    }

    #[test]
    fn tracks_successful_base_updates_by_camera_and_arrival() {
        let first = frame(0, 10);
        let next = frame(0, 11);
        let other = frame(1, 10);
        let mut tracker = CameraBaseImageTracker::default();
        assert!(tracker.needs_update(first.camera_id, first.arrival_time));
        tracker.mark_updated(first.camera_id, first.arrival_time);
        assert!(!tracker.needs_update(first.camera_id, first.arrival_time));
        assert!(tracker.needs_update(next.camera_id, next.arrival_time));
        assert!(tracker.needs_update(other.camera_id, other.arrival_time));
        assert_eq!(tracker.arrival(CameraId(0)), Some(ArrivalTime(10)));
        tracker.clear();
        assert!(tracker.needs_update(first.camera_id, first.arrival_time));
    }
}
