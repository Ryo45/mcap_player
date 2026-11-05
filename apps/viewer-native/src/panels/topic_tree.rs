//! Metadata and field Plot views with read-only, concrete inputs.
use crate::interaction::ViewerAction;
use viewer_core::TopicTree;

pub(crate) fn show_topics(ui: &mut egui::Ui, tree: &TopicTree) -> Vec<ViewerAction> {
    let mut actions = Vec::new();
    ui.separator();
    ui.heading("Topics");
    egui::ScrollArea::vertical()
        .max_height(350.0)
        .show(ui, |ui| {
            for entry in &tree.entries {
                ui.push_id(entry.stream_id.0, |ui| {
                    ui.collapsing(&entry.topic, |ui| {
                        ui.horizontal(|ui| {
                            ui.small(&entry.schema_name);
                            if ui.small_button("Inspect at cursor").clicked() {
                                actions.push(ViewerAction::SelectInspectorTopic(Some(
                                    entry.topic.clone(),
                                )));
                            }
                        });
                        show_topic_details(ui, entry, &mut actions);
                    });
                });
            }
        });
    actions
}

pub(super) fn show_topic_details(
    ui: &mut egui::Ui,
    entry: &viewer_core::TopicTreeEntry,
    actions: &mut Vec<ViewerAction>,
) {
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
    if let Some(error) = &entry.schema_error {
        ui.small(error.to_string());
    }
    for field in &entry.numeric_fields {
        if ui
            .button(format!("Plot {} ({})…", field.path, field.ros_type))
            .clicked()
        {
            actions.push(ViewerAction::InspectNumericField {
                stream: entry.stream_id,
                field_path: field.path.clone(),
            });
        }
    }
}
