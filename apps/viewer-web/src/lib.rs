#[cfg(target_arch = "wasm32")]
mod browser {
    use js_sys::{Date, Uint8Array};
    use std::{cell::RefCell, collections::BTreeMap, time::Duration};
    use viewer_core::{
        ArrivalTime, BevFrameBuilder, CameraCalibrationSet, CameraId, DiagnosticsPresentation,
        McapPlayback, OverlayStatus, PlaybackSpeed, PresentationMetrics, ViewerPresentation,
    };
    use viewer_renderer::{DecodedImage, decode_camera_frame, prepare_camera_frame};
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
        camera_arrivals: BTreeMap<CameraId, i64>,
        last_bev_revision: Option<u64>,
        last_bev_size: (u32, u32),
        camera_topics: Vec<(CameraId, String)>,
        overlay_status: BTreeMap<CameraId, OverlayStatus>,
        presentation_metrics: PresentationMetrics,
    }

    struct WebApp {
        playback: Option<McapPlayback<Vec<u8>>>,
        view: WebViewState,
        focused_camera: Option<CameraId>,
        calibrations: CameraCalibrationSet,
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

    fn draw_image(canvas_id: &str, image: &DecodedImage) {
        let canvas: HtmlCanvasElement = element(canvas_id);
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

    fn copy_canvas(source_id: &str, destination_id: &str) {
        let source: HtmlCanvasElement = element(source_id);
        let destination: HtmlCanvasElement = element(destination_id);
        if destination.width() != source.width() {
            destination.set_width(source.width());
        }
        if destination.height() != source.height() {
            destination.set_height(source.height());
        }
        let context: CanvasRenderingContext2d = destination
            .get_context("2d")
            .expect("2d query")
            .expect("2d context")
            .dyn_into()
            .expect("canvas 2d");
        context
            .draw_image_with_html_canvas_element(&source, 0.0, 0.0)
            .expect("copy camera canvas");
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
                    McapPlayback::new(bytes, TOPIC).map_err(|error| error.to_string())
                }
                .await;
                match result {
                    Ok(playback) => {
                        APP.with(|cell| {
                            let mut app = cell.borrow_mut();
                            app.playback = Some(playback);
                            app.view = WebViewState::default();
                            app.focused_camera = None;
                        });
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
                    playback.clock_mut().toggle();
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
                    playback.seek(cursor).map_err(|error| error.to_string())
                } else {
                    Ok(())
                };
                if let Err(error) = result {
                    set_status(&error, true);
                } else {
                    app.view = WebViewState::default();
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
                    playback.clock_mut().set_speed(speed);
                }
            });
        });
        speed
            .add_event_listener_with_callback("change", speed_callback.as_ref().unchecked_ref())
            .expect("speed listener");
        speed_callback.forget();
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
                return;
            };
            let camera_topics = session.camera_topics();
            let selected_camera = (*focused_camera)
                .filter(|camera_id| camera_topics.iter().any(|(id, _)| id == camera_id))
                .or_else(|| camera_topics.first().map(|(id, _)| *id));
            *focused_camera = selected_camera;
            session.set_focused_camera(selected_camera);
            if let Err(error) = session.tick(elapsed) {
                set_status(&error.to_string(), true);
                return;
            }
            let start = session.clock().start().0;
            let duration = (session.clock().end().0 - start).max(1);
            let timeline: HtmlInputElement = element("timeline");
            timeline
                .set_value_as_number((session.clock().cursor().0 - start) as f64 / duration as f64);
            let counters = session.counters();
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
            let presentation = ViewerPresentation::from_domain(
                state,
                camera_topics,
                selected_camera,
                &view.overlay_status,
                DiagnosticsPresentation {
                    source: "Browser file".to_owned(),
                    primary_topic: TOPIC.to_owned(),
                    counters,
                    playback_performance: Some(session.performance().clone()),
                    performance: view.presentation_metrics.snapshot().clone(),
                    cursor_seconds: Some((session.clock().cursor().0 - start) as f64 / 1e9),
                    ..DiagnosticsPresentation::default()
                },
            );
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
            set_status(
                &format!(
                    "{} decoded · {} errors · {} dropped · path {} pts · scan {} pts · {:.2}s",
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
            performance.set_inner_text(&format!(
                "Focus {focused_fps:.1}/{:.0} Hz · others ≤{:.0} Hz · JPEG {:.2} ms · canvas {:.2} ms · tick {:.2} ms · MCAP/CDR/state {:.2}/{:.2}/{:.2} ms",
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

            let bev_size = bev_canvas_size();
            let bev = BevFrameBuilder::new(session.state()).build();
            if view.last_bev_revision != Some(bev.revision) || view.last_bev_size != bev_size {
                draw_bev(bev.path, bev_size);
                view.last_bev_revision = Some(bev.revision);
                view.last_bev_size = bev_size;
            }
            let has_frames = state.camera.frames().next().is_some();
            let selected_has_frame = selected_camera
                .and_then(|camera_id| state.camera.latest_for(camera_id))
                .is_some();
                if !selected_has_frame && view.last_drawn.is_some() {
                clear_canvas("camera");
                view.last_drawn = None;
            }
            if !has_frames {
                for (camera_id, _) in camera_topics {
                    clear_canvas(&format!("camera-thumb-{}", camera_id.0));
                }
                view.presentation_metrics
                    .record_render(duration_since(tick_started));
                view.presentation_metrics.advance(elapsed);
                return;
            }
            for (camera_id, frame) in state.camera.frames() {
                let thumbnail_id = format!("camera-thumb-{}", camera_id.0);
                let thumbnail_changed =
                    view.camera_arrivals.get(camera_id) != Some(&frame.arrival_time.0);
                let focus_changed = Some(*camera_id) == selected_camera
                    && view.last_drawn != Some(frame.arrival_time.0);
                if !thumbnail_changed && !focus_changed {
                    continue;
                }
                let decode_started = Date::now();
                match decode_camera_frame(frame) {
                    Ok(image) => {
                        let decode_elapsed = duration_since(decode_started);
                        let upload_started = Date::now();
                        let prepared = prepare_camera_frame(
                            frame,
                            image,
                            state.bev.latest(),
                            &state.transforms,
                            calibrations,
                        );
                        view.overlay_status
                            .insert(*camera_id, prepared.overlay_status);
                        if thumbnail_changed {
                            draw_image(&thumbnail_id, &prepared.image);
                            view.camera_arrivals
                                .insert(*camera_id, prepared.arrival_time.0);
                        }
                        if focus_changed {
                            if thumbnail_changed {
                                copy_canvas(&thumbnail_id, "camera");
                            } else {
                                draw_image("camera", &prepared.image);
                            }
                            view.last_drawn = Some(prepared.arrival_time.0);
                        }
                        view.presentation_metrics.record_camera(
                            *camera_id,
                            decode_elapsed,
                            duration_since(upload_started),
                        );
                    }
                    Err(error) => set_status(&error.to_string(), true),
                }
            }
            view.presentation_metrics
                .record_render(duration_since(tick_started));
            view.presentation_metrics.advance(elapsed);
        });
    }

    fn duration_since(started_ms: f64) -> Duration {
        Duration::from_secs_f64(((Date::now() - started_ms).max(0.0)) / 1_000.0)
    }

    fn bev_canvas_size() -> (u32, u32) {
        let canvas: HtmlCanvasElement = element("bev");
        let scale = web_sys::window()
            .expect("window")
            .device_pixel_ratio()
            .clamp(1.0, 3.0);
        let width = (f64::from(canvas.client_width().max(1)) * scale)
            .round()
            .clamp(1.0, 4096.0) as u32;
        let height = (f64::from(canvas.client_height().max(1)) * scale)
            .round()
            .clamp(1.0, 4096.0) as u32;
        (width, height)
    }

    fn draw_bev(path: &[[f32; 2]], size: (u32, u32)) {
        let canvas: HtmlCanvasElement = element("bev");
        if canvas.width() != size.0 {
            canvas.set_width(size.0);
        }
        if canvas.height() != size.1 {
            canvas.set_height(size.1);
        }
        let context: CanvasRenderingContext2d = canvas
            .get_context("2d")
            .expect("2d query")
            .expect("2d context")
            .dyn_into()
            .expect("canvas 2d");
        let width = f64::from(size.0);
        let height = f64::from(size.1);
        let pixels_per_meter = (width.min(height) / 36.0).max(4.0);
        let origin = (width * 0.5, height * 0.70);
        context.set_fill_style_str("#0b1117");
        context.fill_rect(0.0, 0.0, width, height);

        let x_min = (-origin.0 / pixels_per_meter).floor() as i32;
        let x_max = ((width - origin.0) / pixels_per_meter).ceil() as i32;
        for meter in x_min..=x_max {
            context.set_stroke_style_str(if meter == 0 {
                "#6c3933"
            } else if meter % 5 == 0 {
                "#28505a"
            } else {
                "#172c34"
            });
            context.set_line_width(if meter % 5 == 0 { 1.4 } else { 0.75 });
            let x = origin.0 + f64::from(meter) * pixels_per_meter;
            context.begin_path();
            context.move_to(x, 0.0);
            context.line_to(x, height);
            context.stroke();
        }
        let y_min = (-(height - origin.1) / pixels_per_meter).floor() as i32;
        let y_max = (origin.1 / pixels_per_meter).ceil() as i32;
        for meter in y_min..=y_max {
            context.set_stroke_style_str(if meter == 0 {
                "#386c5e"
            } else if meter % 5 == 0 {
                "#28505a"
            } else {
                "#172c34"
            });
            context.set_line_width(if meter % 5 == 0 { 1.4 } else { 0.75 });
            let y = origin.1 - f64::from(meter) * pixels_per_meter;
            context.begin_path();
            context.move_to(0.0, y);
            context.line_to(width, y);
            context.stroke();
        }

        if let Some(first) = path.first() {
            context.set_stroke_style_str("#f5b829");
            context.set_line_width(4.4);
            context.set_line_join("round");
            context.set_line_cap("round");
            context.begin_path();
            context.move_to(
                origin.0 + f64::from(first[0]) * pixels_per_meter,
                origin.1 - f64::from(first[1]) * pixels_per_meter,
            );
            for point in &path[1..] {
                context.line_to(
                    origin.0 + f64::from(point[0]) * pixels_per_meter,
                    origin.1 - f64::from(point[1]) * pixels_per_meter,
                );
            }
            context.stroke();
        }

        context.set_fill_style_str("#31c5d8");
        context.fill_rect(
            origin.0 - 0.92 * pixels_per_meter,
            origin.1 - 2.1 * pixels_per_meter,
            1.84 * pixels_per_meter,
            4.2 * pixels_per_meter,
        );
        context.set_fill_style_str("#09232b");
        context.fill_rect(
            origin.0 - 0.62 * pixels_per_meter,
            origin.1 - 1.28 * pixels_per_meter,
            1.24 * pixels_per_meter,
            0.93 * pixels_per_meter,
        );
    }

    #[wasm_bindgen(start)]
    pub fn start() -> Result<(), JsValue> {
        install_file_input();
        install_controls();
        let size = bev_canvas_size();
        draw_bev(&[], size);
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
