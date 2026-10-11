use super::{NativePanel, PanelDataRequirements, PanelOutput, PlaceholderPanel};
use crate::window_query_loader::TopicHealthQueryState;
use serde::Deserialize;
use viewer_core::ArrivalTime;
use viewer_layout::{PanelId, PanelNode};
use viewer_mcap::{DEFAULT_QUERY_WINDOW_SECONDS, MAX_QUERY_WINDOW_SECONDS, TopicHealthRequirement};

#[derive(Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    window_seconds: u32,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            window_seconds: DEFAULT_QUERY_WINDOW_SECONDS,
        }
    }
}
pub(crate) struct TopicHealthPanel {
    id: PanelId,
    title: Option<String>,
    requirement: TopicHealthRequirement,
}
impl TopicHealthPanel {
    pub(crate) fn create(node: &PanelNode) -> NativePanel {
        if node.config_version != 1 {
            return NativePanel::Placeholder(PlaceholderPanel::unsupported_version(node, 1));
        }
        match serde_json::from_value::<Config>(node.config.clone()) {
            Ok(config) if (1..=MAX_QUERY_WINDOW_SECONDS).contains(&config.window_seconds) => {
                NativePanel::TopicHealth(Self {
                    id: node.id.clone(),
                    title: node.title.clone(),
                    requirement: TopicHealthRequirement {
                        window_seconds: config.window_seconds,
                    },
                })
            }
            _ => NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                format!("Topic Health windowSeconds must be 1..={MAX_QUERY_WINDOW_SECONDS}"),
            )),
        }
    }
    pub(crate) fn contribute_data_requirements(&self, requirements: &mut PanelDataRequirements) {
        requirements.topic_health.insert(self.requirement);
    }
    pub(crate) fn show(
        &self,
        ui: &mut egui::Ui,
        states: &[TopicHealthQueryState],
        cursor: Option<ArrivalTime>,
    ) -> PanelOutput {
        ui.push_id(self.id.as_str(), |ui| {
            ui.heading(self.title.as_deref().unwrap_or("Topic Health"));
            ui.label(format!(
                "Transport / log-time facts · {} s window",
                self.requirement.window_seconds
            ));
            let Some(cursor) = cursor else {
                ui.label("Available for recordings");
                return;
            };
            let Some(state) = states
                .iter()
                .find(|s| s.requirement == self.requirement && s.cursor <= cursor)
            else {
                ui.label("Loading timing facts…");
                return;
            };
            if state.loading {
                ui.label(format!(
                    "Updating… last completed window ends at {} ns",
                    state.cursor.0
                ));
            }
            if let Some(error) = &state.error {
                ui.label(error.to_string());
            }
            let Some(rows) = &state.snapshot else {
                return;
            };
            egui::ScrollArea::both().show(ui, |ui| {
                egui::Grid::new("topic-health")
                    .striped(true)
                    .num_columns(8)
                    .show(ui, |ui| {
                        for header in [
                            "Topic",
                            "Type",
                            "Recording count",
                            "Window count",
                            "Rate",
                            "Age",
                            "Recent gap",
                            "Max gap",
                        ] {
                            ui.strong(header);
                        }
                        ui.end_row();
                        for row in rows.iter() {
                            ui.label(&row.topic);
                            ui.label(&row.schema);
                            ui.label(
                                row.recording_message_count
                                    .map_or_else(|| "—".into(), |v| v.to_string()),
                            );
                            if let Some(error) = &row.error {
                                ui.label(error.to_string());
                                for _ in 0..4 {
                                    ui.label("—");
                                }
                            } else {
                                ui.label(row.sample_count.to_string());
                                ui.label(
                                    row.observed_rate_hz
                                        .map_or_else(|| "—".into(), |v| format!("{v:.2} Hz")),
                                );
                                let age = row.latest_arrival.map(|last| {
                                    std::time::Duration::from_nanos(cursor.0.abs_diff(last.0))
                                });
                                ui.label(format_duration(age));
                                ui.label(format_duration(row.recent_gap));
                                ui.label(format_duration(row.max_gap));
                            }
                            ui.end_row();
                        }
                    });
            });
        });
        PanelOutput::default()
    }
}
fn format_duration(value: Option<std::time::Duration>) -> String {
    value.map_or_else(
        || "—".into(),
        |v| {
            if v.as_secs_f64() < 1.0 {
                format!("{:.1} ms", v.as_secs_f64() * 1000.0)
            } else {
                format!("{:.2} s", v.as_secs_f64())
            }
        },
    )
}
