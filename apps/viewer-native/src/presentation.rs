use crate::session::SessionDiagnostics;
use std::{collections::BTreeMap, time::Duration};
#[cfg(test)]
use viewer_core::OverlayStatus;
use viewer_core::{
    BevFrameBuilder, BevSnapshot, CameraCalibrationSet, CameraId, DiagnosticsPresentation,
    DomainState, PresentationMetrics, SceneFrameBuilder, SceneSnapshot, ViewerPresentation,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PresentationTransition {
    SourceChanged,
    Seeked,
}

#[derive(Default)]
pub(crate) struct PresentationState {
    scene_builder: SceneFrameBuilder,
    camera_overlays: CameraOverlayState,
    calibrations: CameraCalibrationSet,
    metrics: PresentationMetrics,
}

impl PresentationState {
    pub(crate) fn set_camera_calibrations(&mut self, calibrations: CameraCalibrationSet) {
        self.calibrations = calibrations;
        self.camera_overlays.reset_source();
    }

    pub(crate) fn update_camera_overlays(
        &mut self,
        state: &DomainState,
        base_images: &[(CameraId, viewer_core::ArrivalTime, (u32, u32))],
    ) {
        for (camera_id, base_arrival, image_size) in base_images {
            let Some(frame) = state.camera.latest_for(*camera_id) else {
                continue;
            };
            if frame.arrival_time != *base_arrival {
                continue;
            }
            self.camera_overlays.update(
                frame,
                *image_size,
                state.bev.latest(),
                state.bev.revision(),
                &state.transforms,
                state.transforms.revision(),
                &self.calibrations,
            );
        }
    }

    pub(crate) fn build<'a>(
        &'a mut self,
        state: &'a DomainState,
        diagnostics: SessionDiagnostics,
        focused_camera: Option<CameraId>,
        accumulate_points: bool,
        error: Option<String>,
    ) -> PresentationFrame<'a> {
        let overlay_status = self
            .camera_overlays
            .snapshots()
            .map(|snapshot| (snapshot.camera_id, snapshot.status.clone()))
            .collect::<BTreeMap<_, _>>();
        let viewer = ViewerPresentation::from_domain(
            state,
            &diagnostics.camera_topics,
            focused_camera,
            &overlay_status,
            DiagnosticsPresentation {
                source: diagnostics.source_name,
                primary_topic: diagnostics.primary_topic,
                counters: diagnostics.counters,
                playback_performance: diagnostics.playback_performance,
                performance: self.metrics.snapshot().clone(),
                error,
                ..DiagnosticsPresentation::default()
            },
        );
        let bev = BevFrameBuilder::new(state).build();
        let scene = self.scene_builder.build(state, accumulate_points);
        PresentationFrame {
            viewer,
            camera_overlays: &self.camera_overlays,
            bev,
            scene,
            static_transform_count: state.transforms.static_len(),
            dynamic_transform_count: state.transforms.dynamic_len(),
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
        self.scene_builder.reset();
        self.camera_overlays.reset_source();
        self.metrics.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{ArrivalTime, BevPathFrame, MeasurementTime, PipelineCounters};

    fn diagnostics() -> SessionDiagnostics {
        SessionDiagnostics {
            source_name: "fixture".into(),
            primary_topic: "/camera/front/image/compressed".into(),
            camera_topics: vec![(CameraId(0), "/camera/front/image/compressed".into())],
            counters: PipelineCounters::default(),
            playback_performance: None,
        }
    }

    #[test]
    fn owns_and_resets_cpu_presentation_caches() {
        let mut state = DomainState::default();
        state.bev.apply(BevPathFrame {
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(2),
            frame_id: "base_link".into(),
            points: vec![[1.0, 2.0]],
        });
        let mut presentation = PresentationState::default();
        presentation.record_camera_updates([CameraBasePresentationUpdate {
            camera_id: CameraId(0),
            jpeg_decode_time: Duration::from_millis(2),
            upload_time: Duration::from_millis(1),
        }]);
        presentation.advance_metrics(Duration::from_secs(1));

        let frame = presentation.build(&state, diagnostics(), Some(CameraId(0)), true, None);
        let camera = frame.viewer.focused_camera().unwrap();
        assert_eq!(camera.overlay, OverlayStatus::Waiting);
        assert_eq!(camera.fps, 1.0);
        assert_eq!(frame.bev.path, [[1.0, 2.0]]);
        assert!(frame.scene.accumulate);
        drop(frame);

        presentation.apply_transition(PresentationTransition::Seeked);
        let frame = presentation.build(&state, diagnostics(), Some(CameraId(0)), true, None);
        let camera = frame.viewer.focused_camera().unwrap();
        assert_eq!(camera.overlay, OverlayStatus::Waiting);
        assert_eq!(camera.fps, 0.0);
    }
}
