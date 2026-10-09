use super::{
    INSPECTOR_CONFIG_VERSION, NativePanel, PanelDataRequirements, PanelOutput, PlaceholderPanel,
};
use crate::inspection::{InspectorRequirement, TopicInspection};
use crate::interaction::ViewerAction;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use viewer_core::{
    ArrivalTime, InspectorContainer, InspectorDecodeError, InspectorMessage, InspectorPage,
    InspectorPageCursor, InspectorScalar, InspectorValue,
};
use viewer_layout::{PanelId, PanelNode};
#[derive(Clone, Copy)]
pub(crate) struct InspectorPanelInput<'a> {
    pub inspections: &'a [TopicInspection],
    pub cursor: Option<ArrivalTime>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InspectorPanelConfig {
    pub topic: String,
}
pub(crate) struct InspectorPanel {
    id: PanelId,
    title: Option<String>,
    config: InspectorPanelConfig,
    tree: InspectorTreeState,
}
impl InspectorPanel {
    pub fn create(node: &PanelNode) -> NativePanel {
        if node.config_version != INSPECTOR_CONFIG_VERSION {
            return NativePanel::Placeholder(PlaceholderPanel::unsupported_version(
                node,
                INSPECTOR_CONFIG_VERSION,
            ));
        }
        match serde_json::from_value::<InspectorPanelConfig>(node.config.clone()) {
            Ok(config) if !config.topic.trim().is_empty() => NativePanel::Inspector(Self {
                id: node.id.clone(),
                title: node.title.clone(),
                config,
                tree: InspectorTreeState::default(),
            }),
            _ => NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                "Inspector requires a non-empty topic".into(),
            )),
        }
    }
    pub fn reset_for_source(&mut self) {
        self.tree = InspectorTreeState::default();
    }
    pub fn contribute_data_requirements(&self, requirements: &mut PanelDataRequirements) {
        let requirement = InspectorRequirement {
            topic: self.config.topic.clone(),
        };
        if !requirements.inspections.contains(&requirement) {
            requirements.inspections.push(requirement);
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui, input: InspectorPanelInput<'_>) -> PanelOutput {
        ui.push_id(self.id.as_str(), |ui| {
            ui.heading(self.title.as_deref().unwrap_or("Inspector"));
            show_inspection(ui, &self.config.topic, input, &mut self.tree);
        });
        PanelOutput::default()
    }
}
struct PageState {
    cursor: Option<InspectorPageCursor>,
    loaded_cursor: Option<InspectorPageCursor>,
    history: Vec<Option<InspectorPageCursor>>,
    page: Result<InspectorPage, InspectorDecodeError>,
}
#[derive(Default)]
pub(crate) struct InspectorTreeState {
    message: Option<std::sync::Arc<InspectorMessage>>,
    pages: BTreeMap<(usize, usize), PageState>,
}
impl InspectorTreeState {
    fn reset_message(&mut self, message: &std::sync::Arc<InspectorMessage>) {
        if self
            .message
            .as_ref()
            .is_none_or(|old| !std::sync::Arc::ptr_eq(old, message))
        {
            self.message = Some(message.clone());
            self.pages.clear();
        }
    }
}
pub(crate) fn show_topic_inspector(
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
        state.message = None;
        state.pages.clear();
    }
    actions
}
fn show_inspection(
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
    ui.small(format!("Exact query at {} ns", inspection.target.0));
    if inspection.loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Updating cursor query…");
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
    if inspection.message.is_none() {
        state.message = None;
        state.pages.clear();
    }
    if let Some(message) = &inspection.message {
        state.reset_message(message);
        if ui.small_button("Reset field expansion").clicked() {
            state.pages.clear();
        }
        let mut visible = BTreeSet::new();
        let mut budget = 2048usize;
        egui::ScrollArea::vertical()
            .max_height(650.0)
            .show(ui, |ui| {
                show_container(ui, message, &message.root, state, &mut visible, &mut budget)
            });
        // Collapsed branches release their bounded decoded pages immediately.
        state.pages.retain(|id, _| visible.contains(id));
    }
}
fn show_container(
    ui: &mut egui::Ui,
    message: &InspectorMessage,
    container: &InspectorContainer,
    state: &mut InspectorTreeState,
    visible: &mut BTreeSet<(usize, usize)>,
    budget: &mut usize,
) {
    let id = container.id();
    visible.insert(id);
    if !state.pages.contains_key(&id) {
        if state.pages.len() >= 32 {
            ui.label("Expansion budget reached; collapse another field");
            return;
        }
        state.pages.insert(
            id,
            PageState {
                cursor: None,
                loaded_cursor: None,
                history: Vec::new(),
                page: message.page(container, None),
            },
        );
    }
    let cache = state.pages.get_mut(&id).unwrap();
    if let Ok(page) = &cache.page
        && container.len() > viewer_core::INSPECTOR_PAGE_FIELDS
    {
        ui.horizontal(|ui| {
            ui.small(format!(
                "{}..{} of {}",
                page.start_index,
                page.start_index + page.fields.len(),
                container.len()
            ));
            let mut change = None;
            if ui
                .add_enabled(cache.cursor.is_some(), egui::Button::new("First"))
                .clicked()
            {
                cache.history.clear();
                change = Some(None);
            }
            if ui
                .add_enabled(!cache.history.is_empty(), egui::Button::new("Previous"))
                .clicked()
            {
                change = Some(cache.history.pop().unwrap());
            }
            if let Some(next) = page.next
                && ui.small_button("Next").clicked()
            {
                if cache.history.len() == 16 {
                    cache.history.remove(0);
                }
                cache.history.push(cache.cursor);
                change = Some(Some(next));
            }
            if let Some(cursor) = change {
                cache.cursor = cursor;
            }
        });
    }
    // A page cursor changes only on explicit navigation, not on each frame.
    if cache.loaded_cursor != cache.cursor {
        cache.page = message.page(container, cache.cursor);
        cache.loaded_cursor = cache.cursor;
    }
    let page = match &cache.page {
        Ok(page) => page.clone(),
        Err(error) => {
            ui.colored_label(ui.visuals().error_fg_color, error.to_string());
            return;
        }
    };
    for field in page.fields {
        if *budget == 0 {
            ui.label("Visible field budget reached");
            break;
        }
        *budget -= 1;
        match field.value {
            InspectorValue::Scalar(value) => {
                let value = match value {
                    InspectorScalar::Bool(v) => v.to_string(),
                    InspectorScalar::Signed(v) => v.to_string(),
                    InspectorScalar::Unsigned(v) => v.to_string(),
                    InspectorScalar::Float(v) => v.to_string(),
                    InspectorScalar::Text {
                        preview,
                        bytes,
                        truncated,
                    } => format!(
                        "{preview:?}{} ({bytes} bytes)",
                        if truncated { "…" } else { "" }
                    ),
                };
                ui.horizontal(|ui| {
                    ui.monospace(format!("{}: {value}", field.name));
                    ui.weak(field.ros_type);
                });
            }
            InspectorValue::Container(child) => {
                ui.push_id(child.id(), |ui| {
                    ui.collapsing(
                        format!(
                            "{}: {} ({} {})",
                            field.name,
                            field.ros_type,
                            child.len(),
                            if child.is_array() {
                                "elements"
                            } else {
                                "fields"
                            }
                        ),
                        |ui| show_container(ui, message, &child, state, visible, budget),
                    );
                });
            }
        }
    }
}
