use crate::{ArrivalTime, MeasurementTime};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Identifies a camera within one Viewer Domain/session.
///
/// This is not a physical camera serial number or a persistent identity across sessions.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct CameraId(pub u16);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraFrame {
    pub camera_id: CameraId,
    pub measurement_time: MeasurementTime,
    pub arrival_time: ArrivalTime,
    pub frame_id: String,
    pub jpeg: Bytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraStatus {
    WaitingForCameraFrame,
    Ready,
    Error,
}

#[derive(Clone, Debug, Default)]
pub struct CameraState {
    latest: BTreeMap<CameraId, CameraFrame>,
    status: BTreeMap<CameraId, CameraStatus>,
}

impl CameraState {
    pub fn apply(&mut self, frame: CameraFrame) -> bool {
        if self
            .latest
            .get(&frame.camera_id)
            .is_some_and(|current| current.arrival_time > frame.arrival_time)
        {
            return false;
        }
        let camera_id = frame.camera_id;
        self.latest.insert(camera_id, frame);
        self.status.insert(camera_id, CameraStatus::Ready);
        true
    }

    pub fn cold_seek(&mut self) {
        self.latest.clear();
        for status in self.status.values_mut() {
            *status = CameraStatus::WaitingForCameraFrame;
        }
    }

    pub fn latest_for(&self, camera_id: CameraId) -> Option<&CameraFrame> {
        self.latest.get(&camera_id)
    }

    pub fn latest_by_arrival(&self) -> Option<&CameraFrame> {
        self.latest
            .iter()
            .max_by_key(|(camera_id, frame)| (frame.arrival_time, **camera_id))
            .map(|(_, frame)| frame)
    }

    pub fn status_for(&self, camera_id: CameraId) -> CameraStatus {
        self.status
            .get(&camera_id)
            .copied()
            .unwrap_or(CameraStatus::WaitingForCameraFrame)
    }

    pub fn set_error_for(&mut self, camera_id: CameraId) {
        self.status.insert(camera_id, CameraStatus::Error);
    }
    pub fn ids(&self) -> impl Iterator<Item = CameraId> + '_ {
        self.status.keys().copied()
    }
    pub fn frames(&self) -> impl Iterator<Item = (&CameraId, &CameraFrame)> {
        self.latest.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(arrival: i64) -> CameraFrame {
        CameraFrame {
            camera_id: CameraId(0),
            measurement_time: MeasurementTime(0),
            arrival_time: ArrivalTime(arrival),
            frame_id: String::new(),
            jpeg: Bytes::new(),
        }
    }

    #[test]
    fn rejects_old_arrival_and_clears_on_seek() {
        let mut state = CameraState::default();
        assert!(state.apply(frame(2)));
        assert!(!state.apply(frame(1)));
        state.cold_seek();
        assert!(state.latest_for(CameraId(0)).is_none());
        assert!(state.apply(frame(3)));
    }

    #[test]
    fn keeps_frames_independent_by_camera_id() {
        let mut state = CameraState::default();
        let mut rear = frame(1);
        rear.camera_id = CameraId(1);
        assert!(state.apply(frame(2)));
        assert!(state.apply(rear));
        assert!(state.latest_for(CameraId(0)).is_some());
        assert!(state.latest_for(CameraId(1)).is_some());
        assert_eq!(
            state.latest_by_arrival().map(|frame| frame.camera_id),
            Some(CameraId(0))
        );
        assert_eq!(
            state.ids().collect::<Vec<_>>(),
            vec![CameraId(0), CameraId(1)]
        );
    }

    #[test]
    fn errors_and_status_are_scoped_to_one_camera() {
        let mut state = CameraState::default();
        let mut rear = frame(3);
        rear.camera_id = CameraId(1);
        state.apply(frame(2));
        state.apply(rear);

        assert_eq!(
            state.latest_by_arrival().map(|frame| frame.camera_id),
            Some(CameraId(1))
        );
        state.set_error_for(CameraId(1));
        assert_eq!(state.status_for(CameraId(0)), CameraStatus::Ready);
        assert_eq!(state.status_for(CameraId(1)), CameraStatus::Error);
        assert_eq!(
            state.status_for(CameraId(2)),
            CameraStatus::WaitingForCameraFrame
        );
    }
}
