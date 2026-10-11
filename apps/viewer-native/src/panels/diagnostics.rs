use super::{NativePanel, PanelDataRequirements, PanelOutput, PlaceholderPanel};
use crate::{interaction::ViewerAction, window_query_loader::DiagnosticsQueryState};
use serde::Deserialize;
use viewer_core::{
    ArrivalTime, DEFAULT_QUERY_WINDOW_SECONDS, DiagnosticsRequirement, MAX_QUERY_WINDOW_SECONDS,
    PlaybackCommand,
};
use viewer_layout::{PanelId, PanelNode};

#[derive(Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    topic: String,
    window_seconds: u32,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            topic: "/diagnostics".into(),
            window_seconds: DEFAULT_QUERY_WINDOW_SECONDS,
        }
    }
}
pub(crate) struct DiagnosticsPanel {
    id: PanelId,
    title: Option<String>,
    requirement: DiagnosticsRequirement,
}
impl DiagnosticsPanel {
    pub(crate) fn create(node: &PanelNode) -> NativePanel {
        if node.config_version != 1 {
            return NativePanel::Placeholder(PlaceholderPanel::unsupported_version(node, 1));
        }
        match serde_json::from_value::<Config>(node.config.clone()) {
            Ok(config)
                if !config.topic.trim().is_empty()
                    && (1..=MAX_QUERY_WINDOW_SECONDS).contains(&config.window_seconds) =>
            {
                NativePanel::Diagnostics(Self {
                    id: node.id.clone(),
                    title: node.title.clone(),
                    requirement: DiagnosticsRequirement {
                        topic: config.topic,
                        window_seconds: config.window_seconds,
                    },
                })
            }
            _ => NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                format!(
                    "Diagnostics requires a non-empty topic and windowSeconds in 1..={MAX_QUERY_WINDOW_SECONDS}"
                ),
            )),
        }
    }
    pub(crate) fn contribute_data_requirements(&self, requirements: &mut PanelDataRequirements) {
        requirements.diagnostics.insert(self.requirement.clone());
    }
    pub(crate) fn show(
        &self,
        ui: &mut egui::Ui,
        states: &[DiagnosticsQueryState],
        cursor: Option<ArrivalTime>,
    ) -> PanelOutput {
        let mut output = PanelOutput::default();
        ui.push_id(self.id.as_str(), |ui| {
            ui.heading(self.title.as_deref().unwrap_or("Diagnostics"));
            ui.label(format!(
                "Producer-reported levels · {} · {} s window",
                self.requirement.topic, self.requirement.window_seconds
            ));
            let Some(cursor) = cursor else {
                ui.label("Available for recordings");
                return;
            };
            let Some(state) = states
                .iter()
                .find(|s| s.requirement == self.requirement && s.cursor <= cursor)
            else {
                ui.label("Loading diagnostics…");
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
            let Some(snapshot) = &state.snapshot else {
                return;
            };
            if snapshot.malformed_messages > 0 {
                ui.label(format!(
                    "{} malformed messages skipped: {}",
                    snapshot.malformed_messages,
                    snapshot
                        .first_decode_error
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default()
                ));
            }
            if snapshot.events.is_empty() {
                ui.label("No diagnostic events in this window");
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                let window_start = cursor
                    .0
                    .saturating_sub(i64::from(self.requirement.window_seconds) * 1_000_000_000);
                for event in &snapshot.events {
                    // During a forward refresh, hide aggregates whose counts include expired
                    // occurrences. A complete query will replace them with the current counts.
                    if event.first_seen.0 < window_start || event.last_seen > cursor {
                        continue;
                    }
                    ui.push_id((event.level, &event.name, &event.message), |ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .button(format!(
                                    "{}  {}  {} × {}",
                                    event.level, event.name, event.message, event.count
                                ))
                                .on_hover_text("Seek to latest occurrence (MCAP log time)")
                                .clicked()
                            {
                                output
                                    .actions
                                    .push(ViewerAction::Playback(PlaybackCommand::Seek(
                                        event.last_seen,
                                    )));
                            }
                        });
                        ui.label(format!(
                            "Hardware: {} · latest {} ns · first {} ns",
                            event.hardware_id, event.last_seen.0, event.first_seen.0
                        ));
                        if !event.values.is_empty() {
                            ui.collapsing("Values (latest occurrence)", |ui| {
                                for value in &event.values {
                                    ui.horizontal(|ui| {
                                        ui.monospace(&value.key);
                                        ui.label(&value.value);
                                    });
                                }
                            });
                        }
                        ui.separator();
                    });
                }
            });
        });
        output
    }
}
