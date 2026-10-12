//! Browser application root: event wiring and frame/source orchestration.
mod camera;
mod presentation;
mod scene;
mod source;
use crate::playback::WebPlayback;
use bev_renderer::BevFrame;
use camera::{clear_canvas, rebuild_camera_cards, update_camera_presentation};
use js_sys::Date;
use presentation::{build_viewer_presentation, update_dom_diagnostics};
use scene::{initialize_webgpu, invalidate_gpu, render_scene, update_scene_presentation};
use scene_renderer::SceneFrame;
use std::{cell::RefCell, time::Duration};
use viewer_core::{
    ArrivalTime, CameraCalibrationSet, CameraId, PlaybackCommand, PlaybackSpeed,
    PresentationMetrics,
};
use wasm_bindgen::{JsCast, closure::Closure, prelude::*};
use web_sys::{Event, HtmlButtonElement, HtmlElement, HtmlInputElement, HtmlSelectElement};

const TOPIC: &str = "/camera/front/image/compressed";
#[derive(Default)]
struct WebViewState {
    camera: camera::CameraViewState,
    presentation_metrics: PresentationMetrics,
    scene: viewer_core::ScenePresentationState,
}

impl WebViewState {
    fn reset_for_source(&mut self) {
        *self = Self::default();
        rebuild_camera_cards(&[]);
        clear_canvas("camera");
        invalidate_gpu();
    }

    fn apply_playback_effect(&mut self, effect: viewer_core::PlaybackEffect) {
        if effect == viewer_core::PlaybackEffect::Seeked {
            *self = Self::default();
            clear_canvas("camera");
            invalidate_gpu();
        }
    }
}

struct WebApp {
    playback: Option<WebPlayback>,
    view: WebViewState,
    focused_camera: Option<CameraId>,
    calibrations: CameraCalibrationSet,
    topics: crate::topics::TopicBrowser,
    previous_ms: f64,
}

thread_local! { static APP: RefCell<WebApp> = RefCell::new(WebApp {
    playback: None,
    view: WebViewState::default(),
    focused_camera: None,
    calibrations: CameraCalibrationSet::from_json(include_str!(
        "../../../config/camera_calibration.json"
    )).expect("bundled camera calibration"),
    previous_ms: Date::now(),
    topics: crate::topics::TopicBrowser::default(),
}); }

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

fn set_status(message: &str, error: bool) {
    let status: HtmlElement = element("status");
    status.set_inner_text(message);
    status.set_class_name(if error { "error" } else { "" });
}

fn install_controls() {
    let play: HtmlButtonElement = element("play");
    let play_callback = Closure::<dyn FnMut()>::new(move || {
        APP.with(|app| {
            if let Some(playback) = &mut app.borrow_mut().playback {
                if let Err(error) = playback.apply_command(PlaybackCommand::Toggle) {
                    set_status(&error.to_string(), true);
                    return;
                }
                let button: HtmlButtonElement = element("play");
                button.set_inner_text(if playback.clock().is_playing() {
                    "Pause"
                } else {
                    "Play"
                });
            }
        })
    });
    play.set_onclick(Some(play_callback.as_ref().unchecked_ref()));
    play_callback.forget();

    let timeline: HtmlInputElement = element("timeline");
    let timeline_callback = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        let value = event
            .target()
            .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
            .map(|input| input.value_as_number())
            .unwrap_or(0.0);
        APP.with(|app| {
            let mut app = app.borrow_mut();
            let result = if let Some(playback) = &mut app.playback {
                let start = playback.clock().start().0;
                let duration = playback.clock().end().0 - start;
                let cursor = ArrivalTime(start + (duration as f64 * value.clamp(0.0, 1.0)) as i64);
                playback
                    .apply_command(PlaybackCommand::Seek(cursor))
                    .map_err(|error| error.to_string())
            } else {
                Ok(viewer_core::PlaybackEffect::None)
            };
            match result {
                Ok(effect) => app.view.apply_playback_effect(effect),
                Err(error) => set_status(&error, true),
            }
        });
    });
    timeline
        .add_event_listener_with_callback("input", timeline_callback.as_ref().unchecked_ref())
        .expect("timeline listener");
    timeline_callback.forget();

    let speed: HtmlSelectElement = element("speed");
    let speed_callback = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        let value = event
            .target()
            .and_then(|target| target.dyn_into::<HtmlSelectElement>().ok())
            .map(|select| select.value())
            .unwrap_or_default();
        let speed = match value.as_str() {
            "0.25" => PlaybackSpeed::Quarter,
            "0.5" => PlaybackSpeed::Half,
            "2" => PlaybackSpeed::Double,
            _ => PlaybackSpeed::Normal,
        };
        APP.with(|app| {
            if let Some(playback) = &mut app.borrow_mut().playback
                && let Err(error) = playback.apply_command(PlaybackCommand::SetSpeed(speed))
            {
                set_status(&error.to_string(), true);
            }
        });
    });
    speed
        .add_event_listener_with_callback("change", speed_callback.as_ref().unchecked_ref())
        .expect("speed listener");
    speed_callback.forget();
}

fn install_workspace_controls() {
    let thumbnails: HtmlElement = element("camera-thumbnails");
    let callback = Closure::<dyn FnMut(Event)>::new(|event: Event| {
        let selected = event
            .target()
            .and_then(|target| target.dyn_into::<HtmlElement>().ok())
            .and_then(|target| target.closest("button[data-camera]").ok().flatten())
            .and_then(|button| button.get_attribute("data-camera"))
            .and_then(|id| id.parse::<u16>().ok());
        if let Some(selected) = selected {
            APP.with(|app| app.borrow_mut().focused_camera = Some(CameraId(selected)));
        }
    });
    thumbnails.set_onclick(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    let list: HtmlElement = element("topic-list");
    let callback = Closure::<dyn FnMut(Event)>::new(|event: Event| {
        let Some(topic) = event
            .target()
            .and_then(|target| target.dyn_into::<HtmlElement>().ok())
            .and_then(|target| target.get_attribute("data-topic"))
        else {
            return;
        };
        APP.with(|app| {
            let mut app = app.borrow_mut();
            let WebApp {
                topics, playback, ..
            } = &mut *app;
            if let Some(session) = playback {
                topics.select(topic, session);
            }
        });
    });
    list.set_onclick(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    let filter: HtmlInputElement = element("topic-filter");
    let callback = Closure::<dyn FnMut(Event)>::new(|_: Event| {
        let filter: HtmlInputElement = element("topic-filter");
        APP.with(|app| app.borrow_mut().topics.render_topics(&filter.value()));
    });
    filter.set_oninput(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    let inspect: HtmlButtonElement = element("inspect-message");
    let callback = Closure::<dyn FnMut()>::new(|| {
        APP.with(|app| {
            let mut app = app.borrow_mut();
            let WebApp {
                topics, playback, ..
            } = &mut *app;
            if let Some(session) = playback {
                topics.request(session);
            }
        })
    });
    inspect.set_onclick(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    let automatic: HtmlInputElement = element("inspect-auto");
    let callback = Closure::<dyn FnMut(Event)>::new(|_: Event| {
        let automatic: HtmlInputElement = element("inspect-auto");
        APP.with(|app| app.borrow_mut().topics.set_automatic(automatic.checked()));
    });
    automatic.set_onchange(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    let fields: HtmlElement = element("message-fields");
    let callback = Closure::<dyn FnMut(Event)>::new(|event: Event| {
        let Some(target) = event
            .target()
            .and_then(|target| target.dyn_into::<HtmlElement>().ok())
        else {
            return;
        };
        let page = target
            .get_attribute("data-expand")
            .map(|index| (index, false))
            .or_else(|| target.get_attribute("data-next").map(|index| (index, true)));
        if let Some((index, next)) =
            page.and_then(|(index, next)| index.parse::<usize>().ok().map(|index| (index, next)))
        {
            APP.with(|app| app.borrow_mut().topics.show_page(index, next));
        }
    });
    fields.set_onclick(Some(callback.as_ref().unchecked_ref()));
    callback.forget();

    scene::install_controls();
}

fn advance_source_and_playback(
    session: &mut WebPlayback,
    focused_camera: &mut Option<CameraId>,
    elapsed: Duration,
) -> Result<(Option<CameraId>, viewer_core::PlaybackEffect), String> {
    let camera_topics = session.camera_topics();
    let selected_camera = (*focused_camera)
        .filter(|camera_id| camera_topics.iter().any(|(id, _)| id == camera_id))
        .or_else(|| camera_topics.first().map(|(id, _)| *id));
    *focused_camera = selected_camera;
    session.set_focused_camera(selected_camera);
    let effect = session.tick(elapsed).map_err(|error| error.to_string())?;
    Ok((selected_camera, effect))
}

fn tick() {
    APP.with(|cell| {
        let tick_started = Date::now();
        let mut app = cell.borrow_mut();
        let now = Date::now();
        let elapsed = Duration::from_secs_f64(((now - app.previous_ms) / 1000.0).clamp(0.0, 0.25));
        app.previous_ms = now;
        let WebApp {
            playback,
            view,
            focused_camera,
            calibrations,
            topics,
            previous_ms: _,
        } = &mut *app;
        let Some(session) = playback else {
            render_scene(
                BevFrame {
                    revision: u64::MAX,
                    path: &[],
                },
                SceneFrame::default(),
            );
            return;
        };
        let (selected_camera, playback_effect) =
            match advance_source_and_playback(session, focused_camera, elapsed) {
                Ok(result) => result,
                Err(error) => {
                    set_status(&error, true);
                    return;
                }
            };
        view.apply_playback_effect(playback_effect);
        if playback_effect == viewer_core::PlaybackEffect::Seeked {
            topics.clear_message();
        }
        topics.poll(session);

        let start = session.clock().start().0;
        let duration = (session.clock().end().0 - start).max(1);
        let timeline: HtmlInputElement = element("timeline");
        timeline.set_value_as_number((session.display_cursor().0 - start) as f64 / duration as f64);
        timeline.set_disabled(false);
        let speed: HtmlSelectElement = element("speed");
        speed.set_disabled(false);
        update_camera_presentation(
            session,
            &mut view.camera,
            &mut view.presentation_metrics,
            calibrations,
            selected_camera,
        );
        let presentation = build_viewer_presentation(session, view, selected_camera);
        update_dom_diagnostics(&presentation, session);
        update_scene_presentation(session, view);
        view.presentation_metrics
            .record_render(duration_since(tick_started));
        view.presentation_metrics.advance(elapsed);
    });
}

fn duration_since(started_ms: f64) -> Duration {
    Duration::from_secs_f64(((Date::now() - started_ms).max(0.0)) / 1_000.0)
}

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    source::install_controls();
    install_controls();
    install_workspace_controls();
    initialize_webgpu();
    let callback = Closure::<dyn FnMut()>::new(tick);
    web_sys::window()
        .expect("window")
        .set_interval_with_callback_and_timeout_and_arguments_0(
            callback.as_ref().unchecked_ref(),
            16,
        )?;
    callback.forget();
    Ok(())
}
