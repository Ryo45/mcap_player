//! Experimental egui browser host. Recording and independent queries keep their existing owners.
mod gpu;
mod inspection;
mod signal;
mod source;

use crate::{camera_bitmap::CameraBitmapQueue, playback::WebPlayback};
use gpu::GpuViews;
use inspection::Inspection;
use scene_renderer::SceneFrame;
use signal::SpeedQuery;
use source::SourcePicker;
use std::{cell::RefCell, time::Duration};
use viewer_core::{
    CameraId, PathProjectionPolicy, PlatformGeometry, PlaybackCommand, PlaybackEffect,
    PointCloudState, ScenePresentationInput, ScenePresentationState,
};
use viewer_ui::{
    action::ViewerAction,
    camera::{CameraPanel, CameraPanelInput},
    panel::Panel,
    plot::{PlotPanel, PlotPanelInput},
    scene::{ScenePanel, ScenePanelInput},
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
    camera_panel: CameraPanel,
    scene_panel: ScenePanel,
    plot_panel: PlotPanel,
    presentation: viewer_core::ViewerPresentation,
    overlays: viewer_renderer::CameraOverlayState,
    show_plot: bool,
    speed_query: Option<SpeedQuery>,
    signal_error: Option<String>,
    preview_time: Option<viewer_core::ArrivalTime>,
    preview_playing: Option<bool>,
    resume_after_seek: bool,
    plot_rect: Option<egui::Rect>,
    plot_chart_rect: Option<egui::Rect>,
    camera_rect: Option<egui::Rect>,
    previous: f64,
    filter: String,
    topics: Vec<String>,
    filtered_topics: Vec<usize>,
    mobile_tab: usize,
}
impl ViewerApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        let Panel::Camera(camera_panel) = make_panel(
            "camera",
            serde_json::json!({"showThumbnails":false,"showOverlay":false}),
        )?
        else {
            return Err("camera factory mismatch".into());
        };
        let Panel::Scene(scene_panel) =
            make_panel("scene-3d", serde_json::json!({"optionalData":true}))?
        else {
            return Err("scene factory mismatch".into());
        };
        let Panel::Plot(plot_panel) =
            make_panel("plot", serde_json::json!({"signal":"vehicle-speed"}))?
        else {
            return Err("plot factory mismatch".into());
        };
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
            camera_panel,
            scene_panel,
            plot_panel,
            presentation: viewer_core::ViewerPresentation::default(),
            overlays: viewer_renderer::CameraOverlayState::default(),
            show_plot: false,
            speed_query: None,
            signal_error: None,
            preview_time: None,
            preview_playing: None,
            resume_after_seek: false,
            plot_rect: None,
            plot_chart_rect: None,
            camera_rect: None,
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
            accumulate: self.scene_panel.state.accumulate_points,
            coloring: self.scene_panel.state.coloring,
        });
        let output = self.scene_panel.show(
            ui,
            ScenePanelInput {
                texture_id: self.gpu.scene_id,
                scan_points: self.presentation.diagnostics.scan_points,
                visible_scan_points: snapshot.cloud.len(),
                camera_distance: self.gpu.scene.camera().distance,
                camera_mode: self.gpu.scene.camera_mode(),
                diagnostics: &snapshot.diagnostics,
                static_transform_count: session.transforms().state().static_len(),
                dynamic_transform_count: session.transforms().state().dynamic_len(),
                playback: Some(session.clock().view()),
                telemetry: self.presentation.telemetry.as_ref(),
                camera: self.presentation.focused_camera(),
                source_error: None,
            },
        );
        if let Some(view) = output.render_requests.scene {
            self.gpu.scene.set_camera_mode(view.camera_mode);
            if view.reset_camera {
                self.gpu.scene.reset_camera();
            }
            self.gpu.scene.zoom(view.wheel_delta);
            self.gpu.scene.orbit(view.orbit_delta.x, view.orbit_delta.y);
            let dpr = ui.ctx().pixels_per_point();
            self.gpu.render_scene(
                (
                    (view.logical_size.x * dpr).ceil() as u32,
                    (view.logical_size.y * dpr).ceil() as u32,
                ),
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
        }
        self.apply_actions(output.actions);
    }
    fn camera(&mut self, ui: &mut egui::Ui) {
        if self.gpu.software {
            ui.label("Camera upload unavailable on this experimental software adapter");
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let textures = self.gpu.texture_views().collect::<Vec<_>>();
        let output = self.camera_panel.show(
            ui,
            CameraPanelInput {
                cameras: &self.presentation.cameras,
                textures: &textures,
                preview_textures: &[],
                preview_active: false,
                overlays: &self.overlays,
            },
        );
        self.camera_rect = Some(ui.min_rect());
        for (camera, size) in output.render_requests.camera_sizes {
            if let Some(frame) = session.cameras().state().latest_for(camera)
                && let Err(error) = self.bitmaps.submit(frame, size)
            {
                self.error = error;
            }
        }
        self.apply_actions(output.actions);
    }
    fn plot(&mut self, ui: &mut egui::Ui) {
        let Some(session) = &self.session else {
            return;
        };
        ui.small("Independent query · first 60 s · at most 2,048 points");
        let query = self.speed_query.as_ref();
        let error = self
            .signal_error
            .as_ref()
            .or_else(|| query.and_then(|q| q.error.as_ref()));
        let current = session
            .odometry()
            .state()
            .latest()
            .map(|frame| viewer_core::SignalSample {
                measurement_time: Some(frame.measurement_time),
                arrival_time: frame.arrival_time,
                value: frame.speed,
            });
        let output = self.plot_panel.show(
            ui,
            PlotPanelInput {
                playback: Some(session.clock().view()),
                signal: query.and_then(|q| q.result.as_ref()),
                current,
                loading: query.is_some_and(|q| q.loading),
                error: error.map(|e| e as &dyn std::fmt::Display),
                display_time: self.preview_time,
                preview_signal: None,
                bookmarks: &[],
            },
        );
        self.plot_chart_rect = output.render_requests.plot_rect;
        self.apply_actions(output.actions);
    }
    fn apply_actions(&mut self, actions: Vec<ViewerAction>) {
        for action in actions {
            match action {
                ViewerAction::SetAccumulatePoints { accumulate, .. } => {
                    self.scene_panel.state.accumulate_points = accumulate
                }
                ViewerAction::SelectCameraTopic { topic, .. } => {
                    if let Some(session) = &mut self.session
                        && let Some(id) = session
                            .camera_topics()
                            .iter()
                            .find(|(_, candidate)| *candidate == topic)
                            .map(|(id, _)| *id)
                    {
                        match session.select_camera(id) {
                            Ok(_) => {
                                self.selected_camera = Some(id);
                                self.camera_panel.state.focused_camera = Some(id);
                                self.camera_panel.state.topic = Some(topic);
                                session.set_focused_camera(Some(id));
                            }
                            Err(error) => self.error = error.to_string(),
                        }
                    }
                }
                ViewerAction::Playback(command) => {
                    if let Some(session) = &mut self.session
                        && let Err(error) = session.apply_command(command)
                    {
                        self.error = error.to_string();
                    }
                }
                ViewerAction::BeginPreview(time) => {
                    self.preview_time = Some(time);
                    if let Some(session) = &mut self.session {
                        let playing = session.clock().is_playing();
                        if self.preview_playing.is_some() {
                            continue;
                        }
                        self.preview_playing = Some(playing);
                        if playing {
                            let _ = session.apply_command(PlaybackCommand::Toggle);
                        }
                    }
                }
                ViewerAction::SetPreviewTime(time) => self.preview_time = time,
                ViewerAction::CommitPreview(time) => {
                    self.preview_time = None;
                    self.resume_after_seek = self.preview_playing.take().unwrap_or(false);
                    if let Some(session) = &mut self.session
                        && let Err(error) = session.apply_command(PlaybackCommand::Seek(time))
                    {
                        self.resume_after_seek = false;
                        self.error = error.to_string();
                    }
                }
                ViewerAction::SelectInspectorTopic(Some(topic)) => self.inspection.select(topic),
                ViewerAction::SelectInspectorTopic(None) => {}
                ViewerAction::AddNumericSeries { .. }
                | ViewerAction::RemoveNumericSeries { .. } => {
                    self.error =
                        "Custom field queries are not yet exposed in this Web experiment".into()
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
                    self.speed_query = None;
                    self.signal_error = None;
                    self.plot_panel.reset_for_source();
                    self.preview_time = None;
                    self.preview_playing = None;
                    self.resume_after_seek = false;
                    self.selected_camera = session.camera_topics().first().map(|(id, _)| *id);
                    session.set_focused_camera(self.selected_camera);
                    self.camera_panel.bind_topics(
                        &session
                            .camera_topics()
                            .iter()
                            .map(|(_, t)| t.clone())
                            .collect::<Vec<_>>(),
                        session.camera_topics().first().map(|(_, t)| t.as_str()),
                        session.camera_topics(),
                    );
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
                Err(error) => {
                    self.error = error.to_string();
                    self.resume_after_seek = false;
                    self.preview_time = None;
                    self.preview_playing = None;
                }
            }
        }
        if seeked {
            self.reset_views();
            self.inspection.invalidate();
            if self.resume_after_seek {
                self.resume_after_seek = false;
                if let Some(session) = &mut self.session
                    && !session.clock().is_playing()
                {
                    let _ = session.apply_command(PlaybackCommand::Toggle);
                }
            }
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
        if let Some(session) = &self.session {
            if self.show_plot && self.speed_query.is_none() && self.signal_error.is_none() {
                match SpeedQuery::new(
                    session,
                    &crate::workspace_bindings::bundled().odometry_topic,
                ) {
                    Ok(query) => self.speed_query = Some(query),
                    Err(error) => self.signal_error = Some(error),
                }
            }
            if let Some(query) = &mut self.speed_query {
                query.poll();
            }
            self.presentation = viewer_core::ViewerPresentation::from_features(
                viewer_core::ViewerPresentationInput {
                    cameras: session.cameras().state(),
                    telemetry: session.odometry().state(),
                    path: session.path().state(),
                    point_cloud: session.point_cloud().map(|p| p.point_cloud()),
                    camera_topics: session.camera_topics(),
                    focused_camera: self.selected_camera,
                    overlays: &std::collections::BTreeMap::new(),
                    diagnostics: viewer_core::DiagnosticsPresentation::default(),
                },
            );
        }
        egui::TopBottomPanel::top("source").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open MCAP").clicked() {
                    self.picker.open();
                }
                ui.label(&self.source);
                ui.label("egui experiment · Local MCAP");
                let checkbox = ui.checkbox(&mut self.show_plot, "Speed plot");
                self.plot_rect = Some(checkbox.rect);
                if checkbox.changed() {
                    self.speed_query = None;
                    self.signal_error = None;
                    self.plot_panel.reset_for_source();
                    self.plot_chart_rect = None;
                    ui.ctx().request_repaint();
                }
                ui.checkbox(&mut self.show_platform, "Body");
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
                    for (index, label) in ["Topics", "Scene", "Camera", "Plot"].iter().enumerate() {
                        if ui
                            .selectable_value(&mut self.mobile_tab, index, *label)
                            .clicked()
                            && index == 3
                        {
                            self.show_plot = true;
                            ui.ctx().request_repaint();
                        }
                    }
                });
            });
            egui::CentralPanel::default().show(ctx, |ui| match self.mobile_tab {
                0 => self.topics(ui),
                1 => self.scene(ui),
                2 => self.camera(ui),
                _ => self.plot(ui),
            });
        } else {
            egui::SidePanel::left("inspection")
                .default_width(360.)
                .show(ctx, |ui| {
                    if self.show_plot {
                        let height = ui.available_height();
                        ui.allocate_ui(egui::vec2(ui.available_width(), height * 0.55), |ui| {
                            self.topics(ui)
                        });
                        ui.separator();
                        self.plot(ui);
                    } else {
                        self.topics(ui);
                    }
                });
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
                    if matches!(command, PlaybackCommand::Toggle) {
                        self.resume_after_seek = false;
                    }
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
                    "plot_checkbox":self.plot_rect.map(rect),
                    "plot_chart":self.plot_chart_rect.map(rect),
                    "camera_pixels":self.selected_camera.and_then(|id|self.gpu.camera(id)).map(|(_,size)|[size.0,size.1]),
                    "plot_points":self.speed_query.as_ref().and_then(|q|q.result.as_ref()).map_or(0,|s|s.display.values.len()),
                    "plot_error":self.signal_error.as_ref().or_else(||self.speed_query.as_ref().and_then(|q|q.error.as_ref())),
                    "plot_loading":self.speed_query.as_ref().is_some_and(|q|q.loading),
                    "plot_requests":self.speed_query.as_ref().map_or(0,|q|q.request_count()),
                    "camera_rect":self.camera_rect.map(rect),
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
            || self.speed_query.as_ref().is_some_and(|q| q.loading)
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}
fn make_panel(kind: &str, config: serde_json::Value) -> Result<Panel, String> {
    let node = viewer_layout::PanelNode {
        id: viewer_layout::PanelId::new(kind).map_err(|e| e.to_string())?,
        panel_type: kind.into(),
        config_version: 1,
        title: None,
        config,
    };
    Panel::create(&node).ok_or_else(|| format!("Unknown panel {kind}"))?
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
