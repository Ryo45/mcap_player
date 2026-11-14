const BEV_CONFIG_VERSION: u32 = 1;
use crate::panel::PanelOutput;
use crate::views::bev::{BevViewInput, show_bev_view};
use serde::{Deserialize, Serialize};
use viewer_layout::{PanelId, PanelNode};

#[derive(Clone, Copy)]
pub struct BevPanelInput {
    pub texture_id: egui::TextureId,
    pub path_points: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BevPanelConfig {}

pub struct BevPanel {
    id: PanelId,
    title: Option<String>,
    _config: BevPanelConfig,
}

impl BevPanel {
    pub fn contribute_data_requirements(
        &self,
        requirements: &mut viewer_core::PlaybackRequirements,
    ) {
        requirements.require_path();
    }

    pub fn create(node: &PanelNode) -> Result<Self, String> {
        if node.config_version != BEV_CONFIG_VERSION {
            return Err(format!(
                "Unsupported {} config version {}; expected {}",
                node.panel_type, node.config_version, BEV_CONFIG_VERSION
            ));
        }
        match serde_json::from_value::<BevPanelConfig>(node.config.clone()) {
            Ok(config) => Ok(Self {
                id: node.id.clone(),
                title: node.title.clone(),
                _config: config,
            }),
            Err(error) => Err(format!("Invalid BEV config: {error}")),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, input: BevPanelInput) -> PanelOutput {
        ui.push_id((self.id.as_str(), self.title.as_deref()), |ui| {
            let output = show_bev_view(
                ui,
                BevViewInput {
                    texture_id: input.texture_id,
                    path_points: input.path_points,
                },
            );
            PanelOutput {
                render_requests: crate::panel::PanelRenderRequests {
                    bev_size: Some(output.logical_size),
                    scene: None,
                    ..Default::default()
                },
                ..PanelOutput::default()
            }
        })
        .inner
    }
}
