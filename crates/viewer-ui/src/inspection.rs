use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use viewer_core::{
    InspectorContainer, InspectorDecodeError, InspectorMessage, InspectorPage, InspectorPageCursor,
    InspectorScalar, InspectorValue,
};

struct PageState {
    cursor: Option<InspectorPageCursor>,
    loaded_cursor: Option<InspectorPageCursor>,
    history: Vec<Option<InspectorPageCursor>>,
    page: Result<InspectorPage, InspectorDecodeError>,
}
#[derive(Default)]
pub struct InspectorTreeState {
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
impl InspectorTreeState {
    pub fn clear(&mut self) {
        self.message = None;
        self.pages.clear();
    }

    pub fn show(&mut self, ui: &mut egui::Ui, message: &Arc<InspectorMessage>) {
        self.reset_message(message);
        if ui.small_button("Reset field expansion").clicked() {
            self.pages.clear();
        }
        let mut visible = BTreeSet::new();
        let mut budget = 2048usize;
        egui::ScrollArea::vertical()
            .max_height(650.0)
            .show(ui, |ui| {
                show_container(ui, message, &message.root, self, &mut visible, &mut budget)
            });
        self.pages.retain(|id, _| visible.contains(id));
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
