mod camera;
mod diagnostics;
mod telemetry;

pub use camera::{CameraPresentation, OverlayStatus};
pub use diagnostics::DiagnosticsPresentation;
pub use telemetry::TelemetryPresentation;

use crate::{BevState, CameraId, CameraState, PointCloudState, TelemetryState};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewerPresentation {
    pub cameras: Vec<CameraPresentation>,
    pub telemetry: Option<TelemetryPresentation>,
    pub diagnostics: DiagnosticsPresentation,
}

/// Narrow read-only input used to build one UI presentation snapshot.
pub struct ViewerPresentationInput<'a> {
    pub cameras: &'a CameraState,
    pub telemetry: &'a TelemetryState,
    pub path: &'a BevState,
    pub point_cloud: Option<&'a PointCloudState>,
    pub camera_topics: &'a [(CameraId, String)],
    pub focused_camera: Option<CameraId>,
    pub overlays: &'a BTreeMap<CameraId, OverlayStatus>,
    pub diagnostics: DiagnosticsPresentation,
}

impl ViewerPresentation {
    pub fn from_features(input: ViewerPresentationInput<'_>) -> Self {
        let ViewerPresentationInput {
            cameras,
            telemetry,
            path,
            point_cloud,
            camera_topics,
            focused_camera,
            overlays,
            mut diagnostics,
        } = input;
        diagnostics.path_points = path.latest().map_or(0, |frame| frame.points.len());
        diagnostics.scan_points = point_cloud
            .and_then(PointCloudState::latest)
            .map_or(0, |frame| frame.points.len());

        let topics = camera_topics.iter().cloned().collect::<BTreeMap<_, _>>();
        let camera_ids = topics
            .keys()
            .copied()
            .chain(cameras.ids())
            .collect::<BTreeSet<_>>();
        let cameras = camera_ids
            .into_iter()
            .map(|camera_id| CameraPresentation {
                id: camera_id,
                topic: topics
                    .get(&camera_id)
                    .cloned()
                    .unwrap_or_else(|| format!("camera {}", camera_id.0)),
                status: cameras.status_for(camera_id),
                fps: diagnostics
                    .performance
                    .camera_fps
                    .get(&camera_id)
                    .copied()
                    .unwrap_or_default(),
                overlay: overlays.get(&camera_id).cloned().unwrap_or_default(),
                focused: Some(camera_id) == focused_camera,
            })
            .collect();
        Self {
            cameras,
            telemetry: telemetry.latest().map(TelemetryPresentation::from),
            diagnostics,
        }
    }

    pub fn focused_camera(&self) -> Option<&CameraPresentation> {
        self.cameras.iter().find(|camera| camera.focused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ArrivalTime, CameraFrame, CameraStatus, MeasurementTime, PresentationSnapshot,
        TelemetryFrame,
    };

    #[test]
    fn builds_camera_and_telemetry_values_from_feature_states() {
        let mut cameras = CameraState::default();
        cameras.apply(CameraFrame {
            camera_id: CameraId(1),
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(2),
            frame_id: "rear".into(),
            jpeg: vec![].into(),
        });
        let mut telemetry = TelemetryState::default();
        telemetry.apply(TelemetryFrame {
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(2),
            frame_id: "odom".into(),
            child_frame_id: "base_link".into(),
            position_x: 3.0,
            position_y: 4.0,
            yaw_radians: 0.5,
            forward_velocity: 1.0,
            speed: 1.25,
            yaw_rate: 0.1,
        });
        let mut performance = PresentationSnapshot::default();
        performance.camera_fps.insert(CameraId(1), 5.0);
        let mut overlays = BTreeMap::new();
        overlays.insert(CameraId(1), OverlayStatus::Ready { visible_points: 7 });
        let path = BevState::default();
        let topics = [
            (CameraId(0), "/front".into()),
            (CameraId(1), "/rear".into()),
        ];
        let model = ViewerPresentation::from_features(ViewerPresentationInput {
            cameras: &cameras,
            telemetry: &telemetry,
            path: &path,
            point_cloud: None,
            camera_topics: &topics,
            focused_camera: Some(CameraId(1)),
            overlays: &overlays,
            diagnostics: DiagnosticsPresentation {
                performance,
                ..DiagnosticsPresentation::default()
            },
        });

        assert_eq!(model.cameras.len(), 2);
        let focused = model.focused_camera().unwrap();
        assert_eq!(focused.id, CameraId(1));
        assert_eq!(focused.status, CameraStatus::Ready);
        assert_eq!(focused.fps, 5.0);
        assert_eq!(focused.overlay.to_string(), "plan 7 visible pts");
        assert_eq!(model.telemetry.unwrap().position_x, 3.0);
    }
}
