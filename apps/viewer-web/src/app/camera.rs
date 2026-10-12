//! Camera canvas preparation, overlay composition and thumbnail presentation.
use super::{document, duration_since, element, set_status};
use crate::playback::WebPlayback;
use js_sys::Date;
use std::collections::{BTreeMap, BTreeSet};
use viewer_core::{CameraCalibrationSet, CameraId, PresentationMetrics};
use viewer_renderer::{
    CameraBaseImageTracker, CameraOverlaySnapshot, CameraOverlayState, DecodedImage,
    decode_camera_frame,
};
use wasm_bindgen::{Clamped, JsCast};
use web_sys::{
    CanvasRenderingContext2d, HtmlButtonElement, HtmlCanvasElement, HtmlElement, ImageData,
};
#[derive(Default)]
pub(super) struct CameraViewState {
    last_drawn: Option<i64>,
    last_drawn_camera: Option<CameraId>,
    camera_base_images: CameraBaseImageTracker,
    camera_base_canvases: BTreeMap<CameraId, HtmlCanvasElement>,
    pub(super) camera_overlays: CameraOverlayState,
    camera_topics: Vec<(CameraId, String)>,
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

pub(super) fn clear_canvas(canvas_id: &str) {
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

pub(super) fn rebuild_camera_cards(camera_topics: &[(CameraId, String)]) {
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

pub(super) fn update_camera_presentation(
    session: &WebPlayback,
    view: &mut CameraViewState,
    metrics: &mut PresentationMetrics,
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
                    metrics.record_camera(
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
