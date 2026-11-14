//! Browser boundary: file dialogs and generation-checked source/configuration reads.
use super::settings::OpenSettings;
use crate::{
    local::open_browser_mcap,
    playback::WebPlayback,
    remote::{RemoteApiClient, adapt_catalog},
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};
use viewer_core::WorkspaceBindings;
use viewer_remote_protocol::RecordingDescriptor;
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{AbortController, Event, HtmlInputElement};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FileKind {
    Mcap,
    Bindings,
    Calibration,
    Platform,
    Layout,
}
impl FileKind {
    fn id(self) -> &'static str {
        match self {
            Self::Mcap => "mcap-file",
            Self::Bindings => "bindings-file",
            Self::Calibration => "calibration-file",
            Self::Platform => "platform-file",
            Self::Layout => "layout-file",
        }
    }
}
pub(super) struct OpenedSource {
    pub name: String,
    pub playback: WebPlayback,
    pub bindings: WorkspaceBindings,
}
pub(super) enum FileResult {
    Source(Result<Box<OpenedSource>, String>),
    Json(FileKind, String, Result<String, String>),
}
pub(super) struct SourcePicker {
    inputs: Vec<(FileKind, HtmlInputElement)>,
    callbacks: Vec<Closure<dyn FnMut(Event)>>,
    generation: Rc<Cell<u64>>,
    inbox: Rc<RefCell<VecDeque<FileResult>>>,
    pub settings: Rc<RefCell<OpenSettings>>,
    pub busy: Rc<Cell<bool>>,
    pub remote: RemoteSource,
}
impl SourcePicker {
    pub fn new(context: &egui::Context) -> Result<Self, String> {
        let settings = Rc::new(RefCell::new(OpenSettings::default()));
        let generation = Rc::new(Cell::new(0u64));
        let inbox = Rc::new(RefCell::new(VecDeque::new()));
        let busy = Rc::new(Cell::new(false));
        let mut inputs = Vec::new();
        let mut callbacks = Vec::new();
        for kind in [
            FileKind::Mcap,
            FileKind::Bindings,
            FileKind::Calibration,
            FileKind::Platform,
            FileKind::Layout,
        ] {
            let input = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id(kind.id()))
                .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
                .ok_or_else(|| format!("missing {}", kind.id()))?;
            let generation = generation.clone();
            let inbox = inbox.clone();
            let settings = settings.clone();
            let context = context.clone();
            let busy = busy.clone();
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
                // All file/config choices invalidate older choices. JSON errors stay explicit.
                let expected = generation.get().wrapping_add(1);
                generation.set(expected);
                inbox.borrow_mut().clear();
                busy.set(true);
                context.request_repaint();
                let binding_generation = (kind == FileKind::Bindings)
                    .then(|| settings.borrow_mut().bindings.begin(name.clone()));
                let selected = settings.borrow().resolve();
                let requirements = settings.borrow().requirements.clone();
                let generation = generation.clone();
                let inbox = inbox.clone();
                let settings = settings.clone();
                let context = context.clone();
                let busy = busy.clone();
                spawn_local(async move {
                    let result = if kind == FileKind::Mcap {
                        let result = async {
                            let bindings = selected?;
                            let recording =
                                open_browser_mcap(file, "", &bindings, Some(&requirements))
                                    .await
                                    .map_err(|e| e.to_string())?;
                            Ok(OpenedSource {
                                name,
                                playback: WebPlayback::from_local(recording)
                                    .map_err(|e| e.to_string())?,
                                bindings,
                            })
                        }
                        .await;
                        FileResult::Source(result.map(Box::new))
                    } else {
                        let text = if !file.size().is_finite() || file.size() > 1024. * 1024. {
                            Err("Configuration file exceeds 1 MiB".into())
                        } else {
                            JsFuture::from(file.text())
                                .await
                                .map_err(|e| format!("read {name}: {e:?}"))
                                .and_then(|v| {
                                    v.as_string().ok_or("configuration is not text".into())
                                })
                        };
                        if generation.get() == expected
                            && let Some(id) = binding_generation
                        {
                            settings.borrow_mut().bindings.finish(
                                id,
                                text.as_ref()
                                    .map_err(Clone::clone)
                                    .and_then(|text| WorkspaceBindings::from_json(text)),
                            );
                        }
                        FileResult::Json(kind, name, text)
                    };
                    if generation.get() == expected {
                        inbox.borrow_mut().push_back(result);
                        busy.set(false);
                        context.request_repaint();
                    }
                });
            });
            input
                .add_event_listener_with_callback("change", callback.as_ref().unchecked_ref())
                .map_err(|e| format!("file listener: {e:?}"))?;
            inputs.push((kind, input));
            callbacks.push(callback);
        }
        Ok(Self {
            inputs,
            callbacks,
            generation: generation.clone(),
            inbox: inbox.clone(),
            settings: settings.clone(),
            busy: busy.clone(),
            remote: RemoteSource::new(context.clone(), settings, generation, inbox, busy),
        })
    }
    pub fn pick(&self, kind: FileKind) {
        if let Some((_, input)) = self.inputs.iter().find(|(k, _)| *k == kind) {
            input.click();
        }
    }
    pub fn open(&self) {
        self.pick(FileKind::Mcap);
    }
    pub fn poll(&mut self) -> Option<FileResult> {
        self.inbox.borrow_mut().pop_front()
    }
}
impl Drop for SourcePicker {
    fn drop(&mut self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        for ((_, input), callback) in self.inputs.iter().zip(&self.callbacks) {
            let _ = input
                .remove_event_listener_with_callback("change", callback.as_ref().unchecked_ref());
        }
    }
}

pub(super) struct RemoteSource {
    pub url: String,
    pub recordings: Rc<RefCell<Vec<RecordingDescriptor>>>,
    pub selected: String,
    pub status: Rc<RefCell<String>>,
    context: egui::Context,
    settings: Rc<RefCell<OpenSettings>>,
    generation: Rc<Cell<u64>>,
    inbox: Rc<RefCell<VecDeque<FileResult>>>,
    busy: Rc<Cell<bool>>,
    abort: Option<AbortController>,
}
impl RemoteSource {
    fn new(
        context: egui::Context,
        settings: Rc<RefCell<OpenSettings>>,
        generation: Rc<Cell<u64>>,
        inbox: Rc<RefCell<VecDeque<FileResult>>>,
        busy: Rc<Cell<bool>>,
    ) -> Self {
        Self {
            url: "http://localhost:8081".into(),
            recordings: Rc::new(RefCell::new(Vec::new())),
            selected: String::new(),
            status: Rc::new(RefCell::new(String::new())),
            context,
            settings,
            generation,
            inbox,
            busy,
            abort: None,
        }
    }
    fn begin(&mut self) -> Result<(RemoteApiClient, u64, web_sys::AbortSignal), String> {
        let client = RemoteApiClient::new(self.url.clone()).map_err(|e| e.to_string())?;
        let abort =
            AbortController::new().map_err(|e| format!("request abort controller: {e:?}"))?;
        let signal = abort.signal();
        if let Some(previous) = self.abort.replace(abort) {
            previous.abort();
        }
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        self.inbox.borrow_mut().clear();
        self.busy.set(true);
        self.context.request_repaint();
        Ok((client, generation, signal))
    }
    pub fn connect(&mut self) {
        let (client, expected, signal) = match self.begin() {
            Ok(v) => v,
            Err(e) => {
                *self.status.borrow_mut() = e;
                return;
            }
        };
        let generation = self.generation.clone();
        let status = self.status.clone();
        let recordings = self.recordings.clone();
        let context = self.context.clone();
        let busy = self.busy.clone();
        *status.borrow_mut() = "Loading recordings…".into();
        spawn_local(async move {
            let result = client.list_recordings(Some(&signal)).await;
            if generation.get() != expected {
                return;
            }
            busy.set(false);
            match result {
                Ok(response) => {
                    *status.borrow_mut() = format!("{} recording(s)", response.recordings.len());
                    *recordings.borrow_mut() = response.recordings;
                }
                Err(error) => *status.borrow_mut() = error.to_string(),
            };
            context.request_repaint();
        });
    }
    pub fn open(&mut self) {
        let bindings = match self.settings.borrow().resolve() {
            Ok(b) => b,
            Err(e) => {
                *self.status.borrow_mut() = e;
                return;
            }
        };
        if self.selected.is_empty() {
            *self.status.borrow_mut() = "Select a recording".into();
            return;
        }
        let (client, expected, signal) = match self.begin() {
            Ok(v) => v,
            Err(e) => {
                *self.status.borrow_mut() = e;
                return;
            }
        };
        let requirements = self.settings.borrow().requirements.clone();
        let id = self.selected.clone();
        let generation = self.generation.clone();
        let inbox = self.inbox.clone();
        let context = self.context.clone();
        let busy = self.busy.clone();
        let status = self.status.clone();
        *status.borrow_mut() = "Opening recording…".into();
        spawn_local(async move {
            let result = async {
                let catalog = client
                    .fetch_catalog(&id, Some(&signal))
                    .await
                    .map_err(|e| e.to_string())?;
                let name = id.clone();
                let adapted = adapt_catalog(&catalog, &bindings, Some(&requirements))
                    .map_err(|e| e.to_string())?;
                let playback =
                    WebPlayback::from_remote(client, adapted).map_err(|e| e.to_string())?;
                Ok(OpenedSource {
                    name,
                    playback,
                    bindings,
                })
            }
            .await;
            if generation.get() == expected {
                busy.set(false);
                *status.borrow_mut() = "".into();
                inbox
                    .borrow_mut()
                    .push_back(FileResult::Source(result.map(Box::new)));
                context.request_repaint();
            }
        });
    }
}
impl Drop for RemoteSource {
    fn drop(&mut self) {
        if let Some(abort) = self.abort.take() {
            abort.abort();
        }
    }
}
