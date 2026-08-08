#[cfg(any(test, target_arch = "wasm32"))]
mod data_plane;
#[cfg(any(test, target_arch = "wasm32"))]
mod range_spike;
#[cfg(any(test, target_arch = "wasm32"))]
mod remote;

#[cfg(target_arch = "wasm32")]
mod range_spike_browser;
#[cfg(any(test, target_arch = "wasm32"))]
mod webgpu;

#[cfg(target_arch = "wasm32")]
mod browser {
    use crate::remote::{RemotePlayback, WebPlayback};
    use crate::webgpu::WebGpuHost;
    use bev_renderer::BevFrame;
    use js_sys::{Date, Uint8Array};
    use std::{
        cell::RefCell,
        collections::{BTreeMap, BTreeSet},
        time::Duration,
    };
    use viewer_core::{
        ArrivalTime, BevFrameBuilder, CameraCalibrationSet, CameraId, DiagnosticsPresentation,
        McapPlayback, PlaybackCommand, PlaybackLoadState, PlaybackSpeed, PresentationMetrics,
        ViewerPresentation,
    };
    use viewer_renderer::{
        CameraBaseImageTracker, CameraOverlaySnapshot, CameraOverlayState, DecodedImage,
        decode_camera_frame,
    };
    use wasm_bindgen::{Clamped, JsCast, closure::Closure, prelude::*};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
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
    }

    impl WebViewState {
        fn reset_for_source(&mut self) {
            *self = Self::default();
        }

        fn apply_playback_effect(&mut self, effect: viewer_core::PlaybackEffect) {
            if effect == viewer_core::PlaybackEffect::Seeked {
                *self = Self::default();
            }
        }
    }

    struct WebApp {
        playback: Option<WebPlayback>,
        view: WebViewState,
        focused_camera: Option<CameraId>,
        calibrations: CameraCalibrationSet,
        previous_ms: f64,
    }

    enum WebGpuState {
        Initializing,
        Ready(WebGpuHost),
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
    }); }
    thread_local! { static WEBGPU: RefCell<WebGpuState> = const {
        RefCell::new(WebGpuState::Initializing)
    }; }

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
                    WEBGPU.with(|state| *state.borrow_mut() = WebGpuState::Ready(host));
                    set_bev_status("WebGPU: shared BevRenderer ready", false);
                }
                Err(error) => {
                    WEBGPU.with(|state| *state.borrow_mut() = WebGpuState::Unavailable);
                    set_bev_status(&format!("WebGPU unavailable: {error}"), true);
                }
            }
        });
    }

    fn render_bev(frame: BevFrame<'_>) {
        let error = WEBGPU.with(|state| {
            let mut state = state.borrow_mut();
            match &mut *state {
                WebGpuState::Ready(host) => host.render(frame).err(),
                WebGpuState::Initializing | WebGpuState::Unavailable => None,
            }
        });
        if let Some(error) = error {
            set_bev_status(&format!("WebGPU BEV stopped: {error}"), true);
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

            let selected = *camera_id;
            let callback = Closure::<dyn FnMut(Event)>::new(move |_| {
                APP.with(|app| app.borrow_mut().focused_camera = Some(selected));
            });
            button
                .add_event_listener_with_callback("click", callback.as_ref().unchecked_ref())
                .expect("camera card listener");
            callback.forget();
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
            spawn_local(async move {
                set_status("Reading MCAP…", false);
                let result = async {
                    let buffer = JsFuture::from(file.array_buffer())
                        .await
                        .map_err(|_| "File read failed".to_owned())?;
                    let bytes = Uint8Array::new(&buffer).to_vec();
                    McapPlayback::new(bytes, TOPIC)
                        .map(WebPlayback::Local)
                        .map_err(|error| error.to_string())
                }
                .await;
                match result {
                    Ok(playback) => {
                        APP.with(|cell| {
                            let mut app = cell.borrow_mut();
                            app.playback = Some(playback);
                            app.view.reset_for_source();
                            app.focused_camera = None;
                        });
                        let timeline: HtmlInputElement = element("timeline");
                        timeline.set_disabled(false);
                        let speed: HtmlSelectElement = element("speed");
                        speed.set_disabled(false);
                        set_status(&format!("{file_name} ready"), false);
                    }
                    Err(error) => set_status(&error, true),
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
                if let Some(playback) = &mut app.borrow_mut().playback {
                    if let Err(error) = playback.apply_command(PlaybackCommand::SetSpeed(speed)) {
                        set_status(&error.to_string(), true);
                    }
                }
            });
        });
        speed
            .add_event_listener_with_callback("change", speed_callback.as_ref().unchecked_ref())
            .expect("speed listener");
        speed_callback.forget();
    }

    fn advance_source_and_playback(
        session: &mut WebPlayback,
        focused_camera: &mut Option<CameraId>,
        elapsed: Duration,
    ) -> Result<Option<CameraId>, String> {
        let camera_topics = session.camera_topics();
        let selected_camera = (*focused_camera)
            .filter(|camera_id| camera_topics.iter().any(|(id, _)| id == camera_id))
            .or_else(|| camera_topics.first().map(|(id, _)| *id));
        *focused_camera = selected_camera;
        session.set_focused_camera(selected_camera);
        session.tick(elapsed).map_err(|error| error.to_string())?;
        Ok(selected_camera)
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

        let state = session.state();
        let mut camera_visual_changes = BTreeSet::new();
        for (camera_id, frame) in state.camera.frames() {
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
                    state.bev.latest(),
                    state.bev.revision(),
                    &state.transforms,
                    state.transforms.revision(),
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
            state.camera.latest_for(camera_id).is_some_and(|frame| {
                view.camera_base_images.arrival(camera_id) == Some(frame.arrival_time)
            })
        });
        if !selected_has_base && view.last_drawn.is_some() {
            clear_canvas("camera");
            view.last_drawn = None;
        }
        if let Some(camera_id) = selected_camera
            && let Some(frame) = state.camera.latest_for(camera_id)
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
        if state.camera.frames().next().is_none() {
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
        let state = session.state();
        let start = session.clock().start().0;
        let overlay_status = view
            .camera_overlays
            .snapshots()
            .map(|snapshot| (snapshot.camera_id, snapshot.status.clone()))
            .collect::<BTreeMap<_, _>>();
        ViewerPresentation::from_domain(
            state,
            session.camera_topics(),
            selected_camera,
            &overlay_status,
            DiagnosticsPresentation {
                source: if session.is_remote() {
                    "Recording Server".to_owned()
                } else {
                    "Browser file".to_owned()
                },
                primary_topic: session
                    .camera_topics()
                    .first()
                    .map_or_else(|| TOPIC.to_owned(), |(_, topic)| topic.clone()),
                counters: session.counters(),
                playback_performance: Some(session.performance().clone()),
                performance: view.presentation_metrics.snapshot().clone(),
                cursor_seconds: Some((session.clock().cursor().0 - start) as f64 / 1e9),
                ..DiagnosticsPresentation::default()
            },
        )
    }

    fn update_dom_diagnostics(presentation: &ViewerPresentation, session: &WebPlayback) {
        let focus_label: HtmlElement = element("camera-focus-label");
        focus_label.set_inner_text(&presentation.focused_camera().map_or_else(
            || "waiting".to_owned(),
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
        let remote = session.remote_diagnostics().map_or_else(String::new, |metrics| {
            format!(
                " · Remote {} loads/{} reads · {:.1} MB rx · {} windows/{:.1} MB RAM · ahead {:.2}s · {} evicted · last {:.1} ms · buffering {} · stale {}",
                metrics.load_requests,
                metrics.source_reads,
                metrics.source_bytes as f64 / (1024.0 * 1024.0),
                metrics.window_count,
                metrics.resident_bytes as f64 / (1024.0 * 1024.0),
                metrics.buffer_ahead.as_secs_f64(),
                metrics.eviction_count,
                metrics.last_window_latency_ms,
                metrics.buffering_count,
                metrics.stale_results_discarded,
            )
        });
        performance.set_inner_text(&format!(
            "Focus {focused_fps:.1}/{:.0} Hz · others ≤{:.0} Hz · JPEG {:.2} ms · canvas {:.2} ms · tick {:.2} ms · source/CDR/state {:.2}/{:.2}/{:.2} ms{remote}",
            playback_performance.focused_camera_hz(),
            playback_performance.background_camera_hz(),
            diagnostics.performance.jpeg_decode_ms,
            diagnostics.performance.upload_ms,
            diagnostics.performance.render_ms,
            playback_performance.source_read.average_ms,
            playback_performance.pipeline_decode.average_ms,
            playback_performance.state_apply.average_ms,
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

    fn update_bev_presentation(session: &WebPlayback) {
        let bev = BevFrameBuilder::new(session.state()).build();
        render_bev(BevFrame {
            revision: bev.revision,
            path: bev.path,
        });
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
                previous_ms: _,
            } = &mut *app;
            let Some(session) = playback else {
                render_bev(BevFrame {
                    revision: u64::MAX,
                    path: &[],
                });
                return;
            };
            let selected_camera =
                match advance_source_and_playback(session, focused_camera, elapsed) {
                    Ok(selected_camera) => selected_camera,
                    Err(error) => {
                        set_status(&error, true);
                        return;
                    }
                };

            let start = session.clock().start().0;
            let duration = (session.clock().end().0 - start).max(1);
            let timeline: HtmlInputElement = element("timeline");
            timeline
                .set_value_as_number((session.clock().cursor().0 - start) as f64 / duration as f64);
            timeline.set_disabled(session.is_remote());
            let speed: HtmlSelectElement = element("speed");
            speed.set_disabled(session.is_remote());
            update_camera_presentation(session, view, calibrations, selected_camera);
            let presentation = build_viewer_presentation(session, view, selected_camera);
            update_dom_diagnostics(&presentation, session);
            update_bev_presentation(session);
            view.presentation_metrics
                .record_render(duration_since(tick_started));
            view.presentation_metrics.advance(elapsed);
        });
    }

    fn duration_since(started_ms: f64) -> Duration {
        Duration::from_secs_f64(((Date::now() - started_ms).max(0.0)) / 1_000.0)
    }

    pub(crate) fn install_remote_playback(playback: RemotePlayback) {
        APP.with(|cell| {
            let mut app = cell.borrow_mut();
            app.playback = Some(WebPlayback::Remote(playback));
            app.view.reset_for_source();
            app.focused_camera = None;
            app.previous_ms = Date::now();
        });
        let timeline: HtmlInputElement = element("timeline");
        timeline.set_disabled(true);
        let speed: HtmlSelectElement = element("speed");
        speed.set_value("1");
        speed.set_disabled(true);
        let play: HtmlButtonElement = element("play");
        play.set_inner_text("Play");
        set_status("Remote playback opened · buffering first window", false);
    }

    #[wasm_bindgen(start)]
    pub fn start() -> Result<(), JsValue> {
        install_file_input();
        install_controls();
        crate::remote::install();
        crate::range_spike_browser::install();
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
