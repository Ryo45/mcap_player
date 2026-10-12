#[cfg(any(test, target_arch = "wasm32"))]
mod data_plane;
#[cfg(any(test, target_arch = "wasm32"))]
mod inspector_query;
#[cfg(any(test, target_arch = "wasm32"))]
mod local;
#[cfg(any(test, target_arch = "wasm32"))]
mod playback;
#[cfg(any(test, target_arch = "wasm32"))]
mod remote;
#[cfg(any(test, target_arch = "wasm32"))]
mod topics;

#[cfg(any(test, target_arch = "wasm32"))]
mod webgpu;

#[cfg(target_arch = "wasm32")]
mod browser {
    use crate::local::open_browser_mcap;
    use crate::playback::WebPlayback;
    use crate::webgpu::WebGpuHost;
    use bev_renderer::BevFrame;
    use js_sys::Date;
    use scene_renderer::{SceneCameraMode, SceneFrame};
    use std::{
        cell::{Cell, RefCell},
        collections::{BTreeMap, BTreeSet},
        time::Duration,
    };
    use viewer_core::{
        ArrivalTime, BevFrameBuilder, CameraCalibrationSet, CameraId, DiagnosticsPresentation,
        PlaybackCommand, PlaybackLoadState, PlaybackSpeed, PresentationMetrics, ViewerPresentation,
    };
    use viewer_renderer::{
        CameraBaseImageTracker, CameraOverlaySnapshot, CameraOverlayState, DecodedImage,
        decode_camera_frame,
    };
    use wasm_bindgen::{Clamped, JsCast, closure::Closure, prelude::*};
    use wasm_bindgen_futures::spawn_local;
    use web_sys::{
        CanvasRenderingContext2d, Event, HtmlButtonElement, HtmlCanvasElement, HtmlElement,
        HtmlInputElement, HtmlSelectElement, ImageData,
    };

    const TOPIC: &str = "/camera/front/image/compressed";
    #[derive(Default)]
    struct WebViewState {
        last_drawn: Option<i64>,
        last_drawn_camera: Option<CameraId>,
        camera_base_images: CameraBaseImageTracker,
        camera_base_canvases: BTreeMap<CameraId, HtmlCanvasElement>,
        camera_overlays: CameraOverlayState,
        camera_topics: Vec<(CameraId, String)>,
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

    enum WebGpuState {
        Initializing,
        Ready(Box<WebGpuHost>),
        Unavailable,
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
    thread_local! { static WEBGPU: RefCell<WebGpuState> = const {
        RefCell::new(WebGpuState::Initializing)
    }; }
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

    fn set_bev_status(message: &str, error: bool) {
        let status: HtmlElement = element("bev-status");
        status.set_inner_text(message);
        status.set_class_name(if error { "error" } else { "" });
    }

    fn initialize_webgpu() {
        let canvas: HtmlCanvasElement = element("bev");
        set_bev_status("WebGPU: initializing", false);
        spawn_local(async move {
            match WebGpuHost::new(canvas).await {
                Ok(host) => {
                    WEBGPU.with(|state| *state.borrow_mut() = WebGpuState::Ready(Box::new(host)));
                    set_bev_status("WebGPU ready", false);
                }
                Err(error) => {
                    WEBGPU.with(|state| *state.borrow_mut() = WebGpuState::Unavailable);
                    set_bev_status(&format!("WebGPU unavailable: {error}"), true);
                }
            }
        });
    }

    fn invalidate_gpu() {
        WEBGPU.with(|state| {
            if let WebGpuState::Ready(host) = &mut *state.borrow_mut() {
                host.invalidate();
            }
        });
    }

    fn render_scene(bev: BevFrame<'_>, scene: SceneFrame<'_>) {
        let mode: HtmlSelectElement = element("view-mode");
        let error = WEBGPU.with(|state| {
            let mut state = state.borrow_mut();
            match &mut *state {
                WebGpuState::Ready(host) => host.render(bev, scene, mode.value() == "bev").err(),
                WebGpuState::Initializing | WebGpuState::Unavailable => None,
            }
        });
        if let Some(error) = error {
            set_bev_status(&format!("WebGPU stopped: {error}"), true);
            WEBGPU.with(|state| *state.borrow_mut() = WebGpuState::Unavailable);
        }
    }

    fn draw_base_image(canvas: &HtmlCanvasElement, image: &DecodedImage) {
        if canvas.width() != image.width {
            canvas.set_width(image.width);
        }
        if canvas.height() != image.height {
            canvas.set_height(image.height);
        }
        let context: CanvasRenderingContext2d = canvas
            .get_context("2d")
            .expect("2d query")
            .expect("2d context")
            .dyn_into()
            .expect("canvas 2d");
        let data = ImageData::new_with_u8_clamped_array_and_sh(
            Clamped(&image.rgba),
            image.width,
            image.height,
        )
        .expect("image data");
        context
            .put_image_data(&data, 0.0, 0.0)
            .expect("draw camera");
    }

    fn compose_camera_canvas(
        destination_id: &str,
        base: &HtmlCanvasElement,
        overlay: Option<&CameraOverlaySnapshot>,
    ) {
        let destination: HtmlCanvasElement = element(destination_id);
        if destination.width() != base.width() {
            destination.set_width(base.width());
        }
        if destination.height() != base.height() {
            destination.set_height(base.height());
        }
        let context: CanvasRenderingContext2d = destination
            .get_context("2d")
            .expect("2d query")
            .expect("2d context")
            .dyn_into()
            .expect("canvas 2d");
        context
            .draw_image_with_html_canvas_element(base, 0.0, 0.0)
            .expect("draw camera base image");
        if let Some(overlay) = overlay
            && overlay.image_size == (base.width(), base.height())
        {
            draw_camera_overlay(&context, overlay);
        }
    }

    fn draw_camera_overlay(context: &CanvasRenderingContext2d, overlay: &CameraOverlaySnapshot) {
        for (color, width) in [("rgba(0,0,0,0.7)", 5.0), ("#2deba5", 2.0)] {
            context.set_stroke_style_str(color);
            context.set_line_width(width);
            for pair in overlay.projected_path.windows(2) {
                let [Some(start), Some(end)] = pair else {
                    continue;
                };
                context.begin_path();
                context.move_to(f64::from(start[0]), f64::from(start[1]));
                context.line_to(f64::from(end[0]), f64::from(end[1]));
                context.stroke();
            }
        }
    }

    fn clear_canvas(canvas_id: &str) {
        let canvas: HtmlCanvasElement = element(canvas_id);
        let context: CanvasRenderingContext2d = canvas
            .get_context("2d")
            .expect("2d query")
            .expect("2d context")
            .dyn_into()
            .expect("canvas 2d");
        context.clear_rect(
            0.0,
            0.0,
            f64::from(canvas.width()),
            f64::from(canvas.height()),
        );
    }

    fn rebuild_camera_cards(camera_topics: &[(CameraId, String)]) {
        let container: HtmlElement = element("camera-thumbnails");
        container.set_inner_html("");
        for (camera_id, topic) in camera_topics {
            let button: HtmlButtonElement = document()
                .create_element("button")
                .expect("camera card")
                .dyn_into()
                .expect("camera card button");
            let button_id = format!("camera-card-{}", camera_id.0);
            button.set_id(&button_id);
            button.set_class_name("camera-card");
            button.set_type("button");
            button
                .set_attribute("data-camera", &camera_id.0.to_string())
                .expect("camera card ID");
            button
                .set_attribute("aria-label", &format!("Focus camera {topic}"))
                .expect("camera card aria label");

            let canvas: HtmlCanvasElement = document()
                .create_element("canvas")
                .expect("camera thumbnail")
                .dyn_into()
                .expect("camera thumbnail canvas");
            canvas.set_id(&format!("camera-thumb-{}", camera_id.0));
            canvas.set_width(160);
            canvas.set_height(120);
            button
                .append_child(&canvas)
                .expect("camera thumbnail append");

            let label: HtmlElement = document()
                .create_element("span")
                .expect("camera card label")
                .dyn_into()
                .expect("camera card label element");
            label.set_id(&format!("camera-label-{}", camera_id.0));
            label.set_inner_text(topic);
            button.append_child(&label).expect("camera label append");

            container
                .append_child(&button)
                .expect("camera card container append");
        }
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
                    let cursor =
                        ArrivalTime(start + (duration as f64 * value.clamp(0.0, 1.0)) as i64);
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
            if let Some((index, next)) = page
                .and_then(|(index, next)| index.parse::<usize>().ok().map(|index| (index, next)))
            {
                APP.with(|app| app.borrow_mut().topics.show_page(index, next));
            }
        });
        fields.set_onclick(Some(callback.as_ref().unchecked_ref()));
        callback.forget();

        let canvas: HtmlCanvasElement = element("bev");
        let callback =
            Closure::<dyn FnMut(web_sys::WheelEvent)>::new(|event: web_sys::WheelEvent| {
                event.prevent_default();
                WEBGPU.with(|state| {
                    if let WebGpuState::Ready(host) = &mut *state.borrow_mut() {
                        host.scene_renderer.zoom(-event.delta_y() as f32);
                    }
                });
            });
        canvas.set_onwheel(Some(callback.as_ref().unchecked_ref()));
        callback.forget();
        let callback =
            Closure::<dyn FnMut(web_sys::MouseEvent)>::new(|event: web_sys::MouseEvent| {
                if event.buttons() & 1 == 0 {
                    return;
                }
                WEBGPU.with(|state| {
                    if let WebGpuState::Ready(host) = &mut *state.borrow_mut() {
                        host.scene_renderer
                            .orbit(event.movement_x() as f32, event.movement_y() as f32);
                    }
                });
            });
        canvas.set_onmousemove(Some(callback.as_ref().unchecked_ref()));
        callback.forget();
        let mode: HtmlSelectElement = element("scene-camera-mode");
        let callback = Closure::<dyn FnMut(Event)>::new(|_: Event| {
            let mode: HtmlSelectElement = element("scene-camera-mode");
            let mode = match mode.value().as_str() {
                "free" => SceneCameraMode::Free,
                "eye" => SceneCameraMode::VehicleEye,
                _ => SceneCameraMode::Chase,
            };
            WEBGPU.with(|state| {
                if let WebGpuState::Ready(host) = &mut *state.borrow_mut() {
                    host.scene_renderer.set_camera_mode(mode);
                }
            });
        });
        mode.set_onchange(Some(callback.as_ref().unchecked_ref()));
        callback.forget();
        let reset: HtmlButtonElement = element("reset-scene");
        let callback = Closure::<dyn FnMut()>::new(|| {
            WEBGPU.with(|state| {
                if let WebGpuState::Ready(host) = &mut *state.borrow_mut() {
                    host.scene_renderer.reset_camera();
                }
            })
        });
        reset.set_onclick(Some(callback.as_ref().unchecked_ref()));
        callback.forget();
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

    fn update_camera_presentation(
        session: &WebPlayback,
        view: &mut WebViewState,
        calibrations: &CameraCalibrationSet,
        selected_camera: Option<CameraId>,
    ) {
        let camera_topics = session.camera_topics();
        if view.camera_topics.as_slice() != camera_topics {
            rebuild_camera_cards(camera_topics);
            view.camera_topics = camera_topics.to_vec();
        }
        if view.last_drawn_camera != selected_camera {
            view.last_drawn = None;
            view.last_drawn_camera = selected_camera;
        }

        let cameras = session.cameras().state();
        let mut camera_visual_changes = BTreeSet::new();
        for (camera_id, frame) in cameras.frames() {
            if view.camera_base_images.needs_update(frame) {
                let decode_started = Date::now();
                match decode_camera_frame(frame) {
                    Ok(image) => {
                        let decode_elapsed = duration_since(decode_started);
                        let upload_started = Date::now();
                        let base_canvas = view
                            .camera_base_canvases
                            .entry(*camera_id)
                            .or_insert_with(|| {
                                document()
                                    .create_element("canvas")
                                    .expect("camera base canvas")
                                    .dyn_into()
                                    .expect("camera base canvas element")
                            })
                            .clone();
                        draw_base_image(&base_canvas, &image);
                        view.camera_base_images.mark_updated(frame);
                        view.presentation_metrics.record_camera(
                            *camera_id,
                            decode_elapsed,
                            duration_since(upload_started),
                        );
                        camera_visual_changes.insert(*camera_id);
                    }
                    Err(error) => {
                        set_status(&error.to_string(), true);
                        continue;
                    }
                }
            }
            if view.camera_base_images.arrival(*camera_id) == Some(frame.arrival_time) {
                let Some(base_canvas) = view.camera_base_canvases.get(camera_id) else {
                    continue;
                };
                if view.camera_overlays.update(
                    frame,
                    (base_canvas.width(), base_canvas.height()),
                    session.path().state().latest(),
                    session.path().state().revision(),
                    session.transforms().state(),
                    session.transforms().state().revision(),
                    calibrations,
                ) {
                    camera_visual_changes.insert(*camera_id);
                }
            }
        }
        for camera_id in &camera_visual_changes {
            let Some(base_canvas) = view.camera_base_canvases.get(camera_id) else {
                continue;
            };
            compose_camera_canvas(
                &format!("camera-thumb-{}", camera_id.0),
                base_canvas,
                view.camera_overlays.snapshot(*camera_id),
            );
        }

        let selected_has_base = selected_camera.is_some_and(|camera_id| {
            cameras.latest_for(camera_id).is_some_and(|frame| {
                view.camera_base_images.arrival(camera_id) == Some(frame.arrival_time)
            })
        });
        if !selected_has_base && view.last_drawn.is_some() {
            clear_canvas("camera");
            view.last_drawn = None;
        }
        if let Some(camera_id) = selected_camera
            && let Some(frame) = cameras.latest_for(camera_id)
            && selected_has_base
            && (view.last_drawn != Some(frame.arrival_time.0)
                || camera_visual_changes.contains(&camera_id))
            && let Some(base_canvas) = view.camera_base_canvases.get(&camera_id)
        {
            compose_camera_canvas(
                "camera",
                base_canvas,
                view.camera_overlays.snapshot(camera_id),
            );
            view.last_drawn = Some(frame.arrival_time.0);
        }
        if cameras.frames().next().is_none() {
            for (camera_id, _) in camera_topics {
                clear_canvas(&format!("camera-thumb-{}", camera_id.0));
            }
        }
    }

    fn build_viewer_presentation(
        session: &WebPlayback,
        view: &WebViewState,
        selected_camera: Option<CameraId>,
    ) -> ViewerPresentation {
        let start = session.clock().start().0;
        let overlay_status = view
            .camera_overlays
            .snapshots()
            .map(|snapshot| (snapshot.camera_id, snapshot.status.clone()))
            .collect::<BTreeMap<_, _>>();
        ViewerPresentation::from_features(viewer_core::ViewerPresentationInput {
            cameras: session.cameras().state(),
            telemetry: session.odometry().state(),
            path: session.path().state(),
            point_cloud: session.scene().map(|scene| scene.point_cloud()),
            camera_topics: session.camera_topics(),
            focused_camera: selected_camera,
            overlays: &overlay_status,
            diagnostics: DiagnosticsPresentation {
                source: session.source_label().to_owned(),
                primary_topic: session
                    .camera_topics()
                    .first()
                    .map_or_else(|| TOPIC.to_owned(), |(_, topic)| topic.clone()),
                counters: session.counters(),
                playback_performance: Some(session.performance()),
                performance: view.presentation_metrics.snapshot().clone(),
                cursor_seconds: Some((session.clock().cursor().0 - start) as f64 / 1e9),
                ..DiagnosticsPresentation::default()
            },
        })
    }

    fn update_dom_diagnostics(presentation: &ViewerPresentation, session: &WebPlayback) {
        let focus_label: HtmlElement = element("camera-focus-label");
        focus_label.set_inner_text(&presentation.focused_camera().map_or_else(
            || {
                if session.camera_topics().is_empty() {
                    "No camera stream in this recording".to_owned()
                } else {
                    "waiting".to_owned()
                }
            },
            |camera| format!("{} · {}", camera.topic, camera.overlay),
        ));
        for camera in &presentation.cameras {
            let camera_id = camera.id;
            let card: HtmlButtonElement = element(&format!("camera-card-{}", camera_id.0));
            card.set_class_name(if camera.focused {
                "camera-card selected"
            } else {
                "camera-card"
            });
            let label: HtmlElement = element(&format!("camera-label-{}", camera_id.0));
            label.set_inner_text(&format!("{} · {:.1} Hz", camera.topic, camera.fps));
        }
        let diagnostics = &presentation.diagnostics;
        let load = match session.load_state() {
            PlaybackLoadState::Ready => "Ready".to_owned(),
            PlaybackLoadState::Buffering {
                requested,
                committed,
            } => format!(
                "Buffering · waiting for +{:.3}s",
                requested.0.saturating_sub(committed.0) as f64 / 1e9
            ),
            PlaybackLoadState::Seeking { .. } => "Seeking".to_owned(),
            PlaybackLoadState::Failed { message } => format!("Failed: {message}"),
        };
        set_status(
            &format!(
                "{load} · {} decoded · {} errors · {} dropped · path {} pts · scan {} pts · {:.2}s",
                diagnostics.counters.decoded,
                diagnostics.counters.errors,
                diagnostics.counters.dropped,
                diagnostics.path_points,
                diagnostics.scan_points,
                diagnostics.cursor_seconds.unwrap_or_default()
            ),
            diagnostics.counters.errors > 0,
        );
        let playback_performance = diagnostics
            .playback_performance
            .as_ref()
            .expect("MCAP playback has performance diagnostics");
        let focused_fps = presentation
            .focused_camera()
            .map_or(0.0, |camera| camera.fps);
        let performance: HtmlElement = element("performance");
        let metrics = session.data_plane_diagnostics();
        let retention = metrics
            .retention_ratio
            .map_or_else(|| "n/a".to_owned(), |ratio| format!("{ratio:.2}x"));
        let data_plane = format!(
            " · {} {} loads/{} reads · {:.1} MB loaded/{:.1} MB decompressed · {} windows/{:.1} MB logical/{:.1} MB RAM ({retention}) · copied {:.1} MB · ahead {:.2}/{:.2}s actual/target · {} evicted · last {:.1} ms (MCAP {:.1} ms) · underruns {} · stale {}",
            session.source_label(),
            metrics.load_requests,
            metrics.source_reads,
            metrics.source_bytes as f64 / (1024.0 * 1024.0),
            metrics.decompressed_bytes as f64 / (1024.0 * 1024.0),
            metrics.window_count,
            metrics.logical_payload_bytes as f64 / (1024.0 * 1024.0),
            metrics.resident_bytes as f64 / (1024.0 * 1024.0),
            metrics.per_message_copied_bytes as f64 / (1024.0 * 1024.0),
            metrics.actual_buffer_ahead.as_secs_f64(),
            metrics.target_ahead.as_secs_f64(),
            metrics.eviction_count,
            metrics.last_window_latency_ms,
            metrics.last_processing_ms,
            metrics.buffer_underrun_count,
            metrics.stale_results_discarded,
        );
        performance.set_inner_text(&format!(
            "Focus {focused_fps:.1}/{:.0} Hz · others ≤{:.0} Hz · JPEG {:.2} ms · canvas {:.2} ms · tick {:.2} ms · source/controller {:.2}/{:.2} ms{data_plane}",
            playback_performance.focused_camera_hz(),
            playback_performance.background_camera_hz(),
            diagnostics.performance.jpeg_decode_ms,
            diagnostics.performance.upload_ms,
            diagnostics.performance.render_ms,
            playback_performance.source_read.average_ms,
            playback_performance.message_processing.average_ms,
        ));
        let telemetry: HtmlElement = element("telemetry");
        telemetry.set_inner_text(&presentation.telemetry.as_ref().map_or_else(
            || "Odometry: waiting".to_owned(),
            |frame| {
                format!(
                    "x {:+.2} m · y {:+.2} m · yaw {:+.1}° · {:.2} m/s · yaw rate {:+.1}°/s",
                    frame.position_x,
                    frame.position_y,
                    frame.yaw_radians.to_degrees(),
                    frame.speed,
                    frame.yaw_rate.to_degrees()
                )
            },
        ));
    }

    fn update_scene_presentation(session: &WebPlayback, view: &mut WebViewState) {
        let bev = BevFrameBuilder::new(session.path().state()).build();
        let empty = viewer_core::PointCloudState::default();
        let points = session.scene().map_or(&empty, |scene| scene.point_cloud());
        let accumulate: HtmlInputElement = element("accumulate-scans");
        let scene = view.scene.build(
            session.path().state(),
            session.odometry().state(),
            points,
            session.transforms().state(),
            accumulate.checked(),
        );
        let info: HtmlElement = element("scene-info");
        info.set_inner_text(&format!(
            "Scan: {} visible points · {}",
            scene.cloud.len(),
            scene.diagnostics.current_tf_error.as_ref().map_or_else(
                || scene
                    .diagnostics
                    .last_tf_route
                    .clone()
                    .unwrap_or_else(|| "TF: waiting".into()),
                |error| format!(
                    "TF missing: {} → {}",
                    error.source_frame, error.target_frame
                )
            )
        ));
        render_scene(
            BevFrame {
                revision: bev.revision,
                path: bev.path,
            },
            SceneFrame {
                revision: scene.revision,
                cloud_revision: scene.cloud_revision,
                ego_position: scene.ego_position,
                ego_yaw: scene.ego_yaw,
                path: scene.path,
                cloud: scene.cloud,
            },
        );
    }

    fn tick() {
        APP.with(|cell| {
            let tick_started = Date::now();
            let mut app = cell.borrow_mut();
            let now = Date::now();
            let elapsed =
                Duration::from_secs_f64(((now - app.previous_ms) / 1000.0).clamp(0.0, 0.25));
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
            timeline
                .set_value_as_number((session.display_cursor().0 - start) as f64 / duration as f64);
            timeline.set_disabled(false);
            let speed: HtmlSelectElement = element("speed");
            speed.set_disabled(false);
            update_camera_presentation(session, view, calibrations, selected_camera);
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

    pub(crate) fn install_remote_playback(playback: WebPlayback) {
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

    #[wasm_bindgen(start)]
    pub fn start() -> Result<(), JsValue> {
        install_file_input();
        install_controls();
        install_workspace_controls();
        crate::remote::install();
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
}

#[cfg(not(target_arch = "wasm32"))]
pub fn start() {}
