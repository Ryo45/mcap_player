//! Embedded metadata browser and cursor Inspector. Selection is query demand only.
use crate::action::ViewerAction;
use crate::inspection::InspectorTreeState;
use crate::{
    inspector::{InspectorPanelInput, show_inspection},
    panel::PanelOutput,
};
use viewer_core::{TopicTree, ViewerPresentation};

#[derive(Default)]
pub struct TopicsPanel {
    filter: String,
    tree: InspectorTreeState,
}

pub struct TopicsPanelInput<'a> {
    pub tree: &'a TopicTree,
    pub series: Option<&'a [(viewer_core::StreamId, &'a str)]>,
    pub selected_topic: Option<&'a str>,
    pub inspector: InspectorPanelInput<'a>,
    pub presentation: &'a ViewerPresentation,
}

impl TopicsPanel {
    pub fn reset_for_source(&mut self) {
        self.tree = InspectorTreeState::default();
    }

    pub fn show(&mut self, ui: &mut egui::Ui, input: TopicsPanelInput<'_>) -> PanelOutput {
        let mut output = PanelOutput::default();
        // Fixed regions keep a large schema or field tree from displacing the other half.
        let rect = ui.available_rect_before_wrap();
        let split = rect.top() + rect.height() * 0.45;
        let upper = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.right(), (split - 2.0).max(rect.top())),
        );
        let lower = egui::Rect::from_min_max(
            egui::pos2(rect.left(), (split + 2.0).min(rect.bottom())),
            rect.max,
        );
        ui.scope_builder(
            egui::UiBuilder::new().id_salt("topics").max_rect(upper),
            |ui| {
                ui.set_clip_rect(upper);
                ui.collapsing("Source / diagnostics", |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(100.0)
                        .show(ui, |ui| {
                            crate::source_status::source_status(ui, input.presentation);
                        });
                });
                ui.heading("Topics");
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter topics"));
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show_rows(
                        ui,
                        ui.spacing().interact_size.y,
                        input
                            .tree
                            .entries
                            .chunk_by(|a, b| a.topic == b.topic)
                            .filter(|entries| topic_matches(&entries[0].topic, &self.filter))
                            .count(),
                        |ui, rows| {
                            // Only visible labels are laid out, even for a large source catalog.
                            for entries in input
                                .tree
                                .entries
                                .chunk_by(|a, b| a.topic == b.topic)
                                .filter(|entries| topic_matches(&entries[0].topic, &self.filter))
                                .skip(rows.start)
                                .take(rows.len())
                            {
                                let entry = &entries[0];
                                if ui
                                    .selectable_label(
                                        input.selected_topic == Some(entry.topic.as_str()),
                                        &entry.topic,
                                    )
                                    .on_hover_text(&entry.schema_name)
                                    .clicked()
                                {
                                    output.actions.push(ViewerAction::SelectInspectorTopic(Some(
                                        entry.topic.clone(),
                                    )));
                                }
                            }
                        },
                    );
            },
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt("selected-topic")
                .max_rect(lower),
            |ui| {
                ui.set_clip_rect(lower);
                ui.heading("Selected topic");
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(topic) = input.selected_topic {
                            show_inspection(ui, topic, input.inspector, &mut self.tree);
                            let displayed = input
                                .inspector
                                .inspections
                                .iter()
                                .find(|i| i.topic == topic)
                                .and_then(|i| i.metadata)
                                .filter(|m| {
                                    input
                                        .inspector
                                        .cursor
                                        .is_some_and(|cursor| m.log_time <= cursor)
                                })
                                .map(|m| m.stream_id);
                            for entry in input.tree.entries.iter().filter(|e| e.topic == topic) {
                                ui.push_id(entry.stream_id.0, |ui| {
                                    if displayed == Some(entry.stream_id) {
                                        ui.small("Channel of the displayed message");
                                    }
                                    show_topic_details(
                                        ui,
                                        entry,
                                        input.series,
                                        &mut output.actions,
                                    );
                                });
                            }
                        } else {
                            self.tree = InspectorTreeState::default();
                            ui.label("Select a topic above to inspect its values.");
                        }
                    });
            },
        );
        ui.allocate_rect(rect, egui::Sense::hover());
        output
    }
}

pub fn show_topic_details(
    ui: &mut egui::Ui,
    entry: &viewer_core::TopicTreeEntry,
    series: Option<&[(viewer_core::StreamId, &str)]>,
    actions: &mut Vec<ViewerAction>,
) {
    ui.small(format!(
        "Channel {} · {} · {} · {} messages",
        entry.stream_id.0,
        entry.schema_name,
        entry.message_encoding,
        entry
            .message_count
            .map_or_else(|| "unknown".into(), |n| n.to_string())
    ));
    if let Some(definition) = &entry.schema_definition {
        ui.collapsing("Schema definition", |ui| {
            ui.small(&definition.encoding);
            if let Ok(text) = std::str::from_utf8(&definition.data) {
                ui.monospace(text);
            } else {
                ui.label(format!("{} schema bytes", definition.data.len()));
            }
        });
    }
    let Some(series) = series else {
        return;
    };
    let fields = match entry.numeric_fields() {
        Ok(fields) => fields,
        Err(error) => {
            ui.small(error.to_string());
            return;
        }
    };
    for field in fields {
        let selected = series
            .iter()
            .any(|s| s.0 == entry.stream_id && s.1 == field.path);
        if ui
            .add_enabled(
                !selected && series.len() < 8,
                egui::Button::new(format!("+ {} ({})", field.path, field.ros_type)),
            )
            .clicked()
        {
            actions.push(ViewerAction::AddNumericSeries {
                stream: entry.stream_id,
                field_path: field.path.clone(),
            });
        }
    }
}

fn topic_matches(topic: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    if topic.is_ascii() && filter.is_ascii() {
        topic
            .as_bytes()
            .windows(filter.len())
            .any(|part| part.eq_ignore_ascii_case(filter.as_bytes()))
    } else {
        topic.to_lowercase().contains(&filter.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{ArrivalTime, StreamId, TopicTreeEntry};

    #[test]
    fn topic_filter_is_case_insensitive_for_ros_and_unicode_names() {
        assert!(topic_matches("/robot/ODOM", "odom"));
        assert!(topic_matches("/robot/odom", "ODOM"));
        assert!(topic_matches("/ÉTAT", "état"));
        assert!(topic_matches("/odom", ""));
        assert!(!topic_matches("/odom", "odometry"));
    }

    #[test]
    fn same_topic_channels_show_their_own_schema_encoding_and_count() {
        let context = egui::Context::default();
        let entry = |id, schema: &str, encoding: &str, count| {
            TopicTreeEntry::from_stream(&viewer_core::StreamDescriptor {
                id: StreamId(id),
                topic: "/same".into(),
                schema: schema.into(),
                message_encoding: encoding.into(),
                schema_definition: None,
                timing: viewer_core::StreamTimingSummary {
                    message_count: Some(count),
                },
            })
        };
        let tree = TopicTree {
            entries: vec![
                entry(1, "first/msg/A", "cdr", 10),
                entry(2, "second/msg/B", "json", 20),
            ],
        };
        let presentation = ViewerPresentation::default();
        let mut panel = TopicsPanel::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 1200.),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    panel.show(
                        ui,
                        TopicsPanelInput {
                            tree: &tree,
                            series: None,
                            selected_topic: Some("/same"),
                            inspector: InspectorPanelInput {
                                inspections: &[],
                                cursor: Some(ArrivalTime(0)),
                            },
                            presentation: &presentation,
                        },
                    );
                });
            },
        );
        let labels = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.text()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            labels.iter().filter(|t| **t == "/same").count(),
            2,
            "one topic row plus the Inspector heading, not one row per channel"
        );
        assert!(
            labels
                .iter()
                .any(|t| t.contains("Channel 1 · first/msg/A · cdr · 10 messages"))
        );
        assert!(
            labels
                .iter()
                .any(|t| t.contains("Channel 2 · second/msg/B · json · 20 messages"))
        );
    }

    #[test]
    fn clicking_a_filtered_topic_emits_the_existing_inspector_query_action() {
        let context = egui::Context::default();
        let mut panel = TopicsPanel {
            filter: "visible".into(),
            ..Default::default()
        };
        let entry = |name: &str| {
            TopicTreeEntry::from_stream(&viewer_core::StreamDescriptor {
                id: StreamId(1),
                topic: name.into(),
                schema: "example/msg/Test".into(),
                message_encoding: "cdr".into(),
                schema_definition: None,
                timing: viewer_core::StreamTimingSummary::default(),
            })
        };
        let tree = TopicTree {
            entries: vec![entry("/hidden"), entry("/visible")],
        };
        let presentation = ViewerPresentation::default();
        let mut frame = |events| {
            let mut actions = Vec::new();
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        actions = panel
                            .show(
                                ui,
                                TopicsPanelInput {
                                    tree: &tree,
                                    series: Some(&[]),
                                    selected_topic: None,
                                    inspector: InspectorPanelInput {
                                        inspections: &[],
                                        cursor: Some(ArrivalTime(42)),
                                    },
                                    presentation: &presentation,
                                },
                            )
                            .actions;
                    });
                },
            );
            (output, actions)
        };
        let (output, _) = frame(vec![]);
        let labels = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!labels.iter().any(|text| text.galley.text() == "/hidden"));
        let label = labels
            .iter()
            .find(|text| text.galley.text() == "/visible")
            .unwrap();
        let pos = label.pos + label.galley.size() * 0.5;
        frame(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ]);
        let (_, actions) = frame(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);
        assert!(
            matches!(actions.as_slice(), [ViewerAction::SelectInspectorTopic(Some(topic))] if topic == "/visible")
        );
    }
}
