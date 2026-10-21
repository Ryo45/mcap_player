//! Camera frame identity and current semantic state.

use crate::{ArrivalTime, MeasurementTime};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Identifies a camera within one Viewer session.
///
/// This is not a physical camera serial number or a persistent identity across sessions.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct CameraId(pub u16);

/// Encoding facts; pixel conversion belongs to presentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraImageFormat {
    Jpeg,
    Png,
    Raw(RawImageLayout),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RawImageEncoding {
    Rgb8,
    Bgr8,
    Rgba8,
    Bgra8,
    Mono8,
}

impl RawImageEncoding {
    pub fn from_ros_encoding(value: &str) -> Result<Self, crate::DecodeError> {
        match value {
            "rgb8" => Ok(Self::Rgb8),
            "bgr8" => Ok(Self::Bgr8),
            "rgba8" => Ok(Self::Rgba8),
            "bgra8" => Ok(Self::Bgra8),
            "mono8" => Ok(Self::Mono8),
            other => Err(crate::DecodeError::UnsupportedFormat(other.into())),
        }
    }
    pub fn as_ros_encoding(self) -> &'static str {
        match self {
            Self::Rgb8 => "rgb8",
            Self::Bgr8 => "bgr8",
            Self::Rgba8 => "rgba8",
            Self::Bgra8 => "bgra8",
            Self::Mono8 => "mono8",
        }
    }

    pub fn channels(self) -> u32 {
        match self {
            Self::Rgb8 | Self::Bgr8 => 3,
            Self::Rgba8 | Self::Bgra8 => 4,
            Self::Mono8 => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawImageLayout {
    pub width: u32,
    pub height: u32,
    pub step: u32,
    pub encoding: RawImageEncoding,
}
impl RawImageLayout {
    /// Includes row padding. All supported channels are single bytes, so endianness is irrelevant.
    pub fn validate(self, data_len: usize) -> Result<(), crate::DecodeError> {
        if self.width == 0
            || self.height == 0
            || u64::from(self.width) * u64::from(self.height) > 32 * 1024 * 1024
            || self
                .width
                .checked_mul(self.encoding.channels())
                .is_none_or(|n| self.step < n)
            || u64::from(self.step) * u64::from(self.height) != data_len as u64
            || data_len > 32 * 1024 * 1024
        {
            return Err(crate::DecodeError::InvalidLength);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraFrame {
    pub camera_id: CameraId,
    pub measurement_time: MeasurementTime,
    pub arrival_time: ArrivalTime,
    pub frame_id: String,
    pub format: CameraImageFormat,
    pub data: Bytes,
}

/// Frame identity and projection facts, independent of the encoded image backing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraFrameMetadata {
    pub camera_id: CameraId,
    pub measurement_time: MeasurementTime,
    pub arrival_time: ArrivalTime,
    pub frame_id: String,
}

impl From<&CameraFrame> for CameraFrameMetadata {
    fn from(frame: &CameraFrame) -> Self {
        Self {
            camera_id: frame.camera_id,
            measurement_time: frame.measurement_time,
            arrival_time: frame.arrival_time,
            frame_id: frame.frame_id.clone(),
        }
    }
}

impl From<CameraFrame> for CameraFrameMetadata {
    fn from(frame: CameraFrame) -> Self {
        Self {
            camera_id: frame.camera_id,
            measurement_time: frame.measurement_time,
            arrival_time: frame.arrival_time,
            frame_id: frame.frame_id,
        }
        // The remaining JPEG field drops here, including any decode-queue byte lease.
    }
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
            format: crate::CameraImageFormat::Jpeg,
            data: Bytes::new(),
        }
    }

    #[test]
    fn metadata_preserves_projection_facts_without_retaining_jpeg_backing() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        struct Backing(Vec<u8>, Arc<AtomicBool>);
        impl AsRef<[u8]> for Backing {
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }
        impl Drop for Backing {
            fn drop(&mut self) {
                self.1.store(true, Ordering::SeqCst);
            }
        }
        let released = Arc::new(AtomicBool::new(false));
        let mut source = frame(123456789);
        source.camera_id = CameraId(7);
        source.measurement_time = MeasurementTime(987654321);
        source.frame_id = "camera_optical".into();
        source.data = Bytes::from_owner(Backing(vec![0; 1024 * 1024], Arc::clone(&released)));
        let borrowed = CameraFrameMetadata::from(&source);
        let owned = CameraFrameMetadata::from(source);
        assert!(released.load(Ordering::SeqCst));
        assert_eq!(owned, borrowed);
        assert_eq!(owned.camera_id, CameraId(7));
        assert_eq!(owned.arrival_time, ArrivalTime(123456789));
        assert_eq!(owned.measurement_time, MeasurementTime(987654321));
        assert_eq!(owned.frame_id, "camera_optical");
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
