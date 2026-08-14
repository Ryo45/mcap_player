mod bev;
mod camera;
mod placeholder;
mod plot;
mod runtime;
mod scene;

pub(crate) use bev::BevPanel;
pub(crate) use camera::CameraPanel;
pub(crate) use placeholder::PlaceholderPanel;
pub(crate) use plot::PlotPanel;
pub(crate) use runtime::PanelRuntimeStore;
pub(crate) use scene::ScenePanel;

use crate::{
    graphics::views::{CameraTextureView, SceneViewOutput},
    interaction::ViewerAction,
    workspace::ViewerInteractionState,
};
use scene_renderer::SceneCameraMode;
use viewer_core::{
    Bookmark, LoadedSignal, PlaybackView, SceneDiagnostics, SignalOverview, ViewerPresentation,
};

pub(crate) const CAMERA_CONFIG_VERSION: u32 = 1;
pub(crate) const BEV_CONFIG_VERSION: u32 = 1;
pub(crate) const PLOT_CONFIG_VERSION: u32 = 1;
pub(crate) const SCENE_CONFIG_VERSION: u32 = 1;

pub(crate) enum NativePanel {
    Camera(CameraPanel),
    Bev(BevPanel),
    Plot(PlotPanel),
    Scene(ScenePanel),
    Placeholder(PlaceholderPanel),
}

impl NativePanel {
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        context: &PanelFrameContext<'_>,
    ) -> PanelOutput {
        match self {
            Self::Camera(panel) => panel.show(ui, context),
            Self::Bev(panel) => panel.show(ui, context),
            Self::Plot(panel) => panel.show(ui, context),
            Self::Scene(panel) => panel.show(ui, context),
            Self::Placeholder(panel) => panel.show(ui),
        }
    }

    pub(crate) fn reset_for_source(&mut self, focused_camera: Option<viewer_core::CameraId>) {
        match self {
            Self::Camera(panel) => panel.state.focused_camera = focused_camera,
            Self::Plot(panel) => panel.reset_for_source(),
            Self::Bev(_) | Self::Scene(_) | Self::Placeholder(_) => {}
        }
    }

    pub(crate) fn set_focused_camera(&mut self, camera_id: Option<viewer_core::CameraId>) -> bool {
        let Self::Camera(panel) = self else {
            return false;
        };
        panel.state.focused_camera = camera_id;
        true
    }

    pub(crate) fn set_accumulate_points(&mut self, accumulate: bool) -> bool {
        let Self::Scene(panel) = self else {
            return false;
        };
        panel.state.accumulate_points = accumulate;
        true
    }

    pub(crate) fn focused_camera(&self) -> Option<viewer_core::CameraId> {
        match self {
            Self::Camera(panel) => panel.state.focused_camera,
            _ => None,
        }
    }

    pub(crate) fn accumulate_points(&self) -> Option<bool> {
        match self {
            Self::Scene(panel) => Some(panel.state.accumulate_points),
            _ => None,
        }
    }

    pub(crate) fn contribute_data_requirements(&self, requirements: &mut PanelDataRequirements) {
        if let Self::Plot(panel) = self {
            panel.contribute_data_requirements(requirements);
        }
    }

    #[cfg(test)]
    pub(crate) fn kind_name(&self) -> &'static str {
        match self {
            Self::Camera(_) => "camera",
            Self::Bev(_) => "bev",
            Self::Plot(_) => "plot",
            Self::Scene(_) => "scene-3d",
            Self::Placeholder(_) => "placeholder",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PanelDataRequirements {
    pub(crate) vehicle_speed: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct PlotDataView<'a> {
    pub(crate) signal: Option<&'a LoadedSignal>,
    pub(crate) loading: bool,
    pub(crate) error: Option<&'a str>,
}

#[derive(Clone, Copy)]
pub(crate) struct PanelResourceView<'a> {
    pub(crate) camera_textures: &'a [CameraTextureView],
    pub(crate) preview_camera_textures: &'a [CameraTextureView],
    pub(crate) bev_texture: egui::TextureId,
    pub(crate) scene_texture: egui::TextureId,
}

#[derive(Clone, Copy)]
pub(crate) struct PreviewDataView<'a> {
    pub(crate) active: bool,
    pub(crate) speed_overview: Option<&'a SignalOverview>,
    pub(crate) bookmarks: &'a [Bookmark],
}

#[derive(Clone, Copy)]
pub(crate) struct SceneDataView<'a> {
    pub(crate) diagnostics: &'a SceneDiagnostics,
    pub(crate) visible_scan_points: usize,
    pub(crate) camera_distance: f32,
    pub(crate) camera_mode: SceneCameraMode,
    pub(crate) static_transform_count: usize,
    pub(crate) dynamic_transform_count: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct PanelFrameContext<'a> {
    pub(crate) playback: Option<PlaybackView>,
    pub(crate) presentation: &'a ViewerPresentation,
    pub(crate) camera_overlays: &'a viewer_renderer::CameraOverlayState,
    pub(crate) interaction: &'a ViewerInteractionState,
    pub(crate) plot: PlotDataView<'a>,
    pub(crate) preview: PreviewDataView<'a>,
    pub(crate) resources: PanelResourceView<'a>,
    pub(crate) scene: SceneDataView<'a>,
}

#[derive(Default)]
pub(crate) struct PanelOutput {
    pub(crate) actions: Vec<ViewerAction>,
    pub(crate) render_requests: PanelRenderRequests,
}

#[derive(Default)]
pub(crate) struct PanelRenderRequests {
    pub(crate) bev_size: Option<egui::Vec2>,
    pub(crate) scene: Option<SceneViewOutput>,
}

impl PanelOutput {
    pub(crate) fn merge(&mut self, mut other: Self) {
        self.actions.append(&mut other.actions);
        if other.render_requests.bev_size.is_some() {
            self.render_requests.bev_size = other.render_requests.bev_size;
        }
        if other.render_requests.scene.is_some() {
            self.render_requests.scene = other.render_requests.scene;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::PlaybackCommand;

    #[test]
    fn panel_output_aggregation_keeps_actions_and_render_requests() {
        let mut aggregate = PanelOutput::default();
        aggregate.merge(PanelOutput {
            actions: vec![ViewerAction::Playback(PlaybackCommand::Toggle)],
            render_requests: PanelRenderRequests {
                bev_size: Some(egui::vec2(320.0, 180.0)),
                scene: None,
            },
        });
        assert_eq!(aggregate.actions.len(), 1);
        assert_eq!(
            aggregate.render_requests.bev_size,
            Some(egui::vec2(320.0, 180.0))
        );
        assert!(aggregate.render_requests.scene.is_none());
    }
}
