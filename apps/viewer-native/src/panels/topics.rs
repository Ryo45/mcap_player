//! Embedded metadata browser and cursor Inspector. Selection is query demand only.
use super::{
    PanelOutput,
    inspector::{InspectorPanelInput, InspectorTreeState, show_inspection},
};
use crate::{interaction::ViewerAction, numeric_plot_loader::NumericSeriesState};
use viewer_core::{TopicTree, ViewerPresentation};

#[derive(Default)]
pub(crate) struct TopicsPanel {
    filter: String,
    tree: InspectorTreeState,
}

pub(crate) struct TopicsPanelInput<'a> {
    pub tree: &'a TopicTree,
    pub series: &'a [NumericSeriesState],
    pub selected_topic: Option<&'a str>,
    pub inspector: InspectorPanelInput<'a>,
    pub presentation: &'a ViewerPresentation,
}

impl TopicsPanel {
    pub(crate) fn reset_for_source(&mut self) {
        self.tree = InspectorTreeState::default();
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, input: TopicsPanelInput<'_>) -> PanelOutput {
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
                            crate::ui_components::source_status(ui, input.presentation);
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
                            .iter()
                            .filter(|entry| entry.topic.contains(&self.filter))
                            .count(),
                        |ui, rows| {
                            // Only visible labels are laid out, even for a large source catalog.
                            for entry in input
                                .tree
                                .entries
                                .iter()
                                .filter(|entry| entry.topic.contains(&self.filter))
                                .skip(rows.start)
                                .take(rows.len())
                            {
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
                            if let Some(entry) =
                                input.tree.entries.iter().find(|e| e.topic == topic)
                            {
                                ui.collapsing("Schema / numeric plots", |ui| {
                                    super::topic_tree::show_topic_details(
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

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{ArrivalTime, StreamId, TopicTreeEntry};

    #[test]
    fn clicking_a_filtered_topic_emits_the_existing_inspector_query_action() {
        let context = egui::Context::default();
        let mut panel = TopicsPanel {
            filter: "visible".into(),
            ..Default::default()
        };
        let entry = |name: &str| TopicTreeEntry {
            stream_id: StreamId(1),
            topic: name.into(),
            schema_name: "example/msg/Test".into(),
            schema_definition: None,
            numeric_fields: vec![],
            schema_error: None,
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
                                    series: &[],
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
