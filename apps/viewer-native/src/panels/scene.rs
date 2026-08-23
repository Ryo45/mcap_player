use super::{
    NativePanel, PanelDataRequirements, PanelOutput, PlaceholderPanel, SCENE_CONFIG_VERSION,
};
use crate::{
    graphics::views::{SceneViewInput, show_scene_view},
    interaction::ViewerAction,
    workspace::SceneViewState,
};
use scene_renderer::SceneCameraMode;
use serde::{Deserialize, Serialize};
use viewer_core::SceneDiagnostics;
use viewer_layout::{PanelId, PanelNode};

#[derive(Clone, Copy)]
pub(crate) struct ScenePanelInput<'a> {
    pub(crate) texture_id: egui::TextureId,
    pub(crate) scan_points: usize,
    pub(crate) visible_scan_points: usize,
    pub(crate) camera_distance: f32,
    pub(crate) camera_mode: SceneCameraMode,
    pub(crate) diagnostics: &'a SceneDiagnostics,
    pub(crate) static_transform_count: usize,
    pub(crate) dynamic_transform_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ScenePanelConfig {}

pub(crate) struct ScenePanel {
    id: PanelId,
    title: Option<String>,
    _config: ScenePanelConfig,
    pub(crate) state: SceneViewState,
}

impl ScenePanel {
    pub(crate) fn create(node: &PanelNode) -> NativePanel {
        if node.config_version != SCENE_CONFIG_VERSION {
            return NativePanel::Placeholder(PlaceholderPanel::unsupported_version(
                node,
                SCENE_CONFIG_VERSION,
            ));
        }
        match serde_json::from_value::<ScenePanelConfig>(node.config.clone()) {
            Ok(config) => NativePanel::Scene(Self {
                id: node.id.clone(),
                title: node.title.clone(),
                _config: config,
                state: SceneViewState::default(),
            }),
            Err(error) => NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                format!("Invalid scene config: {error}"),
            )),
        }
    }

    pub(crate) fn contribute_data_requirements(&self, requirements: &mut PanelDataRequirements) {
        requirements.playback.optional_path();
        requirements.playback.optional_odometry();
        requirements.playback.require_point_cloud();
        requirements.playback.require_transforms();
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, input: ScenePanelInput<'_>) -> PanelOutput {
        ui.push_id((self.id.as_str(), self.title.as_deref()), |ui| {
            let output = show_scene_view(
                ui,
                SceneViewInput {
                    texture_id: input.texture_id,
                    scan_points: input.scan_points,
                    visible_scan_points: input.visible_scan_points,
                    camera_distance: input.camera_distance,
                    camera_mode: input.camera_mode,
                    accumulate_points: self.state.accumulate_points,
                    diagnostics: input.diagnostics,
                    static_transform_count: input.static_transform_count,
                    dynamic_transform_count: input.dynamic_transform_count,
                },
            );
            let mut actions = Vec::new();
            if let Some(accumulate) = output.selected_accumulation {
                actions.push(ViewerAction::SetAccumulatePoints {
                    panel_id: self.id.clone(),
                    accumulate,
                });
            }
            PanelOutput {
                actions,
                render_requests: super::PanelRenderRequests {
                    bev_size: None,
                    scene: Some(output),
                },
            }
        })
        .inner
    }
}
