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
use viewer_remote_protocol::{CatalogResponse, RecordingDescriptor};
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
        self.remote.poll_timeout();
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

/// A list/catalog can only be used with the client that fetched it.
pub(super) struct RemoteConnection {
    pub address: String,
    pub client: RemoteApiClient,
    pub recordings: Vec<RecordingDescriptor>,
    pub catalog: Option<CatalogResponse>,
}
const REQUEST_TIMEOUT_MS: f64 = 30_000.;

pub(super) struct RemoteSource {
    pub url: String,
    pub connection: Rc<RefCell<Option<RemoteConnection>>>,
    pub selected: String,
    pub status: Rc<RefCell<String>>,
    context: egui::Context,
    settings: Rc<RefCell<OpenSettings>>,
    generation: Rc<Cell<u64>>,
    inbox: Rc<RefCell<VecDeque<FileResult>>>,
    busy: Rc<Cell<bool>>,
    active: Rc<Cell<Option<(u64, f64)>>>,
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
            connection: Rc::new(RefCell::new(None)),
            selected: String::new(),
            status: Rc::new(RefCell::new(String::new())),
            context,
            settings,
            generation,
            inbox,
            busy,
            active: Rc::new(Cell::new(None)),
            abort: None,
        }
    }
    pub fn is_loading(&self) -> bool {
        self.active
            .get()
            .is_some_and(|(id, _)| self.generation.get() == id)
    }
    pub fn address_changed(&mut self) {
        self.cancel();
        *self.connection.borrow_mut() = None;
        self.selected.clear();
        *self.status.borrow_mut() = "Connect to the new address".into();
    }
    pub fn select(&mut self, id: String) {
        if id != self.selected {
            self.cancel();
            self.selected = id;
            if let Some(connection) = self.connection.borrow_mut().as_mut() {
                connection.catalog = None;
            }
            self.status.borrow_mut().clear();
        }
    }
    /// Invalidate only work owned by this remote request, never a newer Local/config choice.
    pub fn cancel(&mut self) {
        if let Some(abort) = self.abort.take() {
            abort.abort();
        }
        if let Some((id, _)) = self.active.take()
            && self.generation.get() == id
        {
            self.generation.set(id.wrapping_add(1));
            self.inbox.borrow_mut().clear();
            self.busy.set(false);
            *self.status.borrow_mut() = "Remote request cancelled".into();
        }
        self.context.request_repaint();
    }
    fn poll_timeout(&mut self) {
        if let Some((id, deadline)) = self.active.get()
            && (self.generation.get() != id || js_sys::Date::now() >= deadline)
        {
            let timed_out = self.generation.get() == id;
            self.cancel();
            if timed_out {
                *self.status.borrow_mut() = "Remote request timed out after 30 seconds".into();
            }
        }
    }
    fn begin(&mut self) -> Result<(u64, web_sys::AbortSignal), String> {
        let abort =
            AbortController::new().map_err(|e| format!("request abort controller: {e:?}"))?;
        let signal = abort.signal();
        self.cancel();
        self.abort = Some(abort);
        let id = self.generation.get().wrapping_add(1);
        self.generation.set(id);
        self.active
            .set(Some((id, js_sys::Date::now() + REQUEST_TIMEOUT_MS)));
        self.inbox.borrow_mut().clear();
        self.busy.set(true);
        self.context.request_repaint();
        Ok((id, signal))
    }
    fn target(&self) -> Result<(RemoteApiClient, String), String> {
        let connection = self.connection.borrow();
        let connection = connection.as_ref().ok_or("Connect to a server first")?;
        if connection.address != self.url.trim() {
            return Err("Reconnect after changing the server address".into());
        }
        if !connection
            .recordings
            .iter()
            .any(|r| r.recording_id == self.selected)
        {
            return Err("Select a recording from this connection".into());
        }
        Ok((connection.client.clone(), self.selected.clone()))
    }
    pub fn connect(&mut self) {
        let client = match RemoteApiClient::new(self.url.clone()) {
            Ok(client) => client,
            Err(e) => {
                *self.status.borrow_mut() = e.to_string();
                return;
            }
        };
        let (expected, signal) = match self.begin() {
            Ok(v) => v,
            Err(e) => {
                *self.status.borrow_mut() = e;
                return;
            }
        };
        *self.connection.borrow_mut() = None;
        self.selected.clear();
        let address = self.url.trim().to_owned();
        let generation = self.generation.clone();
        let status = self.status.clone();
        let connection = self.connection.clone();
        let context = self.context.clone();
        let busy = self.busy.clone();
        let active = self.active.clone();
        *status.borrow_mut() = "Loading recordings…".into();
        spawn_local(async move {
            let result = client.list_recordings(Some(&signal)).await;
            if generation.get() != expected {
                return;
            }
            active.set(None);
            busy.set(false);
            match result {
                Ok(response) => {
                    *status.borrow_mut() = format!("{} recording(s)", response.recordings.len());
                    *connection.borrow_mut() = Some(RemoteConnection {
                        address,
                        client,
                        recordings: response.recordings,
                        catalog: None,
                    });
                }
                Err(error) => *status.borrow_mut() = error.to_string(),
            }
            context.request_repaint();
        });
    }
    pub fn load_catalog(&mut self) {
        let (client, id) = match self.target() {
            Ok(v) => v,
            Err(e) => {
                *self.status.borrow_mut() = e;
                return;
            }
        };
        let (expected, signal) = match self.begin() {
            Ok(v) => v,
            Err(e) => {
                *self.status.borrow_mut() = e;
                return;
            }
        };
        if let Some(connection) = self.connection.borrow_mut().as_mut() {
            connection.catalog = None;
        }
        let generation = self.generation.clone();
        let status = self.status.clone();
        let connection = self.connection.clone();
        let context = self.context.clone();
        let busy = self.busy.clone();
        let active = self.active.clone();
        *status.borrow_mut() = "Loading catalog only (no message reads)…".into();
        spawn_local(async move {
            let result = client.fetch_catalog(&id, Some(&signal)).await;
            if generation.get() != expected {
                return;
            }
            active.set(None);
            busy.set(false);
            match result {
                Ok(catalog) => {
                    if let Some(connection) = connection.borrow_mut().as_mut() {
                        connection.catalog = Some(catalog);
                    }
                    *status.borrow_mut() = "Catalog ready; playback has not been opened".into();
                }
                Err(error) => *status.borrow_mut() = error.to_string(),
            }
            context.request_repaint();
        });
    }
    pub fn open(&mut self) {
        let bindings = match self.settings.borrow().resolve() {
            Ok(v) => v,
            Err(e) => {
                *self.status.borrow_mut() = e;
                return;
            }
        };
        let (client, id) = match self.target() {
            Ok(v) => v,
            Err(e) => {
                *self.status.borrow_mut() = e;
                return;
            }
        };
        let cached = self
            .connection
            .borrow()
            .as_ref()
            .and_then(|c| c.catalog.as_ref())
            .filter(|c| c.recording_id == id)
            .cloned();
        let (expected, signal) = match self.begin() {
            Ok(v) => v,
            Err(e) => {
                *self.status.borrow_mut() = e;
                return;
            }
        };
        let requirements = self.settings.borrow().requirements.clone();
        let generation = self.generation.clone();
        let inbox = self.inbox.clone();
        let context = self.context.clone();
        let busy = self.busy.clone();
        let status = self.status.clone();
        let active = self.active.clone();
        *status.borrow_mut() = "Opening recording…".into();
        spawn_local(async move {
            let result = async {
                let catalog = match cached {
                    Some(c) => c,
                    None => client
                        .fetch_catalog(&id, Some(&signal))
                        .await
                        .map_err(|e| e.to_string())?,
                };
                let adapted = adapt_catalog(&catalog, &bindings, Some(&requirements))
                    .map_err(|e| e.to_string())?;
                let playback =
                    WebPlayback::from_remote(client, adapted).map_err(|e| e.to_string())?;
                Ok(OpenedSource {
                    name: id,
                    playback,
                    bindings,
                })
            }
            .await;
            if generation.get() == expected {
                active.set(None);
                busy.set(false);
                status.borrow_mut().clear();
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
        self.cancel();
    }
}
