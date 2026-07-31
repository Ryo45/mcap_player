use crate::{
    session::SessionDiagnostics,
    workspace::{CameraViewState, SceneViewState},
};
use std::{collections::BTreeMap, time::Duration};
use viewer_core::{
    BevFrameBuilder, BevSnapshot, CameraId, DiagnosticsPresentation, DomainState, OverlayStatus,
    PresentationMetrics, SceneFrameBuilder, SceneSnapshot, ViewerPresentation,
};

pub(crate) struct CameraPresentationUpdate {
    pub(crate) camera_id: CameraId,
    pub(crate) overlay_status: OverlayStatus,
    pub(crate) jpeg_decode_time: Duration,
    pub(crate) upload_time: Duration,
}

pub(crate) struct PresentationFrame<'a> {
    pub(crate) viewer: ViewerPresentation,
    pub(crate) bev: BevSnapshot<'a>,
    pub(crate) scene: SceneSnapshot<'a>,
    pub(crate) static_transform_count: usize,
    pub(crate) dynamic_transform_count: usize,
}

#[derive(Default)]
pub(crate) struct PresentationState {
    scene_builder: SceneFrameBuilder,
    overlay_status: BTreeMap<CameraId, OverlayStatus>,
    metrics: PresentationMetrics,
}

impl PresentationState {
    pub(crate) fn build<'a>(
        &'a mut self,
        state: &'a DomainState,
        diagnostics: SessionDiagnostics,
        camera: &CameraViewState,
        scene_state: &SceneViewState,
        error: Option<String>,
    ) -> PresentationFrame<'a> {
        let viewer = ViewerPresentation::from_domain(
            state,
            &diagnostics.camera_topics,
            camera.focused_camera,
            &self.overlay_status,
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
        let scene = self
            .scene_builder
            .build(state, scene_state.accumulate_points);
        PresentationFrame {
            viewer,
            bev,
            scene,
            static_transform_count: state.transforms.static_len(),
            dynamic_transform_count: state.transforms.dynamic_len(),
        }
    }

    pub(crate) fn record_camera_updates(
        &mut self,
        updates: impl IntoIterator<Item = CameraPresentationUpdate>,
    ) {
        for update in updates {
            self.overlay_status
                .insert(update.camera_id, update.overlay_status);
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

    pub(crate) fn reset(&mut self) {
        self.scene_builder.reset();
        self.overlay_status.clear();
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
        let camera_state = CameraViewState {
            focused_camera: Some(CameraId(0)),
        };
        let scene_state = SceneViewState {
            accumulate_points: true,
        };
        let mut presentation = PresentationState::default();
        presentation.record_camera_updates([CameraPresentationUpdate {
            camera_id: CameraId(0),
            overlay_status: OverlayStatus::Ready { visible_points: 7 },
            jpeg_decode_time: Duration::from_millis(2),
            upload_time: Duration::from_millis(1),
        }]);
        presentation.advance_metrics(Duration::from_secs(1));

        let frame = presentation.build(&state, diagnostics(), &camera_state, &scene_state, None);
        let camera = frame.viewer.focused_camera().unwrap();
        assert_eq!(camera.overlay, OverlayStatus::Ready { visible_points: 7 });
        assert_eq!(camera.fps, 1.0);
        assert_eq!(frame.bev.path, [[1.0, 2.0]]);
        assert!(frame.scene.accumulate);
        drop(frame);

        presentation.reset();
        let frame = presentation.build(&state, diagnostics(), &camera_state, &scene_state, None);
        let camera = frame.viewer.focused_camera().unwrap();
        assert_eq!(camera.overlay, OverlayStatus::Waiting);
        assert_eq!(camera.fps, 0.0);
    }
}
