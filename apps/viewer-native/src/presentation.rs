use crate::session::SessionDiagnostics;
use std::{collections::BTreeMap, time::Duration};
#[cfg(test)]
use viewer_core::OverlayStatus;
use viewer_core::{
    BevSnapshot, CameraCalibrationSet, CameraId, DiagnosticsPresentation, PresentationMetrics,
    ScenePresentationState, SceneSnapshot, ViewerPresentation,
};
use viewer_renderer::CameraOverlayState;

pub(crate) struct CameraBasePresentationUpdate {
    pub(crate) camera_id: CameraId,
    pub(crate) jpeg_decode_time: Duration,
    pub(crate) upload_time: Duration,
}

pub(crate) struct PresentationFrame<'a> {
    pub(crate) viewer: ViewerPresentation,
    pub(crate) camera_overlays: &'a CameraOverlayState,
    pub(crate) bev: BevSnapshot<'a>,
    pub(crate) scene: SceneSnapshot<'a>,
    pub(crate) static_transform_count: usize,
    pub(crate) dynamic_transform_count: usize,
}

pub(crate) struct PresentationBuildInput<'a> {
    pub(crate) cameras: &'a viewer_core::CameraController,
    pub(crate) path: &'a viewer_core::PathController,
    pub(crate) odometry: &'a viewer_core::OdometryController,
    pub(crate) transforms: &'a viewer_core::TransformController,
    pub(crate) point_cloud_controller: Option<&'a viewer_core::PointCloudController>,
    pub(crate) diagnostics: SessionDiagnostics,
    pub(crate) focused_camera: Option<CameraId>,
    pub(crate) accumulate_points: bool,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PresentationTransition {
    SourceChanged,
    Seeked,
}

#[derive(Default)]
pub(crate) struct PresentationState {
    camera_overlays: CameraOverlayState,
    calibrations: CameraCalibrationSet,
    metrics: PresentationMetrics,
    scene: ScenePresentationState,
    platform: Option<viewer_core::PlatformGeometry>,
    path_projection: viewer_core::PathProjectionPolicy,
}

impl PresentationState {
    pub(crate) fn set_platform(&mut self, geometry: Option<viewer_core::PlatformGeometry>) {
        self.platform = geometry;
        self.scene.reset();
    }
    pub(crate) fn set_path_projection(&mut self, policy: viewer_core::PathProjectionPolicy) {
        self.path_projection = policy;
        self.camera_overlays.reset_source();
        self.scene.reset();
    }

    pub(crate) fn set_camera_calibrations(&mut self, calibrations: CameraCalibrationSet) {
        self.calibrations = calibrations;
        self.camera_overlays.reset_source();
    }

    pub(crate) fn update_camera_overlays(
        &mut self,
        path: &viewer_core::PathController,
        transforms: &viewer_core::TransformController,
        base_images: &[(viewer_core::CameraFrame, (u32, u32))],
    ) {
        for (frame, image_size) in base_images {
            self.camera_overlays.update(
                frame,
                *image_size,
                path.state().latest(),
                path.state().revision(),
                transforms.state(),
                transforms.state().revision(),
                &self.calibrations,
                &self.path_projection,
            );
        }
    }

    pub(crate) fn build<'a>(
        &'a mut self,
        input: PresentationBuildInput<'a>,
    ) -> PresentationFrame<'a> {
        let PresentationBuildInput {
            cameras,
            path,
            odometry,
            transforms,
            point_cloud_controller,
            diagnostics,
            focused_camera,
            accumulate_points,
            error,
        } = input;
        let overlay_status = self
            .camera_overlays
            .snapshots()
            .map(|snapshot| (snapshot.camera_id, snapshot.status.clone()))
            .collect::<BTreeMap<_, _>>();
        let viewer = ViewerPresentation::from_features(viewer_core::ViewerPresentationInput {
            cameras: cameras.state(),
            telemetry: odometry.state(),
            path: path.state(),
            point_cloud: point_cloud_controller.map(viewer_core::PointCloudController::point_cloud),
            camera_topics: &diagnostics.camera_topics,
            focused_camera,
            overlays: &overlay_status,
            diagnostics: DiagnosticsPresentation {
                source: diagnostics.source_name,
                primary_topic: diagnostics.primary_topic,
                counters: diagnostics.counters,
                playback_performance: diagnostics.playback_performance,
                performance: self.metrics.snapshot().clone(),
                error,
                ..DiagnosticsPresentation::default()
            },
        });
        let static_transform_count = transforms.state().static_len();
        let dynamic_transform_count = transforms.state().dynamic_len();
        let empty = viewer_core::PointCloudState::default();
        let scene = self.scene.build(viewer_core::ScenePresentationInput {
            path: path.state(),
            telemetry: odometry.state(),
            point_cloud: point_cloud_controller.map_or(&empty, |c| c.point_cloud()),
            transforms: transforms.state(),
            platform: self.platform.as_ref(),
            path_projection: &self.path_projection,
            accumulate: accumulate_points,
        });
        let bev = scene.bev;
        PresentationFrame {
            viewer,
            camera_overlays: &self.camera_overlays,
            bev,
            scene,
            static_transform_count,
            dynamic_transform_count,
        }
    }

    pub(crate) fn record_camera_updates(
        &mut self,
        updates: impl IntoIterator<Item = CameraBasePresentationUpdate>,
    ) {
        for update in updates {
            self.metrics.record_camera(
                update.camera_id,
                update.jpeg_decode_time,
                update.upload_time,
            );
        }
    }

    pub(crate) fn record_render(&mut self, elapsed: Duration) {
        self.metrics.record_render(elapsed);
    }

    pub(crate) fn advance_metrics(&mut self, elapsed: Duration) {
        self.metrics.advance(elapsed);
    }

    pub(crate) fn apply_transition(&mut self, _transition: PresentationTransition) {
        self.camera_overlays.reset_source();
        self.metrics.reset();
        self.scene.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{
        PlaybackRequirements, ProcessingCounters, SessionPlan, SourceCatalog, StreamDescriptor,
        StreamId, StreamTimingSummary,
    };

    fn diagnostics() -> SessionDiagnostics {
        SessionDiagnostics {
            source_name: "fixture".into(),
            primary_topic: "/camera/front/image/compressed".into(),
            camera_topics: vec![(CameraId(0), "/camera/front/image/compressed".into())],
            counters: ProcessingCounters::default(),
            playback_performance: None,
        }
    }

    #[test]
    fn owns_and_resets_cpu_presentation_caches() {
        let mut workspace = crate::workspace::NativeWorkspace::default();
        let catalog = SourceCatalog {
            time_range: None,
            streams: vec![StreamDescriptor {
                schema_definition: None,
                id: StreamId(1),
                topic: "/camera/front/image/compressed".into(),
                schema: "sensor_msgs/msg/CompressedImage".into(),
                message_encoding: "cdr".into(),
                timing: StreamTimingSummary::default(),
            }],
            capabilities: Default::default(),
        };
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_all_cameras();
        let plan = SessionPlan::build(
            &catalog,
            "/camera/front/image/compressed",
            &requirements,
            workspace.bindings(),
        )
        .unwrap();
        workspace.configure_session(&plan);
        let mut presentation = PresentationState::default();
        presentation.record_camera_updates([CameraBasePresentationUpdate {
            camera_id: CameraId(0),
            jpeg_decode_time: Duration::from_millis(2),
            upload_time: Duration::from_millis(1),
        }]);
        presentation.advance_metrics(Duration::from_secs(1));

        let frame = presentation.build(PresentationBuildInput {
            cameras: workspace.runtime().cameras(),
            path: workspace.runtime().path(),
            odometry: workspace.runtime().odometry(),
            transforms: workspace.runtime().transforms(),
            point_cloud_controller: workspace.runtime().point_cloud(),
            diagnostics: diagnostics(),
            focused_camera: Some(CameraId(0)),
            accumulate_points: true,
            error: None,
        });
        let camera = frame.viewer.focused_camera().unwrap();
        assert_eq!(camera.overlay, OverlayStatus::Waiting);
        assert_eq!(camera.fps, 1.0);
        assert!(frame.bev.path.is_empty());
        assert!(frame.scene.cloud.is_empty());
        drop(frame);

        presentation.apply_transition(PresentationTransition::Seeked);
        let frame = presentation.build(PresentationBuildInput {
            cameras: workspace.runtime().cameras(),
            path: workspace.runtime().path(),
            odometry: workspace.runtime().odometry(),
            transforms: workspace.runtime().transforms(),
            point_cloud_controller: workspace.runtime().point_cloud(),
            diagnostics: diagnostics(),
            focused_camera: Some(CameraId(0)),
            accumulate_points: true,
            error: None,
        });
        let camera = frame.viewer.focused_camera().unwrap();
        assert_eq!(camera.overlay, OverlayStatus::Waiting);
        assert_eq!(camera.fps, 0.0);
    }
}
