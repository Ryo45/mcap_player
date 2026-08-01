use std::collections::BTreeMap;
use viewer_core::{
    ArrivalTime, BevPathFrame, CameraCalibrationSet, CameraFrame, CameraId, OverlayStatus,
    TransformState,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CameraOverlayKey {
    camera_arrival: ArrivalTime,
    path_revision: u64,
    transform_revision: u64,
    image_size: (u32, u32),
    source_generation: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CameraOverlaySnapshot {
    pub camera_id: CameraId,
    pub camera_arrival: ArrivalTime,
    pub image_size: (u32, u32),
    pub projected_path: Vec<Option<[f32; 2]>>,
    pub status: OverlayStatus,
    pub revision: u64,
}

#[derive(Clone, Debug)]
struct CameraOverlayEntry {
    key: CameraOverlayKey,
    snapshot: CameraOverlaySnapshot,
}

#[derive(Clone, Debug, Default)]
pub struct CameraOverlayState {
    source_generation: u64,
    next_revision: u64,
    entries: BTreeMap<CameraId, CameraOverlayEntry>,
}

impl CameraOverlayState {
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        frame: &CameraFrame,
        image_size: (u32, u32),
        path: Option<&BevPathFrame>,
        path_revision: u64,
        transforms: &TransformState,
        transform_revision: u64,
        calibrations: &CameraCalibrationSet,
    ) -> bool {
        let key = CameraOverlayKey {
            camera_arrival: frame.arrival_time,
            path_revision,
            transform_revision,
            image_size,
            source_generation: self.source_generation,
        };
        if self
            .entries
            .get(&frame.camera_id)
            .is_some_and(|entry| entry.key == key)
        {
            return false;
        }
        let (projected_path, status) = path.map_or_else(
            || (Vec::new(), OverlayStatus::Waiting),
            |path| match calibrations.project_plan(frame, path, transforms, image_size) {
                Ok(projected) => (
                    projected.points,
                    OverlayStatus::Ready {
                        visible_points: projected.visible_points,
                    },
                ),
                Err(error) => (Vec::new(), OverlayStatus::Error(error.to_string())),
            },
        );
        self.next_revision = self.next_revision.wrapping_add(1).max(1);
        self.entries.insert(
            frame.camera_id,
            CameraOverlayEntry {
                key,
                snapshot: CameraOverlaySnapshot {
                    camera_id: frame.camera_id,
                    camera_arrival: frame.arrival_time,
                    image_size,
                    projected_path,
                    status,
                    revision: self.next_revision,
                },
            },
        );
        true
    }

    pub fn snapshot(&self, camera_id: CameraId) -> Option<&CameraOverlaySnapshot> {
        self.entries.get(&camera_id).map(|entry| &entry.snapshot)
    }

    pub fn snapshots(&self) -> impl Iterator<Item = &CameraOverlaySnapshot> {
        self.entries.values().map(|entry| &entry.snapshot)
    }

    pub fn reset_source(&mut self) {
        self.source_generation = self.source_generation.wrapping_add(1);
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CameraBaseImageTracker;
    use viewer_core::{MeasurementTime, TransformBatch, TransformStamped};

    fn camera(arrival: i64) -> CameraFrame {
        CameraFrame {
            camera_id: CameraId(0),
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(arrival),
            frame_id: "camera_front_optical_frame".to_owned(),
            jpeg: Vec::new(),
        }
    }

    fn path(points: Vec<[f32; 2]>) -> BevPathFrame {
        BevPathFrame {
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(11),
            frame_id: "base_link".to_owned(),
            points,
        }
    }

    fn transforms() -> TransformState {
        let mut transforms = TransformState::default();
        transforms.apply(TransformBatch {
            arrival_time: ArrivalTime(1),
            is_static: true,
            transforms: vec![TransformStamped {
                measurement_time: MeasurementTime(0),
                frame_id: "base_link".to_owned(),
                child_frame_id: "camera_front_optical_frame".to_owned(),
                translation: [0.0; 3],
                rotation: [-0.5, 0.5, -0.5, 0.5],
            }],
        });
        transforms
    }

    fn calibrations() -> CameraCalibrationSet {
        CameraCalibrationSet::from_json(include_str!("../../../config/camera_calibration.json"))
            .unwrap()
    }

    #[test]
    fn camera_change_rebuilds_overlay_but_an_identical_key_does_not() {
        let transforms = transforms();
        let path = path(vec![[0.0, 1.0], [0.0, 2.0]]);
        let calibrations = calibrations();
        let first_frame = camera(11);
        let next_frame = camera(12);
        let mut base = CameraBaseImageTracker::default();
        let mut state = CameraOverlayState::default();
        assert!(base.needs_update(&first_frame));
        base.mark_updated(&first_frame);
        assert!(state.update(
            &first_frame,
            (320, 240),
            Some(&path),
            1,
            &transforms,
            transforms.revision(),
            &calibrations,
        ));
        let first_revision = state.snapshot(CameraId(0)).unwrap().revision;
        assert!(!base.needs_update(&first_frame));
        assert!(!state.update(
            &first_frame,
            (320, 240),
            Some(&path),
            1,
            &transforms,
            transforms.revision(),
            &calibrations,
        ));
        assert!(base.needs_update(&next_frame));
        assert!(state.update(
            &next_frame,
            (320, 240),
            Some(&path),
            1,
            &transforms,
            transforms.revision(),
            &calibrations,
        ));
        assert_ne!(
            state.snapshot(CameraId(0)).unwrap().revision,
            first_revision
        );
    }

    #[test]
    fn path_or_transform_revision_rebuilds_without_changing_base_key() {
        let frame = camera(11);
        let transforms = transforms();
        let path = path(vec![[0.0, 1.0], [0.0, 2.0]]);
        let calibrations = calibrations();
        let mut base = CameraBaseImageTracker::default();
        base.mark_updated(&frame);
        let mut overlays = CameraOverlayState::default();
        assert!(overlays.update(
            &frame,
            (320, 240),
            Some(&path),
            1,
            &transforms,
            transforms.revision(),
            &calibrations,
        ));
        let initial = overlays.snapshot(CameraId(0)).unwrap().revision;

        assert!(overlays.update(
            &frame,
            (320, 240),
            Some(&path),
            2,
            &transforms,
            transforms.revision(),
            &calibrations,
        ));
        let path_revision = overlays.snapshot(CameraId(0)).unwrap().revision;
        assert_ne!(path_revision, initial);
        assert!(!base.needs_update(&frame));

        assert!(overlays.update(
            &frame,
            (320, 240),
            Some(&path),
            2,
            &transforms,
            transforms.revision().wrapping_add(1),
            &calibrations,
        ));
        assert_ne!(
            overlays.snapshot(CameraId(0)).unwrap().revision,
            path_revision
        );
        assert!(!base.needs_update(&frame));
    }

    #[test]
    fn missing_path_waits_and_projection_failure_reports_error() {
        let frame = camera(11);
        let transforms = transforms();
        let mut state = CameraOverlayState::default();
        assert!(state.update(
            &frame,
            (320, 240),
            None,
            0,
            &transforms,
            transforms.revision(),
            &calibrations(),
        ));
        assert_eq!(
            state.snapshot(CameraId(0)).unwrap().status,
            OverlayStatus::Waiting
        );
        assert!(state.update(
            &frame,
            (320, 240),
            Some(&path(vec![[0.0, 1.0]])),
            1,
            &transforms,
            transforms.revision(),
            &CameraCalibrationSet::default(),
        ));
        assert!(matches!(
            state.snapshot(CameraId(0)).unwrap().status,
            OverlayStatus::Error(_)
        ));
    }

    #[test]
    fn source_reset_removes_stale_overlay_and_forces_rebuild() {
        let frame = camera(11);
        let transforms = transforms();
        let mut state = CameraOverlayState::default();
        state.update(
            &frame,
            (320, 240),
            None,
            0,
            &transforms,
            transforms.revision(),
            &calibrations(),
        );
        state.reset_source();
        assert!(state.snapshot(CameraId(0)).is_none());
        assert!(state.update(
            &frame,
            (320, 240),
            None,
            0,
            &transforms,
            transforms.revision(),
            &calibrations(),
        ));
    }
}
