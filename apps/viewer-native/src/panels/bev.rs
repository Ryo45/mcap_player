use super::{
    BEV_CONFIG_VERSION, NativePanel, PanelDataRequirements, PanelFrameContext, PanelOutput,
    PlaceholderPanel,
};
use crate::graphics::views::{BevViewInput, show_bev_view};
use serde::{Deserialize, Serialize};
use viewer_layout::{PanelId, PanelNode};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BevPanelConfig {}

pub(crate) struct BevPanel {
    id: PanelId,
    title: Option<String>,
    _config: BevPanelConfig,
}

impl BevPanel {
    pub(crate) fn contribute_data_requirements(&self, requirements: &mut PanelDataRequirements) {
        requirements.playback.require_path();
    }

    pub(crate) fn create(node: &PanelNode) -> NativePanel {
        if node.config_version != BEV_CONFIG_VERSION {
            return NativePanel::Placeholder(PlaceholderPanel::unsupported_version(
                node,
                BEV_CONFIG_VERSION,
            ));
        }
        match serde_json::from_value::<BevPanelConfig>(node.config.clone()) {
            Ok(config) => NativePanel::Bev(Self {
                id: node.id.clone(),
                title: node.title.clone(),
                _config: config,
            }),
            Err(error) => NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                format!("Invalid BEV config: {error}"),
            )),
        }
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        context: &PanelFrameContext<'_>,
    ) -> PanelOutput {
        ui.push_id((self.id.as_str(), self.title.as_deref()), |ui| {
            let output = show_bev_view(
                ui,
                BevViewInput {
                    texture_id: context.resources.bev_texture,
                    path_points: context.presentation.diagnostics.path_points,
                },
            );
            PanelOutput {
                render_requests: super::PanelRenderRequests {
                    bev_size: Some(output.logical_size),
                    scene: None,
                },
                ..PanelOutput::default()
            }
        })
        .inner
    }
}
