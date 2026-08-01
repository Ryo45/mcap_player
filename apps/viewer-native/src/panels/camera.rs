use super::{CAMERA_CONFIG_VERSION, NativePanel, PanelFrameContext, PanelOutput, PlaceholderPanel};
use crate::{
    graphics::views::{CameraViewInput, show_camera_view},
    interaction::ViewerAction,
    workspace::CameraViewState,
};
use serde::{Deserialize, Serialize};
use viewer_layout::{PanelId, PanelNode};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ImageFit {
    #[default]
    Contain,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CameraPanelConfig {
    #[serde(default)]
    pub(crate) fit: ImageFit,
    #[serde(default = "default_true")]
    pub(crate) show_thumbnails: bool,
}

fn default_true() -> bool {
    true
}

pub(crate) struct CameraPanel {
    id: PanelId,
    title: Option<String>,
    config: CameraPanelConfig,
    pub(crate) state: CameraViewState,
}

impl CameraPanel {
    pub(crate) fn create(node: &PanelNode) -> NativePanel {
        if node.config_version != CAMERA_CONFIG_VERSION {
            return NativePanel::Placeholder(PlaceholderPanel::unsupported_version(
                node,
                CAMERA_CONFIG_VERSION,
            ));
        }
        match serde_json::from_value::<CameraPanelConfig>(node.config.clone()) {
            Ok(config) => NativePanel::Camera(Self {
                id: node.id.clone(),
                title: node.title.clone(),
                config,
                state: CameraViewState::default(),
            }),
            Err(error) => NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                format!("Invalid camera config: {error}"),
            )),
        }
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        context: &PanelFrameContext<'_>,
    ) -> PanelOutput {
        ui.push_id((self.id.as_str(), self.title.as_deref()), |ui| {
            let output = match self.config.fit {
                ImageFit::Contain => show_camera_view(
                    ui,
                    CameraViewInput {
                        cameras: &context.presentation.cameras,
                        textures: context.resources.camera_textures,
                        focused_camera: self.state.focused_camera,
                        show_thumbnails: self.config.show_thumbnails,
                        overlays: context.camera_overlays,
                    },
                ),
            };
            let mut panel_output = PanelOutput::default();
            if let Some(camera_id) = output.selected_camera {
                panel_output.actions.push(ViewerAction::SetFocusedCamera {
                    panel_id: self.id.clone(),
                    camera_id: Some(camera_id),
                });
            }
            panel_output
        })
        .inner
    }
}
