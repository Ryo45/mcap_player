//! Metadata and field Plot views with read-only, concrete inputs.
use crate::{interaction::ViewerAction, numeric_plot_loader::NumericSeriesState};
use viewer_core::{PlaybackView, TopicTree};

pub(crate) fn show_topics(
    ui: &mut egui::Ui,
    tree: &TopicTree,
    series: &[NumericSeriesState],
) -> Vec<ViewerAction> {
    let mut actions = Vec::new();
    ui.separator();
    ui.heading("Topics");
    egui::ScrollArea::vertical()
        .max_height(350.0)
        .show(ui, |ui| {
            for entry in &tree.entries {
                ui.push_id(entry.stream_id.0, |ui| {
                    ui.collapsing(&entry.topic, |ui| {
                        ui.small(&entry.schema_name);
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
                            ui.small(error);
                        }
                        for field in &entry.numeric_fields {
                            let selected = series.iter().any(|s| {
                                s.request.stream_id == entry.stream_id
                                    && s.request.field_path == field.path
                            });
                            if ui
                                .add_enabled(
                                    !selected && series.len() < 8,
                                    egui::Button::new(format!(
                                        "+ {} ({})",
                                        field.path, field.ros_type
                                    )),
                                )
                                .clicked()
                            {
                                actions.push(ViewerAction::AddNumericSeries {
                                    stream: entry.stream_id,
                                    field_path: field.path.clone(),
                                });
                            }
                        }
                    });
                });
            }
        });
    actions
}

pub(crate) fn show_field_plots(
    context: &egui::Context,
    tree: &TopicTree,
    series: &[NumericSeriesState],
    playback: Option<PlaybackView>,
) -> Vec<ViewerAction> {
    let mut actions = Vec::new();
    if !series.is_empty() {
        egui::Window::new("Field plots")
            .default_width(560.0)
            .show(context, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(600.0)
                    .show(ui, |ui| {
                        for series in series {
                            let request = &series.request;
                            let topic = tree
                                .entries
                                .iter()
                                .find(|e| e.stream_id == request.stream_id)
                                .map(|e| e.topic.as_str())
                                .unwrap_or("");
                            ui.horizontal(|ui| {
                                ui.label(format!("{topic} · {}", request.field_path));
                                if ui.small_button("Remove").clicked() {
                                    actions.push(ViewerAction::RemoveNumericSeries {
                                        stream: request.stream_id,
                                        field_path: request.field_path.clone(),
                                    });
                                }
                            });
                            match &series.result {
                                None => {
                                    ui.spinner();
                                }
                                Some(Err(error)) => {
                                    ui.colored_label(ui.visuals().error_fg_color, error);
                                }
                                Some(Ok(overview)) => {
                                    ui.small(format!(
                                        "Overview · {} samples · {} decode errors",
                                        overview.input_sample_count, overview.decode_errors
                                    ));
                                    let points: egui_plot::PlotPoints<'_> = overview
                                        .samples
                                        .iter()
                                        .map(|s| {
                                            [
                                                viewer_core::cursor_seconds(
                                                    s.arrival_time,
                                                    request.range.start,
                                                ),
                                                s.value,
                                            ]
                                        })
                                        .collect();
                                    egui_plot::Plot::new((
                                        request.stream_id.0,
                                        &request.field_path,
                                    ))
                                    .height(170.0)
                                    .show(ui, |plot| {
                                        plot.line(egui_plot::Line::new(
                                            &request.field_path,
                                            points,
                                        ));
                                        if let Some(playback) = playback {
                                            plot.vline(egui_plot::VLine::new(
                                                "Cursor",
                                                viewer_core::cursor_seconds(
                                                    playback.cursor,
                                                    request.range.start,
                                                ),
                                            ));
                                        }
                                    });
                                }
                            }
                        }
                    });
            });
    }
    actions
}
