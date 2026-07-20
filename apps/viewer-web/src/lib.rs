#[cfg(target_arch = "wasm32")]
mod browser {
    use js_sys::{Date, Uint8Array};
    use std::{cell::RefCell, time::Duration};
    use viewer_core::{ArrivalTime, McapPlayback, PlaybackSpeed};
    use viewer_renderer::decode_jpeg;
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
        last_bev_revision: Option<u64>,
        last_bev_size: (u32, u32),
    }

    struct WebApp {
        playback: Option<McapPlayback<Vec<u8>>>,
        view: WebViewState,
        previous_ms: f64,
    }

    thread_local! { static APP: RefCell<WebApp> = RefCell::new(WebApp {
        playback: None,
        view: WebViewState::default(),
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
            let mut app = cell.borrow_mut();
            let now = Date::now();
            let elapsed =
                Duration::from_secs_f64(((now - app.previous_ms) / 1000.0).clamp(0.0, 0.25));
            app.previous_ms = now;
            let WebApp {
                playback,
                view,
                previous_ms: _,
            } = &mut *app;
            let Some(session) = playback else {
                return;
            };
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
            let state = session.state();
            let path_points = state.bev.latest().map_or(0, |frame| frame.points.len());
            let scan_points = session
                .state()
                .point_cloud
                .latest()
                .map_or(0, |frame| frame.points.len());
            set_status(
                &format!(
                    "{} decoded · {} errors · path {} pts · scan {} pts · {:.2}s",
                    counters.decoded,
                    counters.errors,
                    path_points,
                    scan_points,
                    (session.clock().cursor().0 - start) as f64 / 1e9
                ),
                counters.errors > 0,
            );
            let telemetry: HtmlElement = element("telemetry");
            telemetry.set_inner_text(&session.state().telemetry.latest().map_or_else(
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
            let bev_revision = session.state().bev.revision();
            if view.last_bev_revision != Some(bev_revision) || view.last_bev_size != bev_size {
                let path = session
                    .state()
                    .bev
                    .latest()
                    .map_or(&[][..], |frame| frame.points.as_slice());
                draw_bev(path, bev_size);
                view.last_bev_revision = Some(bev_revision);
                view.last_bev_size = bev_size;
            }
            let Some(frame) = session.state().camera.latest() else {
                return;
            };
            if view.last_drawn == Some(frame.arrival_time.0) {
                return;
            }
            match decode_jpeg(&frame.jpeg) {
                Ok(image) => {
                    let canvas: HtmlCanvasElement = element("camera");
                    canvas.set_width(image.width);
                    canvas.set_height(image.height);
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
                    view.last_drawn = Some(frame.arrival_time.0);
                }
                Err(error) => set_status(&error.to_string(), true),
            }
        });
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
