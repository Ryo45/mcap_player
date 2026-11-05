//! One analysis entry point: cursor values or an explicit, independent time range.
use super::{
    NativePanel, PanelCompositionInput, PanelOutput, PlaceholderPanel,
    inspector::InspectorPanelInput,
    range_inspection::RangeInspection,
    topics::{TopicsPanel, TopicsPanelInput},
};
use viewer_core::StreamId;
use viewer_layout::PanelNode;

#[derive(Clone, Copy, Default, PartialEq)]
enum InspectionMode {
    #[default]
    Cursor,
    Range,
}

pub(crate) struct InspectionPanel {
    mode: InspectionMode,
    cursor_topic: Option<String>,
    topics: TopicsPanel,
    pub(super) range: RangeInspection,
}

impl Default for InspectionPanel {
    fn default() -> Self {
        Self {
            mode: Default::default(),
            cursor_topic: None,
            topics: Default::default(),
            range: RangeInspection::new(),
        }
    }
}

impl InspectionPanel {
    pub(super) fn create(node: &PanelNode) -> NativePanel {
        if node.config_version != 1 || node.config != serde_json::json!({}) {
            return NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                "Inspection requires version 1 and empty config".into(),
            ));
        }
        NativePanel::Inspection(Box::default())
    }

    pub(super) fn reset_for_source(&mut self) {
        self.mode = InspectionMode::Cursor;
        self.cursor_topic = None;
        self.topics.reset_for_source();
        self.range.reset_for_source();
    }

    pub(super) fn at_cursor(&mut self) {
        self.mode = InspectionMode::Cursor;
    }

    pub(super) fn select_numeric_field(&mut self, stream: StreamId, path: String) {
        self.mode = InspectionMode::Range;
        self.range.select_numeric_field(stream, path);
    }

    pub(super) fn cancel(&mut self) {
        self.range.cancel();
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        input: &PanelCompositionInput<'_>,
    ) -> PanelOutput {
        let selection = self.show_navigation(ui, input.inspector_topic);
        let mut output = match self.mode {
            InspectionMode::Cursor => self.topics.show(
                ui,
                TopicsPanelInput {
                    tree: input.topic_tree,
                    selected_topic: input.inspector_topic,
                    inspector: InspectorPanelInput {
                        inspections: input.inspections,
                        cursor: input.playback.map(|p| p.cursor),
                    },
                    presentation: input.presentation,
                },
            ),
            InspectionMode::Range => {
                egui::ScrollArea::vertical()
                    .id_salt("range-inspection")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.range.show(ui, input))
                    .inner
            }
        };
        output.actions.extend(selection);
        output
    }

    fn show_navigation(
        &mut self,
        ui: &mut egui::Ui,
        topic: Option<&str>,
    ) -> Option<crate::interaction::ViewerAction> {
        if self.mode == InspectionMode::Cursor {
            self.cursor_topic = topic.map(str::to_owned);
        }
        let previous = self.mode;
        ui.horizontal_wrapped(|ui| {
            ui.heading("Inspection");
            ui.selectable_value(&mut self.mode, InspectionMode::Cursor, "At cursor");
            ui.selectable_value(&mut self.mode, InspectionMode::Range, "Time range");
        });
        ui.separator();
        (previous != self.mode).then(|| {
            crate::interaction::ViewerAction::SelectInspectorTopic(
                if self.mode == InspectionMode::Cursor {
                    self.cursor_topic.clone()
                } else {
                    None
                },
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interaction::ViewerAction;

    #[test]
    fn tabs_suspend_cursor_queries_and_restore_the_selected_topic() {
        let context = egui::Context::default();
        let mut panel = InspectionPanel::default();
        let mut frame = |events, topic| {
            let mut action = None;
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900., 600.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        action = panel.show_navigation(ui, topic);
                    });
                },
            );
            (output, action)
        };
        let (output, _) = frame(vec![], Some("/robot/status"));
        let label_position = |output: &egui::FullOutput, label: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| {
                    if let egui::epaint::Shape::Text(text) = &shape.shape
                        && text.galley.text() == label
                    {
                        Some(text.pos + text.galley.size() * 0.5)
                    } else {
                        None
                    }
                })
                .unwrap()
        };
        let click = |pos, pressed| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ]
        };
        let pos = label_position(&output, "Time range");
        frame(click(pos, true), Some("/robot/status"));
        let (output, action) = frame(click(pos, false), Some("/robot/status"));
        assert_eq!(action, Some(ViewerAction::SelectInspectorTopic(None)));
        let pos = label_position(&output, "At cursor");
        frame(click(pos, true), None);
        let (_, action) = frame(click(pos, false), None);
        assert_eq!(
            action,
            Some(ViewerAction::SelectInspectorTopic(Some(
                "/robot/status".into()
            )))
        );
        panel.reset_for_source();
        assert!(panel.cursor_topic.is_none());
    }
}
