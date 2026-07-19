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
    generation: u64,
    latest: Option<CameraFrame>,
    status: CameraStatus,
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            generation: 0,
            latest: None,
            status: CameraStatus::WaitingForCameraFrame,
        }
    }
}

impl CameraState {
    pub fn apply(&mut self, generation: u64, frame: CameraFrame) -> bool {
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
        self.status = CameraStatus::Ready;
        true
    }

    pub fn cold_seek(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.latest = None;
        self.status = CameraStatus::WaitingForCameraFrame;
        self.generation
    }

    pub fn generation(&self) -> u64 {
        self.generation
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
    fn rejects_old_arrival_and_seek_generation() {
        let mut state = CameraState::default();
        assert!(state.apply(0, frame(2)));
        assert!(!state.apply(0, frame(1)));
        let generation = state.cold_seek();
        assert!(!state.apply(generation - 1, frame(3)));
        assert!(state.apply(generation, frame(3)));
    }
}
