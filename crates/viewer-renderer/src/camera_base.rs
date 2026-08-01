use std::collections::BTreeMap;
use viewer_core::{ArrivalTime, CameraFrame, CameraId};

#[derive(Clone, Debug, Default)]
pub struct CameraBaseImageTracker {
    arrivals: BTreeMap<CameraId, ArrivalTime>,
}

impl CameraBaseImageTracker {
    pub fn needs_update(&self, frame: &CameraFrame) -> bool {
        self.arrivals.get(&frame.camera_id) != Some(&frame.arrival_time)
    }

    pub fn mark_updated(&mut self, frame: &CameraFrame) {
        self.arrivals.insert(frame.camera_id, frame.arrival_time);
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
    use viewer_core::MeasurementTime;

    fn frame(camera_id: u16, arrival: i64) -> CameraFrame {
        CameraFrame {
            camera_id: CameraId(camera_id),
            measurement_time: MeasurementTime(arrival),
            arrival_time: ArrivalTime(arrival),
            frame_id: "camera".to_owned(),
            jpeg: Vec::new(),
        }
    }

    #[test]
    fn tracks_successful_base_updates_by_camera_and_arrival() {
        let first = frame(0, 10);
        let next = frame(0, 11);
        let other = frame(1, 10);
        let mut tracker = CameraBaseImageTracker::default();
        assert!(tracker.needs_update(&first));
        tracker.mark_updated(&first);
        assert!(!tracker.needs_update(&first));
        assert!(tracker.needs_update(&next));
        assert!(tracker.needs_update(&other));
        assert_eq!(tracker.arrival(CameraId(0)), Some(ArrivalTime(10)));
        tracker.clear();
        assert!(tracker.needs_update(&first));
    }
}
