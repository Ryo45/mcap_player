mod camera;
mod diagnostics;
mod telemetry;

pub use camera::{CameraPresentation, OverlayStatus};
pub use diagnostics::DiagnosticsPresentation;
pub use telemetry::TelemetryPresentation;

use crate::{CameraId, DomainState};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewerPresentation {
    pub cameras: Vec<CameraPresentation>,
    pub telemetry: Option<TelemetryPresentation>,
    pub diagnostics: DiagnosticsPresentation,
}

impl ViewerPresentation {
    pub fn from_domain(
        state: &DomainState,
        camera_topics: &[(CameraId, String)],
        focused_camera: Option<CameraId>,
        overlays: &BTreeMap<CameraId, OverlayStatus>,
        mut diagnostics: DiagnosticsPresentation,
    ) -> Self {
        diagnostics.path_points = state.bev.latest().map_or(0, |frame| frame.points.len());
        diagnostics.scan_points = state
            .point_cloud
            .latest()
            .map_or(0, |frame| frame.points.len());

        let topics = camera_topics.iter().cloned().collect::<BTreeMap<_, _>>();
        let camera_ids = topics
            .keys()
            .copied()
            .chain(state.camera.ids())
            .collect::<BTreeSet<_>>();
        let cameras = camera_ids
            .into_iter()
            .map(|camera_id| CameraPresentation {
                id: camera_id,
                topic: topics
                    .get(&camera_id)
                    .cloned()
                    .unwrap_or_else(|| format!("camera {}", camera_id.0)),
                status: state.camera.status_for(camera_id),
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
            telemetry: state.telemetry.latest().map(TelemetryPresentation::from),
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
    fn builds_camera_and_telemetry_values_from_domain_state() {
        let mut state = DomainState::default();
        state.camera.apply(CameraFrame {
            camera_id: CameraId(1),
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(2),
            frame_id: "rear".into(),
            jpeg: vec![].into(),
        });
        state.telemetry.apply(TelemetryFrame {
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
        let model = ViewerPresentation::from_domain(
            &state,
            &[
                (CameraId(0), "/front".into()),
                (CameraId(1), "/rear".into()),
            ],
            Some(CameraId(1)),
            &overlays,
            DiagnosticsPresentation {
                performance,
                ..DiagnosticsPresentation::default()
            },
        );

        assert_eq!(model.cameras.len(), 2);
        let focused = model.focused_camera().unwrap();
        assert_eq!(focused.id, CameraId(1));
        assert_eq!(focused.status, CameraStatus::Ready);
        assert_eq!(focused.fps, 5.0);
        assert_eq!(focused.overlay.to_string(), "plan 7 visible pts");
        assert_eq!(model.telemetry.unwrap().position_x, 3.0);
    }
}
