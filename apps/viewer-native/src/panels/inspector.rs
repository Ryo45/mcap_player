use super::{
    INSPECTOR_CONFIG_VERSION, NativePanel, PanelDataRequirements, PanelOutput, PlaceholderPanel,
};
use crate::inspection::{InspectorRequirement, TopicInspection};
use serde::{Deserialize, Serialize};
use viewer_layout::{PanelId, PanelNode};

#[derive(Clone, Copy)]
pub(crate) struct InspectorPanelInput<'a> {
    pub(crate) inspections: &'a [TopicInspection],
}

fn default_max_messages() -> usize {
    16
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InspectorPanelConfig {
    pub(crate) topic: String,
    #[serde(default = "default_max_messages")]
    pub(crate) max_messages: usize,
}

pub(crate) struct InspectorPanel {
    id: PanelId,
    title: Option<String>,
    config: InspectorPanelConfig,
}

impl InspectorPanel {
    pub(crate) fn create(node: &PanelNode) -> NativePanel {
        if node.config_version != INSPECTOR_CONFIG_VERSION {
            return NativePanel::Placeholder(PlaceholderPanel::unsupported_version(
                node,
                INSPECTOR_CONFIG_VERSION,
            ));
        }
        match serde_json::from_value::<InspectorPanelConfig>(node.config.clone()) {
            Ok(config)
                if !config.topic.trim().is_empty() && (1..=256).contains(&config.max_messages) =>
            {
                NativePanel::Inspector(Self {
                    id: node.id.clone(),
                    title: node.title.clone(),
                    config,
                })
            }
            Ok(_) => NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                "Invalid inspector config: topic must be non-empty and maxMessages must be 1..=256"
                    .to_owned(),
            )),
            Err(error) => NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                format!("Invalid inspector config: {error}"),
            )),
        }
    }

    pub(crate) fn contribute_data_requirements(&self, requirements: &mut PanelDataRequirements) {
        let requirement = InspectorRequirement {
            topic: self.config.topic.clone(),
            max_messages: self.config.max_messages,
        };
        if !requirements.inspections.contains(&requirement) {
            requirements.inspections.push(requirement);
        }
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        input: InspectorPanelInput<'_>,
    ) -> PanelOutput {
        ui.push_id((self.id.as_str(), self.title.as_deref()), |ui| {
            ui.heading(self.title.as_deref().unwrap_or("Message Inspector"));
            ui.label(&self.config.topic);
            let Some(inspection) = input
                .inspections
                .iter()
                .find(|inspection| inspection.topic == self.config.topic)
            else {
                ui.label("Inspection unavailable");
                return;
            };
            if inspection.loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading bounded inspection…");
                });
                return;
            }
            if let Some(error) = &inspection.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
                return;
            }
            if inspection.messages.is_empty() {
                ui.label("No matching messages");
                return;
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for message in &inspection.messages {
                    ui.monospace(format!(
                        "{} ns · {} bytes",
                        message.arrival_time.0, message.payload_bytes
                    ));
                }
            });
        });
        PanelOutput::default()
    }
}
