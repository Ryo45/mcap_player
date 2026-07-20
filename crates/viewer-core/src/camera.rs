use crate::{ArrivalTime, CameraId, MeasurementTime};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraFrame {
    pub camera_id: CameraId,
    pub measurement_time: MeasurementTime,
    pub arrival_time: ArrivalTime,
    pub frame_id: String,
    pub jpeg: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraStatus {
    WaitingForCameraFrame,
    Ready,
    Error,
}

#[derive(Clone, Debug)]
pub struct CameraState {
    latest: Option<CameraFrame>,
    status: CameraStatus,
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            latest: None,
            status: CameraStatus::WaitingForCameraFrame,
        }
    }
}

impl CameraState {
    pub fn apply(&mut self, frame: CameraFrame) -> bool {
        if self
            .latest
            .as_ref()
            .is_some_and(|current| current.arrival_time > frame.arrival_time)
        {
            return false;
        }
        self.latest = Some(frame);
        self.status = CameraStatus::Ready;
        true
    }

    pub fn cold_seek(&mut self) {
        self.latest = None;
        self.status = CameraStatus::WaitingForCameraFrame;
    }
    pub fn latest(&self) -> Option<&CameraFrame> {
        self.latest.as_ref()
    }
    pub fn status(&self) -> CameraStatus {
        self.status
    }
    pub fn set_error(&mut self) {
        self.status = CameraStatus::Error;
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
            jpeg: vec![],
        }
    }

    #[test]
    fn rejects_old_arrival_and_clears_on_seek() {
        let mut state = CameraState::default();
        assert!(state.apply(frame(2)));
        assert!(!state.apply(frame(1)));
        state.cold_seek();
        assert!(state.latest().is_none());
        assert!(state.apply(frame(3)));
    }
}
