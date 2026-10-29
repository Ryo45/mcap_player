//! Browser-only settings IO. Active playback never reads this state.
use super::{element, set_status};
use crate::workspace_bindings::BindingsSelection;
use std::cell::{Cell, RefCell};
use viewer_core::WorkspaceBindings;
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Event, HtmlButtonElement, HtmlElement, HtmlInputElement};
thread_local! {
    static SETTINGS:RefCell<BindingsSelection>=RefCell::new(BindingsSelection::default());
    static OPENING:Cell<bool>=const {Cell::new(false)};
}
pub(super) fn for_source_open() -> Result<WorkspaceBindings, String> {
    let point: HtmlInputElement = element("point-cloud-topic");
    SETTINGS.with(|settings| settings.borrow().resolve(&point.value()))
}
pub(super) fn set_opening(opening: bool) {
    OPENING.with(|state| state.set(opening));
    refresh();
}
fn refresh() {
    let result = for_source_open();
    let opening = OPENING.with(Cell::get);
    let label = SETTINGS.with(|s| s.borrow().label.clone());
    let status: HtmlElement = element("workspace-bindings-status");
    match &result {
        Ok(bindings) => {
            status.set_inner_text(&format!(
                "{label}: valid for next recording · point cloud {}",
                bindings.point_cloud_topic
            ));
            status.set_class_name("");
        }
        Err(error) => {
            status.set_inner_text(&format!("{label}: {error}"));
            status.set_class_name("error");
        }
    }
    let file: HtmlInputElement = element("mcap-file");
    file.set_disabled(opening || result.is_err());
    let remote: HtmlButtonElement = element("remote-open-playback");
    remote.set_disabled(opening || result.is_err());
    let file: HtmlInputElement = element("workspace-bindings-file");
    file.set_disabled(opening);
    let reset: HtmlButtonElement = element("workspace-bindings-reset");
    reset.set_disabled(opening);
    let point: HtmlInputElement = element("point-cloud-topic");
    point.set_disabled(opening);
}
pub(super) fn install() {
    let input: HtmlInputElement = element("workspace-bindings-file");
    let callback = Closure::<dyn FnMut(Event)>::new(|_: Event| {
        if OPENING.with(Cell::get) {
            return;
        }
        let input: HtmlInputElement = element("workspace-bindings-file");
        let Some(file) = input.files().and_then(|files| files.get(0)) else {
            return;
        };
        let generation = SETTINGS.with(|settings| settings.borrow_mut().begin(file.name()));
        refresh();
        spawn_local(async move {
            let result = async {
                if file.size() > WorkspaceBindings::MAX_JSON_BYTES as f64 {
                    return Err("workspace bindings exceed 16 KiB".into());
                }
                let buffer = JsFuture::from(file.array_buffer()).await.map_err(|error| {
                    let message = error
                        .as_string()
                        .or_else(|| {
                            js_sys::Reflect::get(&error, &"message".into())
                                .ok()
                                .and_then(|v| v.as_string())
                        })
                        .unwrap_or_else(|| "Browser could not read this file".into());
                    format!("read workspace bindings: {message}")
                })?;
                let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
                let json = std::str::from_utf8(&bytes)
                    .map_err(|e| format!("workspace bindings must be UTF-8: {e}"))?;
                WorkspaceBindings::from_json(json)
            }
            .await;
            SETTINGS.with(|settings| settings.borrow_mut().finish(generation, result));
            refresh();
        });
    });
    input.set_onchange(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    let reset: HtmlButtonElement = element("workspace-bindings-reset");
    let callback = Closure::<dyn FnMut()>::new(|| {
        if OPENING.with(Cell::get) {
            return;
        }
        SETTINGS.with(|settings| settings.borrow_mut().reset());
        let input: HtmlInputElement = element("workspace-bindings-file");
        input.set_value("");
        refresh();
    });
    reset.set_onclick(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    let point: HtmlInputElement = element("point-cloud-topic");
    let callback = Closure::<dyn FnMut(Event)>::new(|_: Event| refresh());
    point.set_oninput(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    refresh();
}
pub(super) fn report_open_error(error: &str) {
    set_status(&format!("Cannot open recording: {error}"), true);
    refresh();
}
