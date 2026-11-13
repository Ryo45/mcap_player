//! Experimental egui browser host. Recording and independent queries keep their existing owners.
mod gpu;
mod inspection;
mod source;

use crate::{camera_bitmap::CameraBitmapQueue, playback::WebPlayback};
use gpu::GpuViews;
use inspection::Inspection;
use scene_renderer::{SceneCameraMode, SceneFrame};
use source::SourcePicker;
use std::{cell::RefCell, time::Duration};
use viewer_core::{
    CameraId, PathProjectionPolicy, PlatformGeometry, PlaybackCommand, PlaybackEffect,
    PointCloudColoring, PointCloudState, ScenePresentationInput, ScenePresentationState,
};
use wasm_bindgen::{JsCast, prelude::wasm_bindgen};

thread_local! {
    static RUNNER: RefCell<Option<eframe::WebRunner>> = const { RefCell::new(None) };
    static STATUS: RefCell<String> = RefCell::new("{}".into());
}
/// Read-only experimental probe; interaction goes through the actual egui canvas.
#[wasm_bindgen]
pub fn egui_status() -> String {
    STATUS.with(|s| s.borrow().clone())
}

struct ViewerApp {
    picker: SourcePicker,
    session: Option<WebPlayback>,
    source: String,
    error: String,
    gpu: GpuViews,
    bitmaps: CameraBitmapQueue,
    inspection: Inspection,
    scene: ScenePresentationState,
    platform: Option<PlatformGeometry>,
    show_platform: bool,
    selected_camera: Option<CameraId>,
    scene_mode: SceneCameraMode,
    previous: f64,
    filter: String,
    topics: Vec<String>,
    filtered_topics: Vec<usize>,
    mobile_tab: usize,
}
impl ViewerApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        Ok(Self {
            picker: SourcePicker::new(&cc.egui_ctx)?,
            session: None,
            source: String::new(),
            error: String::new(),
            gpu: GpuViews::new(
                cc.wgpu_render_state
                    .clone()
                    .ok_or("wgpu renderer unavailable")?,
            ),
            bitmaps: CameraBitmapQueue::default(),
            inspection: Inspection::default(),
            scene: ScenePresentationState::default(),
            platform: Some(
                serde_json::from_str(include_str!(
                    "../../../config/platform_geometry/turtlebot3_waffle_pi.json"
                ))
                .map_err(|e| e.to_string())?,
            ),
            show_platform: true,
            selected_camera: None,
            scene_mode: SceneCameraMode::Chase,
            previous: js_sys::Date::now(),
            filter: String::new(),
            topics: Vec::new(),
            filtered_topics: Vec::new(),
            mobile_tab: 0,
        })
    }
    fn reset_views(&mut self) {
        self.bitmaps.reset();
        self.gpu.reset();
        self.scene = ScenePresentationState::default();
    }
    fn topics(&mut self, ui: &mut egui::Ui) {
        ui.heading("Topics / Inspection");
        if ui.text_edit_singleline(&mut self.filter).changed() {
            self.filtered_topics = self
                .topics
                .iter()
                .enumerate()
                .filter_map(|(index, topic)| topic.contains(&self.filter).then_some(index))
                .collect();
        }
        if let Some(session) = &self.session {
            egui::ScrollArea::vertical()
                .id_salt("topics")
                .max_height(160.)
                .show_rows(
                    ui,
                    ui.text_style_height(&egui::TextStyle::Body),
                    self.filtered_topics.len(),
                    |ui, rows| {
                        for row in rows {
                            let topic = &self.topics[self.filtered_topics[row]];
                            if ui
                                .selectable_label(
                                    self.inspection.selected.as_ref() == Some(topic),
                                    topic,
                                )
                                .clicked()
                            {
                                self.inspection.select(topic.clone());
                                ui.ctx().request_repaint();
                            }
                        }
                    },
                );
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.inspection.automatic, "Follow");
                if ui.button("Refresh").clicked() {
                    self.inspection.refresh(session);
                    ui.ctx().request_repaint();
                }
            });
        }
        ui.separator();
        egui::ScrollArea::both()
            .id_salt("fields")
            .show(ui, |ui| self.inspection.show(ui));
    }
    fn scene(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (mode, label) in [
                (SceneCameraMode::Chase, "Chase"),
                (SceneCameraMode::Free, "Free"),
                (SceneCameraMode::VehicleEye, "Vehicle eye"),
            ] {
                if ui
                    .selectable_value(&mut self.scene_mode, mode, label)
                    .changed()
                {
                    self.gpu.scene.set_camera_mode(mode);
                }
            }
            if ui.button("Reset").clicked() {
                self.gpu.scene.reset_camera();
            }
            ui.checkbox(&mut self.show_platform, "Body");
        });
        let Some(session) = &self.session else {
            ui.label("Open an indexed MCAP to start");
            return;
        };
        let empty = PointCloudState::default();
        let snapshot = self.scene.build(ScenePresentationInput {
            path: session.path().state(),
            telemetry: session.odometry().state(),
            point_cloud: session.point_cloud().map_or(&empty, |p| p.point_cloud()),
            transforms: session.transforms().state(),
            platform: self.platform.as_ref().filter(|_| self.show_platform),
            path_projection: &PathProjectionPolicy::default(),
            accumulate: false,
            coloring: PointCloudColoring::default(),
        });
        ui.label(format!(
            "{} points · 1 grid cell = 1 metre",
            snapshot.cloud.len()
        ));
        let size = egui::vec2(ui.available_width().max(1.), ui.available_height().max(1.));
        let dpr = ui.ctx().pixels_per_point();
        let texture = self.gpu.render_scene(
            ((size.x * dpr) as u32, (size.y * dpr) as u32),
            SceneFrame {
                revision: snapshot.revision,
                cloud_revision: snapshot.cloud_revision,
                overlay_revision: snapshot.overlay_revision,
                overlay: snapshot.overlay,
                focus: snapshot.focus,
                forward: snapshot.forward,
                view_radius: snapshot.view_radius,
                platform: snapshot.platform,
                path: snapshot.path,
                cloud: snapshot.cloud,
                cloud_colors: snapshot.cloud_colors,
            },
        );
        let response = ui.add(egui::Image::new((texture, size)).sense(egui::Sense::drag()));
        if response.dragged() {
            let delta = response.drag_delta();
            self.gpu.scene.orbit(delta.x, delta.y);
        }
        if response.hovered() {
            let delta = ui.input(|i| i.smooth_scroll_delta.y);
            if delta != 0. {
                self.gpu.scene.zoom(delta);
            }
        }
    }
    fn camera(&mut self, ui: &mut egui::Ui) {
        if self.gpu.software {
            ui.label("Camera upload unavailable on this experimental software adapter");
            return;
        }
        let Some(session) = &mut self.session else {
            return;
        };
        let previous = self.selected_camera;
        egui::ComboBox::from_id_salt("camera-topic")
            .selected_text(
                session
                    .camera_topics()
                    .iter()
                    .find(|(id, _)| Some(*id) == self.selected_camera)
                    .map_or("Camera", |(_, topic)| topic.as_str()),
            )
            .show_ui(ui, |ui| {
                for (id, topic) in session.camera_topics() {
                    ui.selectable_value(&mut self.selected_camera, Some(*id), topic);
                }
            });
        if previous != self.selected_camera {
            if let Some(camera) = self.selected_camera
                && let Err(error) = session.select_camera(camera)
            {
                self.error = error.to_string();
            }
            session.set_focused_camera(self.selected_camera);
        }
        let size = ui.available_size().max(egui::vec2(1., 1.));
        if let Some(camera) = self.selected_camera {
            if let Some((id, pixels)) = self.gpu.camera(camera) {
                let scale = (size.x / pixels.0 as f32).min(size.y / pixels.1 as f32);
                ui.add(egui::Image::new((
                    id,
                    egui::vec2(pixels.0 as f32 * scale, pixels.1 as f32 * scale),
                )));
            } else {
                ui.label("Waiting for image");
            }
            if let Some(frame) = session.cameras().state().latest_for(camera) {
                let dpr = ui.ctx().pixels_per_point();
                if let Err(error) = self
                    .bitmaps
                    .submit(frame, ((size.x * dpr) as u32, (size.y * dpr) as u32))
                {
                    self.error = error;
                }
            }
        }
    }
}
impl eframe::App for ViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = js_sys::Date::now();
        let elapsed = Duration::from_secs_f64(((now - self.previous) / 1000.).clamp(0., 0.25));
        self.previous = now;
        if let Some(result) = self.picker.poll() {
            match result {
                Ok((name, mut session)) => {
                    self.reset_views();
                    self.filter.clear();
                    self.topics = session
                        .catalog()
                        .streams
                        .iter()
                        .map(|stream| stream.topic.clone())
                        .collect();
                    self.topics.sort();
                    self.topics.dedup();
                    self.filtered_topics = (0..self.topics.len()).collect();
                    self.inspection.reset(&session);
                    self.selected_camera = session.camera_topics().first().map(|(id, _)| *id);
                    session.set_focused_camera(self.selected_camera);
                    self.session = Some(session);
                    self.source = name;
                    self.error.clear();
                }
                Err(error) => self.error = error,
            }
        }
        let mut seeked = false;
        if let Some(session) = &mut self.session {
            match session.tick(elapsed) {
                Ok(PlaybackEffect::Seeked) => seeked = true,
                Ok(_) => {}
                Err(error) => self.error = error.to_string(),
            }
        }
        if seeked {
            self.reset_views();
            self.inspection.invalidate();
        }
        if let Some(session) = &self.session {
            self.inspection.poll(session);
        }
        for completed in self.bitmaps.drain() {
            match completed.image {
                Ok(bitmap) => {
                    if let Err(error) =
                        self.gpu
                            .import(completed.frame, completed.pixel_size, &bitmap.0)
                    {
                        self.error = error;
                    }
                }
                Err(error) => self.error = error,
            }
        }
        egui::TopBottomPanel::top("source").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open MCAP").clicked() {
                    self.picker.open();
                }
                ui.label(&self.source);
                ui.label("egui experiment · Local MCAP");
            });
            if !self.error.is_empty() {
                ui.colored_label(egui::Color32::LIGHT_RED, &self.error);
            }
        });
        let mut controls = None;
        egui::TopBottomPanel::bottom("playback").show(ctx, |ui| {
            if let Some(session) = &self.session {
                let mut view = session.clock().view();
                view.cursor = session.timeline_cursor();
                ui.spacing_mut().slider_width = (ui.available_width() - 80.).max(80.);
                controls = Some(viewer_ui::playback::playback_controls(ui, view));
                ui.label(format!("{:?}", session.load_state()));
            }
        });
        let mobile = ctx.content_rect().width() < 700.;
        if mobile {
            egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (index, label) in ["Topics", "Scene", "Camera"].iter().enumerate() {
                        ui.selectable_value(&mut self.mobile_tab, index, *label);
                    }
                });
            });
            egui::CentralPanel::default().show(ctx, |ui| match self.mobile_tab {
                0 => self.topics(ui),
                1 => self.scene(ui),
                _ => self.camera(ui),
            });
        } else {
            egui::SidePanel::left("inspection")
                .default_width(360.)
                .show(ctx, |ui| self.topics(ui));
            egui::CentralPanel::default().show(ctx, |ui| {
                let height = ui.available_height();
                ui.allocate_ui(egui::vec2(ui.available_width(), height * 0.53), |ui| {
                    self.scene(ui)
                });
                ui.separator();
                self.camera(ui);
            });
        }
        self.bitmaps.start_pending(self.selected_camera);
        if let Some(session) = &mut self.session {
            if let Some(controls) = &controls {
                for command in &controls.commands {
                    if let PlaybackCommand::Seek(target) = command
                        && *target == session.timeline_cursor()
                        && !matches!(
                            session.load_state(),
                            viewer_core::PlaybackLoadState::Failed { .. }
                        )
                    {
                        continue;
                    }
                    if let Err(error) = session.apply_command(*command) {
                        self.error = error.to_string();
                    }
                }
            }
            let view = session.clock().view();
            let rect = |r: egui::Rect| [r.min.x, r.min.y, r.max.x, r.max.y];
            STATUS.with(|status| {
                *status.borrow_mut() = serde_json::json!({
                    "source": self.source,
                    "cursor": view.cursor.0.to_string(),
                    "start": view.start.0.to_string(),
                    "end": view.end.0.to_string(),
                    "playing": view.playing,
                    "load": format!("{:?}", session.load_state()),
                    "error": self.error,
                    "uploads": self.gpu.uploads,
                    "camera": self.selected_camera.map(|id|id.0),
                    "inspector": self.inspection.status,
                    "inspector_topic": self.inspection.selected,
                    "has_message": self.inspection.message.is_some(),
                    "camera_texture": self.selected_camera.and_then(|id|self.gpu.camera(id)).is_some(),
                    "device": self.gpu.device_type,
                    "play": controls.as_ref().map(|c|rect(c.play_rect)),
                    "timeline": controls.as_ref().map(|c|rect(c.timeline_rect)),
                }).to_string();
            });
        }
        // Poll only while asynchronous work exists. Paused, settled views do not
        // repeatedly repaint or re-upload unchanged images. SourcePicker wakes egui.
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.clock().is_playing() || session.has_pending_work())
            || self.inspection.loading()
            || self.bitmaps.has_pending_work()
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}
#[wasm_bindgen(start)]
pub fn start() {
    wasm_bindgen_futures::spawn_local(async {
        let result = async {
            let document = web_sys::window()
                .ok_or("missing window")?
                .document()
                .ok_or("missing document")?;
            let canvas = document
                .get_element_by_id("viewer")
                .ok_or("missing canvas")?
                .dyn_into::<web_sys::HtmlCanvasElement>()
                .map_err(|_| "invalid canvas")?;
            let runner = eframe::WebRunner::new();
            runner
                .start(
                    canvas,
                    eframe::WebOptions::default(),
                    Box::new(|cc| Ok(Box::new(ViewerApp::new(cc)?))),
                )
                .await
                .map_err(|e| format!("egui startup: {e:?}"))?;
            RUNNER.with(|r| *r.borrow_mut() = Some(runner));
            if let Some(label) = document.get_element_by_id("startup") {
                label.remove();
            }
            Ok::<(), String>(())
        }
        .await;
        if let Err(error) = result
            && let Some(label) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id("startup"))
        {
            label.set_text_content(Some(&error));
        }
    });
}
