use crate::{CameraController, CameraId, StageTiming};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaybackPerformance {
    pub source_read: StageTiming,
    pub message_processing: StageTiming,
    pub camera_input_frames: u64,
    pub camera_presented_frames: u64,
    pub camera_presented_by_id: BTreeMap<CameraId, u64>,
}

impl PlaybackPerformance {
    pub fn from_controllers(
        source_read: StageTiming,
        processing: StageTiming,
        cameras: &CameraController,
    ) -> Self {
        Self {
            source_read,
            message_processing: processing,
            camera_input_frames: cameras.input_frames(),
            camera_presented_frames: cameras.presented_frames(),
            camera_presented_by_id: cameras.presented_by_id().clone(),
        }
    }

    pub fn focused_camera_hz(&self) -> f64 {
        CameraController::focused_hz()
    }

    pub fn background_camera_hz(&self) -> f64 {
        CameraController::background_hz()
    }
}
