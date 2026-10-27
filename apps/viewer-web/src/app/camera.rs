//! Camera canvas preparation, overlay composition and thumbnail presentation.
use super::{document, duration_since, element};
use crate::{
    camera_bitmap::{CameraBitmapQueue, target_size},
    playback::WebPlayback,
};
use js_sys::Date;
use std::collections::{BTreeMap, BTreeSet};
use viewer_core::{CameraCalibrationSet, CameraId, PresentationMetrics};
use viewer_renderer::{CameraBaseImageTracker, CameraOverlaySnapshot, CameraOverlayState};
use wasm_bindgen::JsCast;
use web_sys::{
    CanvasRenderingContext2d, HtmlButtonElement, HtmlCanvasElement, HtmlElement, HtmlSelectElement,
    ImageBitmap,
};
struct CameraImageGeometry {
    source_size: (u32, u32),
    pixel_size: (u32, u32),
}

#[derive(Default)]
pub(super) struct CameraViewState {
    last_drawn: Option<i64>,
    last_drawn_camera: Option<CameraId>,
    camera_base_images: CameraBaseImageTracker,
    camera_base_canvases: BTreeMap<CameraId, HtmlCanvasElement>,
    pub(super) camera_overlays: CameraOverlayState,
    camera_topics: Vec<(CameraId, String)>,
    decode: CameraBitmapQueue,
    geometry: BTreeMap<CameraId, CameraImageGeometry>,
    presented_frames: BTreeMap<CameraId, viewer_core::CameraFrameMetadata>,
    pub(super) errors: BTreeMap<CameraId, String>,
}

impl CameraViewState {
    fn record_error(&mut self, id: CameraId, error: String) {
        if self.errors.len() < 32 || self.errors.contains_key(&id) {
            self.errors.insert(id, format!("camera {}: {error}", id.0));
        }
    }
    pub(super) fn reset(&mut self) {
        self.decode.reset();
        self.errors.clear();
        for canvas in self.camera_base_canvases.values() {
            canvas.set_width(0);
            canvas.set_height(0);
        }
        self.camera_base_canvases.clear();
        self.camera_base_images.clear();
        self.presented_frames.clear();
        self.geometry.clear();
        self.camera_overlays.reset_source();
        self.camera_topics.clear();
        self.last_drawn = None;
        self.last_drawn_camera = None;
    }
}

fn display_target(canvas: &HtmlCanvasElement) -> (u32, u32) {
    let density = web_sys::window().expect("window").device_pixel_ratio();
    (
        ((canvas.client_width().max(1) as f64 * density)
            .ceil()
            .max(1.0)) as u32,
        ((canvas.client_height().max(1) as f64 * density)
            .ceil()
            .max(1.0)) as u32,
    )
}
fn decode_target(id: CameraId, selected: Option<CameraId>) -> (u32, u32) {
    let thumbnail: HtmlCanvasElement = element(&format!("camera-thumb-{}", id.0));
    let mut target = display_target(&thumbnail);
    if selected == Some(id) {
        let focus: HtmlCanvasElement = element("camera");
        let size = display_target(&focus);
        target.0 = target.0.max(size.0);
        target.1 = target.1.max(size.1);
    }
    target
}
fn draw_base_image(canvas: &HtmlCanvasElement, image: &ImageBitmap) -> Result<(), String> {
    if canvas.width() != image.width() {
        canvas.set_width(image.width());
    }
    if canvas.height() != image.height() {
        canvas.set_height(image.height());
    }
    let context: CanvasRenderingContext2d = canvas
        .get_context("2d")
        .map_err(|e| format!("camera context: {e:?}"))?
        .ok_or("camera 2d context unavailable")?
        .dyn_into()
        .map_err(|_| "invalid camera 2d context")?;
    // Each frame replaces the previous pixels, including transparent regions.
    context.clear_rect(
        0.0,
        0.0,
        f64::from(canvas.width()),
        f64::from(canvas.height()),
    );
    context
        .draw_image_with_image_bitmap(image, 0.0, 0.0)
        .map_err(|e| format!("camera upload: {e:?}"))
}

fn compose_camera_canvas(
    destination_id: &str,
    base: &HtmlCanvasElement,
    overlay: Option<&CameraOverlaySnapshot>,
    source_size: (u32, u32),
) {
    let destination: HtmlCanvasElement = element(destination_id);
    let size = target_size(source_size, display_target(&destination));
    if destination.width() != size.0 {
        destination.set_width(size.0);
    }
    if destination.height() != size.1 {
        destination.set_height(size.1);
    }
    let context: CanvasRenderingContext2d = destination
        .get_context("2d")
        .expect("2d query")
        .expect("2d context")
        .dyn_into()
        .expect("canvas 2d");
    // Recompose from the base; old pixels and overlays must not accumulate.
    context.clear_rect(0.0, 0.0, f64::from(size.0), f64::from(size.1));
    context
        .draw_image_with_html_canvas_element_and_dw_and_dh(
            base,
            0.0,
            0.0,
            f64::from(size.0),
            f64::from(size.1),
        )
        .expect("draw camera base image");
    if let Some(overlay) = overlay
        && overlay.image_size == source_size
    {
        draw_camera_overlay(
            &context,
            overlay,
            [
                f64::from(size.0) / f64::from(source_size.0),
                f64::from(size.1) / f64::from(source_size.1),
            ],
        );
    }
}

fn draw_camera_overlay(
    context: &CanvasRenderingContext2d,
    overlay: &CameraOverlaySnapshot,
    scale: [f64; 2],
) {
    for (color, width) in [("rgba(0,0,0,0.7)", 5.0), ("#2deba5", 2.0)] {
        context.set_stroke_style_str(color);
        context.set_line_width(width);
        for pair in overlay.projected_path.windows(2) {
            let [Some(start), Some(end)] = pair else {
                continue;
            };
            context.begin_path();
            context.move_to(
                f64::from(start[0]) * scale[0],
                f64::from(start[1]) * scale[1],
            );
            context.line_to(f64::from(end[0]) * scale[0], f64::from(end[1]) * scale[1]);
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
    let selector: HtmlSelectElement = element("camera-topic");
    selector.set_inner_html("");
    selector.set_disabled(camera_topics.is_empty());
    if camera_topics.is_empty() {
        let option = document()
            .create_element("option")
            .expect("empty camera option");
        option.set_text_content(Some("No image topics"));
        selector
            .append_child(&option)
            .expect("empty camera option append");
    }
    let container: HtmlElement = element("camera-thumbnails");
    container.set_inner_html("");
    for (camera_id, topic) in camera_topics {
        let option = document()
            .create_element("option")
            .expect("camera topic option");
        option
            .set_attribute("value", &camera_id.0.to_string())
            .expect("camera topic value");
        option.set_text_content(Some(topic));
        selector.append_child(&option).expect("camera topic append");
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
    policy: &viewer_core::PathProjectionPolicy,
    selected_camera: Option<CameraId>,
) {
    let camera_topics = session.camera_topics();
    if view.camera_topics.as_slice() != camera_topics {
        rebuild_camera_cards(camera_topics);
        view.camera_topics = camera_topics.to_vec();
    }
    let selector: HtmlSelectElement = element("camera-topic");
    if let Some(id) = selected_camera {
        let value = id.0.to_string();
        if selector.value() != value {
            selector.set_value(&value);
        }
    }
    if view.last_drawn_camera != selected_camera {
        clear_canvas("camera");
        view.last_drawn = None;
        view.last_drawn_camera = selected_camera;
    }

    let cameras = session.cameras().state();
    let mut camera_visual_changes = BTreeSet::new();
    for completed in view.decode.drain() {
        let frame = completed.frame;
        let id = frame.camera_id;
        if view
            .camera_base_images
            .arrival(id)
            .is_some_and(|arrival| arrival > frame.arrival_time)
            || target_size(completed.source_size, decode_target(id, selected_camera))
                != completed.pixel_size
        {
            continue;
        }
        match completed.image {
            Ok(bitmap) => {
                let uploaded = Date::now();
                let base = view.camera_base_canvases.entry(id).or_insert_with(|| {
                    document()
                        .create_element("canvas")
                        .expect("camera base")
                        .dyn_into()
                        .expect("canvas")
                });
                if let Err(error) = draw_base_image(base, &bitmap.0) {
                    view.record_error(id, error);
                    continue;
                }
                view.errors.remove(&id);
                view.camera_base_images
                    .mark_updated(frame.camera_id, frame.arrival_time);
                view.geometry.insert(
                    id,
                    CameraImageGeometry {
                        source_size: completed.source_size,
                        pixel_size: completed.pixel_size,
                    },
                );
                view.presented_frames.insert(id, frame);
                metrics.record_camera(id, completed.decode_time, duration_since(uploaded));
                camera_visual_changes.insert(id);
            }
            Err(error) => view.record_error(id, error),
        }
    }
    for (camera_id, frame) in cameras.frames() {
        let target = decode_target(*camera_id, selected_camera);
        if (view
            .camera_base_images
            .needs_update(frame.camera_id, frame.arrival_time)
            || view.geometry.get(camera_id).is_none_or(|geometry| {
                target_size(geometry.source_size, target) != geometry.pixel_size
            }))
            && let Err(error) = view.decode.submit(frame, target)
        {
            view.record_error(*camera_id, error);
        }
    }
    let priority = selected_camera.filter(|id| {
        session.clock().is_playing() && cameras.frames().any(|(camera, _)| camera == id)
    });
    view.decode.start_pending(priority);
    for (camera_id, frame) in &view.presented_frames {
        if view.camera_overlays.update(
            frame,
            view.geometry[camera_id].source_size,
            session.path().state().latest(),
            session.path().state().revision(),
            session.transforms().state(),
            session.transforms().state().revision(),
            calibrations,
            policy,
        ) {
            camera_visual_changes.insert(*camera_id);
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
            view.geometry[camera_id].source_size,
        );
    }

    let selected_has_base =
        selected_camera.is_some_and(|id| view.presented_frames.contains_key(&id));
    if !selected_has_base && view.last_drawn.is_some() {
        clear_canvas("camera");
        view.last_drawn = None;
    }
    if let Some(id) = selected_camera
        && let Some(frame) = view.presented_frames.get(&id)
        && let Some(base) = view.camera_base_canvases.get(&id)
    {
        let canvas: HtmlCanvasElement = element("camera");
        let size = target_size(view.geometry[&id].source_size, display_target(&canvas));
        if view.last_drawn != Some(frame.arrival_time.0)
            || camera_visual_changes.contains(&id)
            || (canvas.width(), canvas.height()) != size
        {
            compose_camera_canvas(
                "camera",
                base,
                view.camera_overlays.snapshot(id),
                view.geometry[&id].source_size,
            );
            view.last_drawn = Some(frame.arrival_time.0);
        }
    }
    if cameras.frames().next().is_none() {
        for (camera_id, _) in camera_topics {
            clear_canvas(&format!("camera-thumb-{}", camera_id.0));
        }
    }
}
