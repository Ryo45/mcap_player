use super::PanelOutput;
use viewer_layout::{PanelId, PanelNode};

pub(crate) struct PlaceholderPanel {
    pub(crate) id: PanelId,
    pub(crate) title: Option<String>,
    pub(crate) requested_type: String,
    pub(crate) config_version: u32,
    pub(crate) original_config: serde_json::Value,
    pub(crate) error: String,
}

impl PlaceholderPanel {
    pub(crate) fn unknown_type(node: &PanelNode) -> Self {
        Self::from_node(node, format!("Unknown panel type: {}", node.panel_type))
    }

    pub(crate) fn invalid_config(node: &PanelNode, error: String) -> Self {
        Self::from_node(node, error)
    }

    pub(crate) fn unsupported_version(node: &PanelNode, supported: u32) -> Self {
        Self::from_node(
            node,
            format!(
                "Unsupported {} config version {}; expected {supported}",
                node.panel_type, node.config_version
            ),
        )
    }

    pub(crate) fn duplicate_singleton(node: &PanelNode) -> Self {
        Self::from_node(
            node,
            format!(
                "Only one {} panel is supported by the Native renderer",
                node.panel_type
            ),
        )
    }

    fn from_node(node: &PanelNode, error: String) -> Self {
        Self {
            id: node.id.clone(),
            title: node.title.clone(),
            requested_type: node.panel_type.clone(),
            config_version: node.config_version,
            original_config: node.config.clone(),
            error,
        }
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui) -> PanelOutput {
        ui.push_id(self.id.as_str(), |ui| {
            ui.group(|ui| {
                ui.heading(
                    self.title
                        .as_deref()
                        .unwrap_or("Panel could not be created"),
                );
                ui.colored_label(egui::Color32::RED, &self.error);
                ui.label(format!(
                    "id: {} · requested type: {} · config version: {}",
                    self.id, self.requested_type, self.config_version
                ));
                ui.collapsing("Original config", |ui| {
                    ui.monospace(
                        serde_json::to_string_pretty(&self.original_config)
                            .unwrap_or_else(|_| self.original_config.to_string()),
                    );
                });
            });
        });
        PanelOutput::default()
    }
}
