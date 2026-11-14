//! Inspector panel presentation independent of query execution.
use crate::{action::ViewerAction, inspection::InspectorTreeState, panel::PanelOutput};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use viewer_core::{ArrivalTime, InspectionMetadata, InspectorMessage};
use viewer_layout::{PanelId, PanelNode};

#[derive(Clone, Copy)]
pub struct InspectionView<'a> {
    pub topic: &'a str,
    pub target: ArrivalTime,
    pub metadata: Option<InspectionMetadata>,
    pub message: Option<&'a Arc<InspectorMessage>>,
    pub error: Option<&'a dyn std::fmt::Display>,
    pub loading: bool,
}
#[derive(Clone, Copy)]
pub struct InspectorPanelInput<'a> {
    pub inspections: &'a [InspectionView<'a>],
    pub cursor: Option<ArrivalTime>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InspectorPanelConfig {
    pub topic: String,
}
pub struct InspectorPanel {
    id: PanelId,
    title: Option<String>,
    config: InspectorPanelConfig,
    tree: InspectorTreeState,
}
impl InspectorPanel {
    pub fn create(node: &PanelNode) -> Result<Self, String> {
        if node.config_version != 1 {
            return Err("Unsupported inspector config version".into());
        }
        match serde_json::from_value::<InspectorPanelConfig>(node.config.clone()) {
            Ok(config) if !config.topic.trim().is_empty() => Ok(Self {
                id: node.id.clone(),
                title: node.title.clone(),
                config,
                tree: InspectorTreeState::default(),
            }),
            _ => Err("Inspector requires a non-empty topic".into()),
        }
    }
    pub fn reset_for_source(&mut self) {
        self.tree = InspectorTreeState::default();
    }
    pub fn topic(&self) -> &str {
        &self.config.topic
    }
    pub fn show(&mut self, ui: &mut egui::Ui, input: InspectorPanelInput<'_>) -> PanelOutput {
        ui.push_id(self.id.as_str(), |ui| {
            ui.heading(self.title.as_deref().unwrap_or("Inspector"));
            show_inspection(ui, &self.config.topic, input, &mut self.tree);
        });
        PanelOutput::default()
    }
}
pub fn show_topic_inspector(
    context: &egui::Context,
    topic: Option<&str>,
    input: InspectorPanelInput<'_>,
    state: &mut InspectorTreeState,
) -> Vec<ViewerAction> {
    let mut actions = Vec::new();
    if let Some(topic) = topic {
        let mut open = true;
        egui::Window::new("Generic Inspector")
            .open(&mut open)
            .default_width(600.0)
            .default_height(500.0)
            .show(context, |ui| show_inspection(ui, topic, input, state));
        if !open {
            actions.push(ViewerAction::SelectInspectorTopic(None));
        }
    } else {
        state.clear();
    }
    actions
}
pub fn show_inspection(
    ui: &mut egui::Ui,
    topic: &str,
    input: InspectorPanelInput<'_>,
    state: &mut InspectorTreeState,
) {
    ui.label(topic);
    let Some(cursor) = input.cursor else {
        ui.label("Cursor inspection is unavailable for live input");
        return;
    };
    ui.monospace(format!("Playback cursor: {} ns", cursor.0));
    let Some(inspection) = input.inspections.iter().find(|i| i.topic == topic) else {
        ui.label("Waiting for cursor query…");
        return;
    };
    if inspection.target != cursor {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "Stale snapshot · LatestBefore at {} ns",
                inspection.target.0
            ),
        );
    } else {
        ui.small(format!("LatestBefore at {} ns", inspection.target.0));
    }
    if inspection.loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Loading LatestBefore…");
        });
    }
    if let Some(metadata) = inspection.metadata {
        if metadata.log_time > cursor {
            ui.label("Waiting for the earlier cursor query…");
            return;
        }
        egui::Grid::new("message-times")
            .num_columns(2)
            .show(ui, |ui| {
                for (name, value) in [
                    ("Log time", format!("{} ns", metadata.log_time.0)),
                    (
                        "Publish time",
                        metadata
                            .publish_time_ns
                            .map(|t| format!("{t} ns"))
                            .unwrap_or_else(|| "Unavailable".into()),
                    ),
                    (
                        "Sequence",
                        metadata
                            .sequence
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "Unavailable".into()),
                    ),
                    (
                        "Stream / payload",
                        format!(
                            "{} / {} bytes",
                            metadata.stream_id.0, metadata.payload_bytes
                        ),
                    ),
                ] {
                    ui.label(name);
                    ui.monospace(value);
                    ui.end_row();
                }
                ui.label("Header stamp");
                ui.monospace(
                    inspection
                        .message
                        .as_ref()
                        .and_then(|m| m.header_stamp)
                        .map(|stamp| {
                            stamp
                                .measurement_time()
                                .map(|t| {
                                    format!(
                                        "{} ns (sec={}, nanosec={})",
                                        t.0, stamp.seconds, stamp.nanoseconds
                                    )
                                })
                                .unwrap_or_else(|| {
                                    format!(
                                        "Invalid stamp: sec={}, nanosec={}",
                                        stamp.seconds, stamp.nanoseconds
                                    )
                                })
                        })
                        .unwrap_or_else(|| "Absent / unavailable".into()),
                );
                ui.end_row();
            });
    } else if !inspection.loading && inspection.error.is_none() {
        ui.label("No message with log time ≤ query cursor");
    }
    if let Some(error) = &inspection.error {
        ui.colored_label(ui.visuals().error_fg_color, error.to_string());
    }
    if let Some(message) = &inspection.message {
        state.show(ui, message);
    } else {
        state.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn labels(target: i64, loading: bool, error: Option<&dyn std::fmt::Display>) -> Vec<String> {
        let context = egui::Context::default();
        let views = [InspectionView {
            topic: "/scalar",
            target: ArrivalTime(target),
            metadata: None,
            message: None,
            error,
            loading,
        }];
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 600.),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show_inspection(
                        ui,
                        "/scalar",
                        InspectorPanelInput {
                            inspections: &views,
                            cursor: Some(ArrivalTime(20)),
                        },
                        &mut InspectorTreeState::default(),
                    )
                });
            },
        );
        output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }
    #[test]
    fn shared_inspector_marks_stale_follow_and_loading_exact_snapshot() {
        let stale = labels(10, true, None);
        assert!(stale.iter().any(|t| t.contains("Stale snapshot")));
        assert!(stale.iter().any(|t| t.contains("Loading LatestBefore")));
        let exact = labels(20, true, None);
        assert!(!exact.iter().any(|t| t.contains("Stale snapshot")));
        assert!(!exact.iter().any(|t| t.contains("No message with")));
        let error = labels(20, false, Some(&"read failed"));
        assert!(error.iter().any(|t| t.contains("read failed")));
        assert!(!error.iter().any(|t| t.contains("No message with")));
    }
}
