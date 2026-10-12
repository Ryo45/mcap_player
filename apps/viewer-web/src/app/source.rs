//! Source opening and committing Local/Remote playback into the application.
mod remote;

use super::{APP, TOPIC, element, set_status};
use crate::{local::open_browser_mcap, playback::WebPlayback};
use js_sys::Date;
use std::cell::Cell;
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use web_sys::{Event, HtmlButtonElement, HtmlInputElement, HtmlSelectElement};
thread_local! { static SOURCE_GENERATION: Cell<u64> = const { Cell::new(0) }; }
fn begin_source_change() -> u64 {
    SOURCE_GENERATION.with(|generation| {
        let next = generation.get().wrapping_add(1);
        generation.set(next);
        next
    })
}

fn source_is_current(expected: u64) -> bool {
    SOURCE_GENERATION.with(|generation| generation.get() == expected)
}

fn install_file_input() {
    let input: HtmlInputElement = element("mcap-file");
    let callback = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        let input: HtmlInputElement = event
            .target()
            .and_then(|target| target.dyn_into().ok())
            .expect("file input target");
        let Some(file) = input.files().and_then(|files| files.get(0)) else {
            return;
        };
        let file_name = file.name();
        let generation = begin_source_change();
        spawn_local(async move {
            set_status("Reading MCAP Summary…", false);
            let result = match open_browser_mcap(file, TOPIC).await {
                Ok(recording) => WebPlayback::from_local(recording),
                Err(error) => Err(crate::playback::WebPlaybackError::from(error)),
            };
            if !source_is_current(generation) {
                return;
            }
            match result {
                Ok(playback) => {
                    APP.with(|cell| {
                        let mut app = cell.borrow_mut();
                        app.topics.reset(&playback);
                        app.playback = Some(playback);
                        app.view.reset_for_source();
                        app.focused_camera = None;
                    });
                    let timeline: HtmlInputElement = element("timeline");
                    timeline.set_disabled(false);
                    let speed: HtmlSelectElement = element("speed");
                    speed.set_disabled(false);
                    set_status(
                        &format!("{file_name} ready · lazy File.slice playback"),
                        false,
                    );
                }
                Err(error) => set_status(&error.to_string(), true),
            }
        });
    });
    input
        .add_event_listener_with_callback("change", callback.as_ref().unchecked_ref())
        .expect("file change listener");
    callback.forget();
}

fn install_remote_playback(playback: WebPlayback) {
    begin_source_change();
    APP.with(|cell| {
        let mut app = cell.borrow_mut();
        app.topics.reset(&playback);
        app.playback = Some(playback);
        app.view.reset_for_source();
        app.focused_camera = None;
        app.previous_ms = Date::now();
    });
    let timeline: HtmlInputElement = element("timeline");
    timeline.set_disabled(false);
    let speed: HtmlSelectElement = element("speed");
    speed.set_value("1");
    speed.set_disabled(false);
    let play: HtmlButtonElement = element("play");
    play.set_inner_text("Play");
    set_status(
        "Remote playback opened · buffering first window · seek available",
        false,
    );
}

pub(super) fn install_controls() {
    install_file_input();
    remote::install();
}
