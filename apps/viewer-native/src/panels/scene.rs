use super::{NativePanel, PanelFrameContext, PanelOutput, PlaceholderPanel, SCENE_CONFIG_VERSION};
use crate::{
    graphics::views::{SceneViewInput, show_scene_view},
    interaction::ViewerAction,
    workspace::SceneViewState,
};
use serde::{Deserialize, Serialize};
use viewer_layout::{PanelId, PanelNode};

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

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        context: &PanelFrameContext<'_>,
    ) -> PanelOutput {
        ui.push_id((self.id.as_str(), self.title.as_deref()), |ui| {
            let output = show_scene_view(
                ui,
                SceneViewInput {
                    texture_id: context.resources.scene_texture,
                    scan_points: context.presentation.diagnostics.scan_points,
                    visible_scan_points: context.scene.visible_scan_points,
                    camera_distance: context.scene.camera_distance,
                    camera_mode: context.scene.camera_mode,
                    accumulate_points: self.state.accumulate_points,
                    diagnostics: context.scene.diagnostics,
                    static_transform_count: context.scene.static_transform_count,
                    dynamic_transform_count: context.scene.dynamic_transform_count,
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
