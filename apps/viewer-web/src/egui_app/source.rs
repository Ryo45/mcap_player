//! Browser file picking only; catalog, indexed reads and session construction stay in existing code.
use crate::{local::open_browser_mcap, playback::WebPlayback};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use web_sys::{Event, HtmlInputElement};

type OpenResult = Result<(String, WebPlayback), String>;

pub(super) struct SourcePicker {
    input: HtmlInputElement,
    generation: Rc<Cell<u64>>,
    inbox: Rc<RefCell<Option<OpenResult>>>,
    callback: Closure<dyn FnMut(Event)>,
}

impl SourcePicker {
    pub fn new(context: &egui::Context) -> Result<Self, String> {
        let input = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("mcap-file"))
            .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
            .ok_or("missing file picker")?;
        let generation = Rc::new(Cell::new(0u64));
        let inbox = Rc::new(RefCell::new(None));
        let request_generation = generation.clone();
        let results = inbox.clone();
        let context = context.clone();
        let callback = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
            let Some(input) = event
                .target()
                .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
            else {
                return;
            };
            let Some(file) = input.files().and_then(|files| files.get(0)) else {
                return;
            };
            input.set_value("");
            let name = file.name();
            let expected = request_generation.get().wrapping_add(1);
            request_generation.set(expected);
            results.borrow_mut().take();
            let generation = request_generation.clone();
            let results = results.clone();
            let context = context.clone();
            spawn_local(async move {
                let result = async {
                    let recording = open_browser_mcap(
                        file,
                        "/camera/front/image/compressed",
                        &crate::workspace_bindings::bundled(),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                    let playback = WebPlayback::from_local(recording).map_err(|e| e.to_string())?;
                    Ok((name, playback))
                }
                .await;
                if generation.get() == expected {
                    *results.borrow_mut() = Some(result);
                    context.request_repaint();
                }
            });
        });
        input
            .add_event_listener_with_callback("change", callback.as_ref().unchecked_ref())
            .map_err(|e| format!("file listener: {e:?}"))?;
        Ok(Self {
            input,
            generation,
            inbox,
            callback,
        })
    }
    pub fn open(&self) {
        self.input.click();
    }
    pub fn poll(&mut self) -> Option<OpenResult> {
        self.inbox.borrow_mut().take()
    }
}

impl Drop for SourcePicker {
    fn drop(&mut self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        let _ = self
            .input
            .remove_event_listener_with_callback("change", self.callback.as_ref().unchecked_ref());
    }
}
