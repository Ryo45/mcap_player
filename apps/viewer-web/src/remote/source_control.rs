use super::{RemoteApiClient, RequestGeneration, adapt_catalog};
use crate::playback::WebPlayback;
use std::cell::RefCell;
use viewer_remote_protocol::CatalogResponse;
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use web_sys::{
    AbortController, Event, HtmlButtonElement, HtmlElement, HtmlInputElement, HtmlSelectElement,
};

#[derive(Default)]
struct RemoteSourceState {
    generation: RequestGeneration,
    abort: Option<AbortController>,
    client: Option<RemoteApiClient>,
    catalog: Option<CatalogResponse>,
}

thread_local! {
    static STATE: RefCell<RemoteSourceState> = RefCell::new(RemoteSourceState::default());
}

fn document() -> web_sys::Document {
    web_sys::window()
        .expect("window")
        .document()
        .expect("document")
}

fn element<T: JsCast>(id: &str) -> T {
    document()
        .get_element_by_id(id)
        .unwrap_or_else(|| panic!("missing #{id}"))
        .dyn_into()
        .unwrap_or_else(|_| panic!("wrong element type for #{id}"))
}

fn set_output(message: &str, error: bool) {
    let output: HtmlElement = element("remote-output");
    output.set_inner_text(message);
    output.set_class_name(if error { "error" } else { "" });
}

fn begin_request() -> Option<(RemoteApiClient, u64, web_sys::AbortSignal)> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let client = state.client.clone()?;
        if let Some(previous) = state.abort.take() {
            previous.abort();
        }
        let controller = AbortController::new().ok()?;
        let signal = controller.signal();
        let generation = state.generation.next();
        state.abort = Some(controller);
        Some((client, generation, signal))
    })
}

fn apply_if_current(generation: u64, apply: impl FnOnce(&mut RemoteSourceState)) -> bool {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !state.generation.is_current(generation) {
            return false;
        }
        state.abort = None;
        apply(&mut state);
        true
    })
}

fn install_connect() {
    let button: HtmlButtonElement = element("remote-connect");
    let callback = Closure::<dyn FnMut()>::new(move || {
        let input: HtmlInputElement = element("remote-server-url");
        let client = match RemoteApiClient::new(input.value()) {
            Ok(client) => client,
            Err(error) => {
                set_output(&error.to_string(), true);
                return;
            }
        };
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            if let Some(previous) = state.abort.take() {
                previous.abort();
            }
            state.client = Some(client);
            state.catalog = None;
        });
        let Some((client, generation, signal)) = begin_request() else {
            set_output("Unable to start remote request", true);
            return;
        };
        set_output("Loading recordings…", false);
        spawn_local(async move {
            match client.list_recordings(Some(&signal)).await {
                Ok(response) => {
                    let recordings = response.recordings;
                    if !apply_if_current(generation, |_| {}) {
                        return;
                    }
                    let select: HtmlSelectElement = element("remote-recording");
                    select.set_inner_html("");
                    for recording in &recordings {
                        let option: HtmlElement = document()
                            .create_element("option")
                            .expect("recording option")
                            .dyn_into()
                            .expect("recording option element");
                        option
                            .set_attribute("value", &recording.recording_id)
                            .expect("recording option value");
                        option.set_inner_text(&recording.display_name);
                        select
                            .append_child(&option)
                            .expect("recording option append");
                    }
                    set_output(
                        &format!("Connected · {} recording(s)", recordings.len()),
                        false,
                    );
                }
                Err(error) => {
                    if apply_if_current(generation, |_| {}) {
                        set_output(&error.to_string(), true);
                    }
                }
            }
        });
    });
    button.set_onclick(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
}

fn install_catalog() {
    let button: HtmlButtonElement = element("remote-load-catalog");
    let callback = Closure::<dyn FnMut()>::new(move || {
        let recording: HtmlSelectElement = element("remote-recording");
        let recording_id = recording.value();
        if recording_id.is_empty() {
            set_output("Select a recording first", true);
            return;
        }
        let Some((client, generation, signal)) = begin_request() else {
            set_output("Connect to a server first", true);
            return;
        };
        set_output("Loading catalog…", false);
        spawn_local(async move {
            match client.fetch_catalog(&recording_id, Some(&signal)).await {
                Ok(catalog) => {
                    let summary = format_catalog(&catalog);
                    if apply_if_current(generation, |state| state.catalog = Some(catalog)) {
                        set_output(&summary, false);
                    }
                }
                Err(error) => {
                    if apply_if_current(generation, |_| {}) {
                        set_output(&error.to_string(), true);
                    }
                }
            }
        });
    });
    button.set_onclick(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
}

fn install_open_playback() {
    let button: HtmlButtonElement = element("remote-open-playback");
    let callback = Closure::<dyn FnMut()>::new(move || {
        let Some((client, catalog)) = STATE.with(|state| {
            let state = state.borrow();
            Some((state.client.clone()?, state.catalog.clone()?))
        }) else {
            set_output("Connect and load a catalog first", true);
            return;
        };
        let result = adapt_catalog(&catalog)
            .map_err(|error| error.to_string())
            .and_then(|catalog| {
                WebPlayback::from_remote(client, catalog).map_err(|error| error.to_string())
            });
        match result {
            Ok(playback) => {
                crate::browser::install_remote_playback(playback);
                set_output("Remote playback opened. Cold seek is available.", false);
            }
            Err(error) => set_output(&error, true),
        }
    });
    button.set_onclick(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
}

fn format_catalog(catalog: &CatalogResponse) -> String {
    let streams = catalog
        .streams
        .iter()
        .map(|stream| format!("{} · {} · {}", stream.id, stream.topic, stream.schema_name))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "recording: {}\nrevision: {}\nrange: [{}..{})\nstreams: {}\n{}",
        catalog.recording_id,
        catalog.recording_revision,
        catalog.time_range.start_ns,
        catalog.time_range.end_ns_exclusive,
        catalog.streams.len(),
        streams,
    )
}

pub(crate) fn install() {
    install_connect();
    install_catalog();
    install_open_playback();
    let select: HtmlSelectElement = element("remote-recording");
    let callback = Closure::<dyn FnMut(Event)>::new(move |_| {
        STATE.with(|state| state.borrow_mut().catalog = None);
    });
    select
        .add_event_listener_with_callback("change", callback.as_ref().unchecked_ref())
        .expect("remote recording listener");
    callback.forget();
}
