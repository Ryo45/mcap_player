//! egui WebGPU host: browser I/O, presentation and GPU ownership; shared panels/layout own UI.
mod gpu;
mod inspection;
mod odometry;
mod settings;
mod source;

use crate::{camera_bitmap::CameraBitmapQueue, playback::WebPlayback};
use gpu::GpuViews;
use inspection::Inspection;
use odometry::OdometryQuery;
use settings::PhysicalSettings;
use source::{FileKind, FileResult, SourcePicker};
use std::{cell::RefCell, time::Duration};
use viewer_core::{
    ArrivalTime, CameraId, PlaybackCommand, PlaybackEffect, PointCloudState,
    ScenePresentationInput, ScenePresentationState, TopicTree, ViewerPresentation,
    WorkspaceBindings,
};
use viewer_layout::{LayoutDocument, LayoutNode, PanelId};
use viewer_ui::{
    action::ViewerAction,
    panel::{Panel, PanelOutput, PanelRenderRequests, PanelRequirements},
    workspace::PanelStore,
};
use wasm_bindgen::{JsCast, prelude::wasm_bindgen};

thread_local! {
    static RUNNER:RefCell<Option<eframe::WebRunner>>=const {RefCell::new(None)};
    static STATUS:RefCell<String>=RefCell::new("{}".into());
}
#[wasm_bindgen]
pub fn egui_status() -> String {
    STATUS.with(|s| s.borrow().clone())
}

fn panel_from_node(node: &viewer_layout::PanelNode) -> Panel {
    match Panel::create(node) {
        Some(Ok(panel)) => panel,
        Some(Err(error)) => Panel::Placeholder(
            viewer_ui::placeholder::PlaceholderPanel::invalid_config(node, error),
        ),
        None => Panel::Placeholder(viewer_ui::placeholder::PlaceholderPanel::unknown_type(node)),
    }
}
fn build_panels(layout: &LayoutDocument) -> (PanelStore<Panel>, Vec<String>) {
    PanelStore::from_layout(layout, panel_from_node, Panel::kind_name, |node, error| {
        Panel::Placeholder(viewer_ui::placeholder::PlaceholderPanel::invalid_config(
            node, error,
        ))
    })
}
fn bundled_layout() -> LayoutDocument {
    LayoutDocument::from_json(include_str!("../../../config/layouts/web_monitor.json"))
        .expect("bundled monitor layout")
}

struct ViewerApp {
    picker: SourcePicker,
    session: Option<WebPlayback>,
    active_bindings: WorkspaceBindings,
    source: String,
    source_revision: u64,
    error: String,
    gpu: GpuViews,
    bitmaps: CameraBitmapQueue,
    inspection: Inspection,
    pinned_inspections: std::collections::BTreeMap<String, Inspection>,
    pinned_order: Vec<String>,
    pinned_next: usize,
    scene: ScenePresentationState,
    physical: PhysicalSettings,
    layout: std::sync::Arc<LayoutDocument>,
    panels: PanelStore<Panel>,
    requirements: PanelRequirements,
    layout_label: String,
    layout_warnings: Vec<String>,
    presentation: ViewerPresentation,
    topic_tree: TopicTree,
    overlays: viewer_renderer::CameraOverlayState,
    metrics: viewer_core::PresentationMetrics,
    speed_query: Option<OdometryQuery>,
    signal_error: Option<String>,
    preview_time: Option<ArrivalTime>,
    preview_playing: Option<bool>,
    resume_after_seek: bool,
    previous: f64,
    mobile_panel: Option<PanelId>,
    settings_open: bool,
    controls: std::collections::BTreeMap<String, [f32; 4]>,
    play_rect: Option<egui::Rect>,
    timeline_rect: Option<egui::Rect>,
    plot_rect: Option<egui::Rect>,
    plot_chart_rect: Option<egui::Rect>,
}
impl ViewerApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        let layout = bundled_layout();
        let (panels, warnings) = build_panels(&layout);
        let mut app = Self {
            picker: SourcePicker::new(&cc.egui_ctx)?,
            session: None,
            active_bindings: crate::workspace_bindings::bundled(),
            source: String::new(),
            source_revision: 0,
            error: String::new(),
            gpu: GpuViews::new(
                cc.wgpu_render_state
                    .clone()
                    .ok_or("wgpu renderer unavailable")?,
            ),
            bitmaps: CameraBitmapQueue::default(),
            inspection: Inspection::default(),
            pinned_inspections: Default::default(),
            pinned_order: Vec::new(),
            pinned_next: 0,
            scene: ScenePresentationState::default(),
            physical: PhysicalSettings::bundled()?,
            layout: std::sync::Arc::new(layout),
            panels,
            requirements: PanelRequirements::default(),
            layout_label: "Monitor".into(),
            layout_warnings: warnings,
            presentation: ViewerPresentation::default(),
            topic_tree: TopicTree::default(),
            overlays: Default::default(),
            metrics: Default::default(),
            speed_query: None,
            signal_error: None,
            preview_time: None,
            preview_playing: None,
            resume_after_seek: false,
            previous: js_sys::Date::now(),
            mobile_panel: None,
            settings_open: false,
            controls: Default::default(),
            play_rect: None,
            timeline_rect: None,
            plot_rect: None,
            plot_chart_rect: None,
        };
        app.refresh_requirements();
        Ok(app)
    }
    fn track(&mut self, name: &str, response: &egui::Response) {
        let r = response.rect;
        self.controls
            .insert(name.into(), [r.min.x, r.min.y, r.max.x, r.max.y]);
    }
    fn refresh_requirements(&mut self) {
        let mut requirements = PanelRequirements::default();
        for panel in self.panels.values() {
            panel.contribute_requirements(&mut requirements);
        }
        self.picker.settings.borrow_mut().requirements = requirements.playback.clone();
        self.requirements = requirements;
    }
    fn configure_inspections(&mut self) {
        self.pinned_inspections.clear();
        if let Some(session) = &self.session {
            for topic in &self.requirements.inspections {
                let mut controller = Inspection::default();
                controller.reset(session);
                controller.select(topic.clone());
                self.pinned_inspections.insert(topic.clone(), controller);
            }
        }
        self.pinned_order = self.pinned_inspections.keys().cloned().collect();
        self.pinned_next = 0;
    }
    fn focused_camera(&self) -> Option<CameraId> {
        self.panels
            .values()
            .filter_map(|p| match p {
                Panel::Camera(p) if p.scheduler_priority_topic().is_some() => {
                    p.state.focused_camera
                }
                _ => None,
            })
            .next()
            .or_else(|| {
                self.panels.values().find_map(|p| match p {
                    Panel::Camera(p) => p.state.focused_camera,
                    _ => None,
                })
            })
    }
    fn bind_cameras(&mut self, change: Option<(&PanelId, &str)>) -> Result<(), String> {
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let selection = viewer_ui::camera::select_topics(
            session.catalog(),
            "",
            self.panels.iter().filter_map(|(id, p)| match p {
                Panel::Camera(p) => Some((id, p)),
                _ => None,
            }),
            change,
            false,
        )?;
        let planned_topics = session
            .camera_topics()
            .iter()
            .map(|(_, t)| t.clone())
            .collect::<Vec<_>>();
        for (id, panel) in self.panels.iter_mut() {
            if let Panel::Camera(camera) = panel {
                camera.bind_topics(
                    &planned_topics,
                    selection
                        .panels
                        .get(id)
                        .and_then(|t| t.as_deref())
                        .filter(|t| planned_topics.iter().any(|p| p == t)),
                    session.camera_topics(),
                );
            }
        }
        Ok(())
    }
    fn reset_views(&mut self) {
        self.bitmaps.reset();
        self.gpu.reset();
        self.scene = Default::default();
        self.overlays.reset_source();
    }
    fn invalidate_scene_views(&mut self) {
        self.gpu.scene.invalidate();
        self.gpu.bev.invalidate();
    }
    fn replace_layout(&mut self, layout: LayoutDocument, label: String) -> Result<(), String> {
        layout.validate().map_err(|e| e.to_string())?;
        let (panels, warnings) = build_panels(&layout);
        self.panels = panels;
        self.layout = std::sync::Arc::new(layout);
        self.layout_label = label;
        self.layout_warnings = warnings;
        self.refresh_requirements();
        self.configure_inspections();
        self.mobile_panel = None;
        self.speed_query = None;
        self.signal_error = None;
        self.plot_chart_rect = None;
        if let Err(error) = self.bind_cameras(None) {
            self.error = error;
        }
        self.gpu.scene.invalidate();
        self.gpu.bev.invalidate();
        Ok(())
    }
    fn toggle_plot(&mut self, enabled: bool) {
        fn remove(node: LayoutNode) -> Option<LayoutNode> {
            match node {
                LayoutNode::Panel(p) if p.panel_type == "plot" => None,
                LayoutNode::Panel(p) => Some(LayoutNode::Panel(p)),
                LayoutNode::Split {
                    direction,
                    children,
                } => {
                    let mut children = children
                        .into_iter()
                        .filter_map(|c| {
                            remove(c.node).map(|node| viewer_layout::SplitChild {
                                weight: c.weight,
                                node,
                            })
                        })
                        .collect::<Vec<_>>();
                    match children.len() {
                        0 => None,
                        1 => Some(children.remove(0).node),
                        _ => Some(LayoutNode::Split {
                            direction,
                            children,
                        }),
                    }
                }
            }
        }
        let mut layout = (*self.layout).clone();
        if enabled {
            let mut suffix = 0;
            let id = loop {
                let id = PanelId::new(format!("speed-plot-{suffix}")).expect("generated id");
                if self.panels.get(&id).is_none() {
                    break id;
                }
                suffix += 1;
            };
            let plot = LayoutNode::Panel(viewer_layout::PanelNode {
                id,
                panel_type: "plot".into(),
                config_version: 1,
                title: Some("Speed".into()),
                config: serde_json::json!({"signal":"vehicle-speed"}),
            });
            if let LayoutNode::Split {
                direction: viewer_layout::SplitDirection::Row,
                children,
            } = &mut layout.root
            {
                let Some(first) = children.first_mut() else {
                    self.error = "Layout split must have children".into();
                    return;
                };
                let existing = first.node.clone();
                first.node = LayoutNode::Split {
                    direction: viewer_layout::SplitDirection::Column,
                    children: vec![
                        viewer_layout::SplitChild {
                            weight: 0.55,
                            node: existing,
                        },
                        viewer_layout::SplitChild {
                            weight: 0.45,
                            node: plot,
                        },
                    ],
                };
            } else {
                layout.root = LayoutNode::Split {
                    direction: viewer_layout::SplitDirection::Column,
                    children: vec![
                        viewer_layout::SplitChild {
                            weight: 0.7,
                            node: layout.root,
                        },
                        viewer_layout::SplitChild {
                            weight: 0.3,
                            node: plot,
                        },
                    ],
                };
            }
        } else {
            layout.root = remove(layout.root).unwrap_or_else(|| {
                LayoutNode::Panel(viewer_layout::PanelNode {
                    id: PanelId::new("empty").expect("static id"),
                    panel_type: "topics".into(),
                    config_version: 1,
                    title: None,
                    config: serde_json::json!({}),
                })
            });
        }
        if let Err(error) = self.replace_layout(layout, "Custom".into()) {
            self.error = error;
        }
    }
    fn poll_files(&mut self) {
        while let Some(result) = self.picker.poll() {
            match result {
                FileResult::Source(Ok(opened)) => {
                    let opened = *opened;
                    self.reset_views();
                    self.inspection.reset(&opened.playback);
                    self.topic_tree = TopicTree::from_catalog(opened.playback.catalog());
                    self.active_bindings = opened.bindings;
                    self.session = Some(opened.playback);
                    self.source = opened.name;
                    self.source_revision = self.source_revision.wrapping_add(1);
                    self.error.clear();
                    self.configure_inspections();
                    self.metrics.reset();
                    self.speed_query = None;
                    self.signal_error = None;
                    self.preview_time = None;
                    self.preview_playing = None;
                    self.resume_after_seek = false;
                    for panel in self.panels.values_mut() {
                        panel.reset_for_source(None);
                    }
                    if let Err(error) = self.bind_cameras(None) {
                        self.error = error;
                    }
                }
                FileResult::Source(Err(error)) => self.error = error,
                FileResult::Json(kind, name, result) => {
                    let result = result.and_then(|json| match kind {
                        FileKind::Bindings => self.picker.settings.borrow().resolve().map(|_| ()),
                        FileKind::Calibration => {
                            viewer_core::CameraCalibrationSet::from_json(&json)
                                .map_err(|e| e.to_string())
                                .map(|c| {
                                    self.physical.calibrations = c;
                                    self.physical.calibration_label = name.clone();
                                    self.overlays.reset_source();
                                    self.invalidate_scene_views();
                                })
                        }
                        FileKind::Platform => serde_json::from_str(&json)
                            .map_err(|e| e.to_string())
                            .map(|g| {
                                self.physical.platform = Some(g);
                                self.physical.platform_label = name.clone();
                                self.physical.enabled = true;
                                self.scene = Default::default();
                                self.invalidate_scene_views();
                            }),
                        FileKind::Layout => LayoutDocument::from_json(&json)
                            .map_err(|e| e.to_string())
                            .and_then(|layout| self.replace_layout(layout, name.clone())),
                        FileKind::Mcap => unreachable!(),
                    });
                    match result {
                        Err(error) => self.error = format!("{name}: {error}"),
                        Ok(()) => self.error.clear(),
                    }
                }
            }
        }
    }
    fn settings(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_open;
        egui::Window::new("Recording / physical settings")
            .open(&mut open)
            .default_width(560_f32.min((ctx.content_rect().width() - 32.).max(240.)))
            .max_height((ctx.content_rect().height() - 64.).max(160.))
            .vscroll(true)
            .show(ctx, |ui| {
                ui.heading("Next recording");
                ui.small("Bindings and layout input requirements apply when opening a recording.");
                ui.small("Camera choices are limited to streams selected on open. Reopen after changing layout inputs.");
                let busy = self.picker.busy.get();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!busy, egui::Button::new("Workspace bindings JSON"))
                        .clicked()
                    {
                        self.picker.pick(FileKind::Bindings);
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Use bundled bindings"))
                        .clicked()
                    {
                        self.picker.settings.borrow_mut().bindings.reset();
                    }
                });
                {
                    let mut settings = self.picker.settings.borrow_mut();
                    ui.label(&settings.bindings.label);
                    ui.label("Point cloud topic override (optional)");
                    ui.text_edit_singleline(&mut settings.point_cloud);
                    if let Err(error) = settings.resolve() {
                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                    }
                }
                ui.separator();
                ui.heading("Recording Server");
                let url = ui.text_edit_singleline(&mut self.picker.remote.url);
                self.track("remote-url", &url);
                if url.changed() {
                    self.picker.remote.address_changed();
                }
                let connect = ui.add_enabled(!busy, egui::Button::new("Connect"));
                self.track("remote-connect", &connect);
                if connect.clicked() {
                    self.picker.remote.connect();
                }
                let connection = self.picker.remote.connection.borrow();
                let mut selected = self.picker.remote.selected.clone();
                let choice = egui::ComboBox::from_id_salt("remote-recording")
                    .selected_text(if selected.is_empty() {
                        "Select recording"
                    } else {
                        &selected
                    })
                    .show_ui(ui, |ui| {
                        if let Some(connection) = connection.as_ref() {
                            for recording in &connection.recordings {
                                let response = ui.selectable_value(
                                    &mut selected,
                                    recording.recording_id.clone(),
                                    &recording.display_name,
                                );
                                self.controls.insert(
                                    format!("recording-{}", recording.recording_id),
                                    [response.rect.min.x, response.rect.min.y,
                                     response.rect.max.x, response.rect.max.y],
                                );
                            }
                        }
                    });
                drop(connection);
                self.picker.remote.select(selected);
                self.track("remote-choice", &choice.response);
                let ready = !self.picker.remote.selected.is_empty()
                    && self.picker.remote.connection.borrow().is_some();
                let catalog = ui.add_enabled(!busy && ready, egui::Button::new("Load Catalog only"));
                self.track("remote-catalog", &catalog);
                if catalog.clicked() {
                    self.picker.remote.load_catalog();
                }
                let open_remote = ui.add_enabled(!busy && ready, egui::Button::new("Open Remote Playback"));
                self.track("remote-open", &open_remote);
                if open_remote.clicked() {
                    self.picker.remote.open();
                }
                ui.label(self.picker.remote.status.borrow().as_str());
                if let Some(connection) = self.picker.remote.connection.borrow().as_ref() {
                    ui.small(format!("Connected: {}", connection.address));
                    if let Some(catalog) = &connection.catalog {
                        ui.label(format!("Recording: {} · {} streams", catalog.recording_id, catalog.streams.len()));
                        ui.small(format!("Revision: {}", catalog.recording_revision));
                        ui.monospace(format!("Log time [{} .. {}) ns", catalog.time_range.start_ns, catalog.time_range.end_ns_exclusive));
                        egui::ScrollArea::vertical()
                            .id_salt("catalog-preview")
                            .max_height(180.)
                            .show_rows(ui, ui.spacing().interact_size.y, catalog.streams.len(), |ui, rows| {
                                for stream in &catalog.streams[rows] {
                                    ui.label(format!(
                                        "{} · {} · {} · {} · {} messages",
                                        stream.id, stream.topic, stream.schema_name, stream.message_encoding,
                                        stream.message_count.map_or_else(|| "unknown".into(), |n| n.get().to_string()),
                                    ));
                                }
                            });
                    }
                }
                ui.separator();
                ui.heading("Physical geometry / projection");
                let body = ui.checkbox(&mut self.physical.enabled, "Show configured physical body");
                self.track("physical-body", &body);
                if body.changed() {
                    self.invalidate_scene_views();
                }
                ui.label(&self.physical.platform_label);
                if ui
                    .add_enabled(!busy, egui::Button::new("Platform geometry JSON"))
                    .clicked()
                {
                    self.picker.pick(FileKind::Platform);
                }
                ui.label("World frame");
                ui.text_edit_singleline(&mut self.physical.fixed_frame);
                ui.label("Ground frame (empty = original Path z)");
                ui.text_edit_singleline(&mut self.physical.ground_frame);
                let apply_frames = ui.button("Apply frames");
                self.track("physical-frames", &apply_frames);
                if apply_frames.clicked() && self.physical.apply_frames() {
                    self.scene = Default::default();
                    self.overlays.reset_source();
                    self.invalidate_scene_views();
                }
                if !self.physical.error.is_empty() {
                    ui.colored_label(egui::Color32::LIGHT_RED, &self.physical.error);
                }
                ui.separator();
                ui.heading("Camera calibration");
                ui.label(&self.physical.calibration_label);
                if ui
                    .add_enabled(!busy, egui::Button::new("Calibration JSON"))
                    .clicked()
                {
                    self.picker.pick(FileKind::Calibration);
                }
                ui.separator();
                ui.heading("Layout");
                if ui
                    .add_enabled(!busy, egui::Button::new("Layout JSON"))
                    .clicked()
                {
                    self.picker.pick(FileKind::Layout);
                }
                for warning in &self.layout_warnings {
                    ui.colored_label(egui::Color32::LIGHT_RED, warning);
                }
                ui.separator();
                if let Some(session) = &self.session {
                    ui.collapsing("Recording data plane", |ui| {
                        ui.monospace(format!("{:#?}", session.data_plane_diagnostics()));
                    });
                }
                let automatic = ui.checkbox(&mut self.inspection.automatic, "Inspector follows cursor");
                self.track("inspector-auto", &automatic);
                if ui.button("Refresh Inspector").clicked()
                    && let Some(session) = &self.session
                {
                    self.inspection.refresh(session);
                    ctx.request_repaint();
                }
            });
        self.settings_open = open;
    }
    fn render_views(
        &mut self,
        requests: &PanelRenderRequests,
        snapshot: Option<&viewer_core::SceneSnapshot<'_>>,
        dpr: f32,
    ) {
        let Some(scene) = snapshot else { return };
        if let Some(view) = &requests.scene {
            self.gpu.scene.set_camera_mode(view.camera_mode);
            if view.reset_camera {
                self.gpu.scene.reset_camera();
            }
            self.gpu.scene.zoom(view.wheel_delta);
            self.gpu.scene.orbit(view.orbit_delta.x, view.orbit_delta.y);
            self.gpu.render_scene(
                (
                    (view.logical_size.x * dpr).ceil() as u32,
                    (view.logical_size.y * dpr).ceil() as u32,
                ),
                scene_renderer::SceneFrame {
                    revision: scene.revision,
                    cloud_revision: scene.cloud_revision,
                    overlay_revision: scene.overlay_revision,
                    overlay: scene.overlay,
                    focus: scene.focus,
                    forward: scene.forward,
                    view_radius: scene.view_radius,
                    platform: scene.platform,
                    path: scene.path,
                    cloud: scene.cloud,
                    cloud_colors: scene.cloud_colors,
                },
            );
        }
        if let Some(size) = requests.bev_size {
            self.gpu.render_bev(
                ((size.x * dpr).ceil() as u32, (size.y * dpr).ceil() as u32),
                bev_renderer::BevFrame {
                    revision: scene.bev.revision,
                    path: scene.bev.path,
                    footprint: scene.bev.footprint,
                },
            );
        }
    }
    fn apply_actions(&mut self, actions: Vec<ViewerAction>) {
        for action in actions {
            match action {
                ViewerAction::SelectCameraTopic { panel_id, topic } => {
                    let id = self.session.as_ref().and_then(|s| {
                        s.camera_topics()
                            .iter()
                            .find(|(_, t)| *t == topic)
                            .map(|(id, _)| *id)
                    });
                    if id.is_none() {
                        self.error = format!(
                            "Camera topic {topic} is not selected in this session; reopen with a layout that requests it"
                        );
                        continue;
                    }
                    if let (Some(id), Some(session)) = (id, &mut self.session) {
                        match session.select_camera(id) {
                            Ok(true) => {
                                if let Err(error) = self.bind_cameras(Some((&panel_id, &topic))) {
                                    self.error = error;
                                }
                            }
                            Ok(false) => self.error = "Camera stream unavailable".into(),
                            Err(error) => self.error = error.to_string(),
                        }
                    }
                }
                ViewerAction::SetAccumulatePoints {
                    panel_id,
                    accumulate,
                } => {
                    if let Some(Panel::Scene(p)) = self.panels.get_mut(&panel_id) {
                        p.state.accumulate_points = accumulate;
                    }
                }
                ViewerAction::SelectInspectorTopic(topic) => {
                    if let Some(topic) = topic {
                        self.inspection.select(topic);
                    } else {
                        self.inspection.invalidate();
                        self.inspection.selected = None;
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
                    if self.preview_playing.is_none()
                        && let Some(session) = &mut self.session
                    {
                        let playing = session.clock().is_playing();
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
                ViewerAction::AddNumericSeries { .. }
                | ViewerAction::RemoveNumericSeries { .. } => {}
            }
        }
    }
    fn tick(&mut self, elapsed: Duration) {
        self.poll_files();
        let focused = self.focused_camera();
        let mut seeked = false;
        if let Some(session) = &mut self.session {
            session.set_focused_camera(focused);
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
            if let Some(session) = &self.session {
                self.inspection.settle(session);
                for controller in self.pinned_inspections.values_mut() {
                    controller.settle(session);
                }
            }
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
            self.inspection.poll(session, false);
            for controller in self.pinned_inspections.values_mut() {
                controller.poll(session, false);
            }
            let active = usize::from(self.inspection.in_flight())
                + self
                    .pinned_inspections
                    .values()
                    .filter(|q| q.in_flight())
                    .count();
            let mut slots = 2usize.saturating_sub(active);
            if slots > 0 && !self.inspection.in_flight() {
                self.inspection.poll(session, true);
                slots = slots.saturating_sub(usize::from(self.inspection.in_flight()));
            }
            for _ in 0..self.pinned_order.len() {
                if slots == 0 {
                    break;
                }
                let index = self.pinned_next % self.pinned_order.len();
                self.pinned_next = (index + 1) % self.pinned_order.len();
                if let Some(controller) = self.pinned_inspections.get_mut(&self.pinned_order[index])
                    && !controller.in_flight()
                {
                    controller.poll(session, true);
                    slots = slots.saturating_sub(usize::from(controller.loading()));
                }
            }
            if !self.requirements.signals.is_empty()
                && self.speed_query.is_none()
                && self.signal_error.is_none()
            {
                match OdometryQuery::new(session, &self.active_bindings.odometry_topic) {
                    Ok(query) => self.speed_query = Some(query),
                    Err(error) => self.signal_error = Some(error),
                }
            }
            if let Some(query) = &mut self.speed_query {
                query.poll();
            }
        }
        for completed in self.bitmaps.drain() {
            match completed.image {
                Ok(bitmap) => {
                    let before = web_time::Instant::now();
                    match self
                        .gpu
                        .import(completed.frame.clone(), completed.pixel_size, &bitmap.0)
                    {
                        Ok(()) => self.metrics.record_camera(
                            completed.frame.camera_id,
                            completed.decode_time,
                            before.elapsed(),
                        ),
                        Err(error) => self.error = error,
                    }
                }
                Err(error) => self.error = error,
            }
        }
        self.metrics.advance(elapsed);
        if let Some(session) = &self.session {
            for (frame, size) in self.gpu.frames() {
                self.overlays.update(
                    frame,
                    size,
                    session.path().state().latest(),
                    session.path().state().revision(),
                    session.transforms().state(),
                    session.transforms().state().revision(),
                    &self.physical.calibrations,
                    &self.physical.projection,
                );
            }
            let overlay_status = self
                .overlays
                .snapshots()
                .map(|s| (s.camera_id, s.status.clone()))
                .collect();
            self.presentation =
                ViewerPresentation::from_features(viewer_core::ViewerPresentationInput {
                    cameras: session.cameras().state(),
                    telemetry: session.odometry().state(),
                    path: session.path().state(),
                    point_cloud: session.point_cloud().map(|p| p.point_cloud()),
                    camera_topics: session.camera_topics(),
                    focused_camera: focused,
                    overlays: &overlay_status,
                    diagnostics: viewer_core::DiagnosticsPresentation {
                        source: session.source_label().into(),
                        primary_topic: self
                            .presentation
                            .focused_camera()
                            .map_or(String::new(), |c| c.topic.clone()),
                        counters: session.counters(),
                        playback_performance: Some(session.performance()),
                        performance: self.metrics.snapshot().clone(),
                        ..Default::default()
                    },
                });
        }
    }
}
impl eframe::App for ViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let ui_started = web_time::Instant::now();
        let now = js_sys::Date::now();
        let elapsed = Duration::from_secs_f64(((now - self.previous) / 1000.).clamp(0., 0.25));
        self.previous = now;
        self.tick(elapsed);
        self.controls.clear();
        egui::TopBottomPanel::top("source").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                let valid = self.picker.settings.borrow().resolve().is_ok();
                let busy = self.picker.busy.get();
                if ui
                    .add_enabled(valid && !busy, egui::Button::new("Open MCAP"))
                    .clicked()
                {
                    self.picker.open();
                }
                let settings = ui.button("Settings");
                self.track("settings", &settings);
                if settings.clicked() {
                    self.settings_open = !self.settings_open;
                }
                if !self.source.is_empty() {
                    ui.label(&self.source);
                }
                if busy {
                    ui.spinner();
                    if self.picker.remote.is_loading() {
                        let cancel = ui.button("Cancel remote request");
                        self.track("remote-cancel", &cancel);
                        if cancel.clicked() {
                            self.picker.remote.cancel();
                        }
                    }
                }
                let mut show_plot = self.panels.values().any(|p| matches!(p, Panel::Plot(_)));
                let checkbox = ui.checkbox(&mut show_plot, "Speed plot");
                self.plot_rect = Some(checkbox.rect);
                if checkbox.changed() {
                    self.toggle_plot(show_plot);
                    ctx.request_repaint();
                }
                egui::ComboBox::from_id_salt("layout")
                    .selected_text(&self.layout_label)
                    .show_ui(ui, |ui| {
                        for (label, json) in [
                            (
                                "Monitor",
                                include_str!("../../../config/layouts/web_monitor.json"),
                            ),
                            (
                                "Standard",
                                include_str!("../../../config/layouts/native_default.json"),
                            ),
                            (
                                "Cameras",
                                include_str!("../../../config/layouts/native_cameras.json"),
                            ),
                            (
                                "Inspection",
                                include_str!("../../../config/layouts/native_inspection.json"),
                            ),
                        ] {
                            if ui
                                .selectable_label(self.layout_label == label, label)
                                .clicked()
                            {
                                match LayoutDocument::from_json(json) {
                                    Ok(layout) => {
                                        if let Err(error) =
                                            self.replace_layout(layout, label.into())
                                        {
                                            self.error = error;
                                        }
                                    }
                                    Err(error) => self.error = error.to_string(),
                                }
                            }
                        }
                    });
            });
            if !self.error.is_empty() {
                ui.colored_label(egui::Color32::LIGHT_RED, &self.error);
            }
            if let Err(error) = self.picker.settings.borrow().resolve() {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    format!("Next recording blocked: {error}"),
                );
            }
        });
        if self.settings_open {
            self.settings(ctx);
        }
        egui::TopBottomPanel::bottom("playback").show(ctx, |ui| {
            if let Some(session) = &mut self.session {
                let mut view = session.clock().view();
                view.cursor = session.timeline_cursor();
                ui.spacing_mut().slider_width = (ui.available_width() - 80.).max(80.);
                let output = viewer_ui::playback::playback_controls(ui, view);
                self.play_rect = Some(output.play_rect);
                self.timeline_rect = Some(output.timeline_rect);
                for command in output.commands {
                    if let PlaybackCommand::Seek(target) = command
                        && target == session.timeline_cursor()
                        && !matches!(
                            session.load_state(),
                            viewer_core::PlaybackLoadState::Failed { .. }
                        )
                    {
                        continue;
                    }
                    if matches!(command, PlaybackCommand::Toggle) {
                        self.resume_after_seek = false;
                    }
                    if let Err(error) = session.apply_command(command) {
                        self.error = error.to_string();
                    }
                }
                ui.label(format!("{:?}", session.load_state()));
            }
        });
        let mobile = ctx.content_rect().width() < 700.;
        if mobile {
            egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (id, panel) in self.panels.iter() {
                        if ui
                            .selectable_label(
                                self.mobile_panel.as_ref() == Some(id),
                                panel.kind_name(),
                            )
                            .clicked()
                        {
                            self.mobile_panel = Some(id.clone());
                        }
                    }
                });
            });
        }
        let mut scene_state = std::mem::take(&mut self.scene);
        let scene_style = self
            .panels
            .values()
            .find_map(|p| match p {
                Panel::Scene(p) => Some(p.state.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let empty = PointCloudState::default();
        let scene_snapshot = self.session.as_ref().map(|session| {
            scene_state.build(ScenePresentationInput {
                path: session.path().state(),
                telemetry: session.odometry().state(),
                point_cloud: session.point_cloud().map_or(&empty, |p| p.point_cloud()),
                transforms: session.transforms().state(),
                platform: self
                    .physical
                    .platform
                    .as_ref()
                    .filter(|_| self.physical.enabled),
                path_projection: &self.physical.projection,
                accumulate: scene_style.accumulate_points,
                coloring: scene_style.coloring,
            })
        });
        use viewer_ui::panel_input::{
            PanelInput, PanelResourceView, PreviewDataView, SceneDataView, SignalInput,
            SignalInputs,
        };
        // Collect only lightweight borrowed metadata once, irrespective of panel count.
        let textures = self.gpu.texture_views().collect::<Vec<_>>();
        let inspections = self
            .pinned_inspections
            .values()
            .filter_map(Inspection::view)
            .collect::<Vec<_>>();
        let topic_inspections = self.inspection.view().into_iter().collect::<Vec<_>>();
        let input = self.session.as_ref().map(|session| {
            let playback = session.clock().view();
            let query = self.speed_query.as_ref();
            let error = self
                .signal_error
                .as_ref()
                .or_else(|| query.and_then(|q| q.error.as_ref()));
            let signal =
                |id| SignalInput {
                    signal: query.and_then(|q| match id {
                        viewer_core::SignalId::Speed => q.result.speed.as_ref(),
                        viewer_core::SignalId::YawRate => q.result.yaw_rate.as_ref(),
                    }),
                    current: session.odometry().state().latest().map(|f| {
                        viewer_core::SignalSample {
                            arrival_time: f.arrival_time,
                            measurement_time: Some(f.measurement_time),
                            value: match id {
                                viewer_core::SignalId::Speed => f.speed,
                                viewer_core::SignalId::YawRate => f.yaw_rate,
                            },
                        }
                    }),
                    loading: query.is_some_and(|q| q.loading),
                    error: error.map(|e| e as &dyn std::fmt::Display),
                };
            PanelInput {
                playback: Some(playback),
                display_time: Some(self.preview_time.unwrap_or(playback.cursor)),
                presentation: &self.presentation,
                camera_overlays: &self.overlays,
                signals: SignalInputs {
                    speed: signal(viewer_core::SignalId::Speed),
                    yaw_rate: signal(viewer_core::SignalId::YawRate),
                },
                preview: PreviewDataView {
                    active: false,
                    speed_overview: None,
                    bookmarks: &[],
                },
                resources: PanelResourceView {
                    camera_textures: &textures,
                    preview_camera_textures: &[],
                    bev_texture: self.gpu.bev_id,
                    scene_texture: self.gpu.scene_id,
                },
                scene: scene_snapshot.as_ref().map(|scene| SceneDataView {
                    diagnostics: &scene.diagnostics,
                    visible_scan_points: scene.cloud.len(),
                    camera_distance: self.gpu.scene.camera().distance,
                    camera_mode: self.gpu.scene.camera_mode(),
                    static_transform_count: session.transforms().state().static_len(),
                    dynamic_transform_count: session.transforms().state().dynamic_len(),
                }),
                inspections: &inspections,
                topic_inspections: &topic_inspections,
                inspector_topic: self.inspection.selected.as_deref(),
                topic_tree: &self.topic_tree,
                numeric_series: None,
            }
        });
        let show = |ui: &mut egui::Ui, panel: &mut Panel| match &input {
            Some(input) => panel.show(ui, input),
            None => {
                ui.label("Open an indexed MCAP");
                PanelOutput::default()
            }
        };
        let mut panels = std::mem::take(&mut self.panels);
        let layout = self.layout.clone();
        let output = egui::CentralPanel::default()
            .show(ctx, |ui| {
                if mobile {
                    let selected = self
                        .mobile_panel
                        .clone()
                        .or_else(|| panels.iter().next().map(|(id, _)| id.clone()));
                    selected
                        .and_then(|id| panels.get_mut(&id))
                        .map_or_else(PanelOutput::default, |panel| show(ui, panel))
                } else {
                    viewer_ui::layout::show_layout(ui, &layout, &mut panels, |ui, panel| {
                        show(ui, panel)
                    })
                }
            })
            .inner;
        self.render_views(
            &output.render_requests,
            scene_snapshot.as_ref(),
            ctx.pixels_per_point(),
        );
        self.panels = panels;
        self.scene = scene_state;
        self.plot_chart_rect = output.render_requests.plot_rect;
        if let Some(session) = &self.session {
            for (id, size) in &output.render_requests.camera_sizes {
                if let Some(frame) = session.cameras().state().latest_for(*id)
                    && let Err(error) = self.bitmaps.submit(frame, *size)
                {
                    self.error = error;
                }
            }
        }
        self.apply_actions(output.actions);
        // Host update/paint-command staging, excluding GPU completion/presentation.
        self.metrics.record_render(ui_started.elapsed());
        let focused = self.focused_camera();
        self.bitmaps.start_pending(focused);
        let remote = self.picker.remote.connection.borrow();
        let remote_address = remote.as_ref().map(|r| r.address.as_str());
        let catalog_id = remote
            .as_ref()
            .and_then(|r| r.catalog.as_ref())
            .map(|c| c.recording_id.as_str());
        let mut snapshot = if let Some(session) = &self.session {
            let view = session.clock().view();
            let rect = |r: egui::Rect| [r.min.x, r.min.y, r.max.x, r.max.y];
            serde_json::json!({
                "source":self.source,"cursor":view.cursor.0.to_string(),"start":view.start.0.to_string(),"end":view.end.0.to_string(),"playing":view.playing,"load":format!("{:?}",session.load_state()),"error":self.error,
                "uploads":self.gpu.uploads,"uploads_by_camera":self.gpu.uploads_by_camera,"camera":focused.map(|id|id.0),"camera_texture":focused.and_then(|id|self.gpu.camera(id)).is_some(),"camera_pixels":focused.and_then(|id|self.gpu.camera(id)).map(|(_,size)|[size.0,size.1]),"camera_count":self.gpu.texture_views().count(),
                "inspector":self.inspection.status,"inspector_topic":self.inspection.selected,"has_message":self.inspection.message.is_some(),
                "device":self.gpu.device_type,"play":self.play_rect.map(rect),"timeline":self.timeline_rect.map(rect),"plot_checkbox":self.plot_rect.map(rect),"plot_chart":self.plot_chart_rect.map(rect),
                "plot_points":self.speed_query.as_ref().and_then(|q|q.result.speed.as_ref()).map_or(0,|s|s.display.values.len()),"plot_loading":self.speed_query.as_ref().is_some_and(|q|q.loading),"plot_requests":self.speed_query.as_ref().map_or(0,|q|q.request_count()),"plot_error":self.signal_error.as_ref().or_else(||self.speed_query.as_ref().and_then(|q|q.error.as_ref())),
                "layout":self.layout_label,"panel_count":self.panels.len(),"bindings":self.active_bindings,"overlay_count":self.overlays.snapshots().count(),"calibration":self.physical.calibration_label,
                "controls":self.controls,"pending_bindings_error":self.picker.settings.borrow().resolve().err(),
                "remote_status":*self.picker.remote.status.borrow(),  "pinned_inspectors":self.pinned_inspections.iter().map(|(topic,i)| (topic,serde_json::json!({"message":i.message.is_some(),"loading":i.loading(),"error":i.error}))).collect::<std::collections::BTreeMap<_,_>>(),
                "yaw_points":self.speed_query.as_ref().and_then(|q|q.result.yaw_rate.as_ref()).map_or(0,|s|s.display.values.len()),

            })
        } else {
            serde_json::json!({
                    "source":null,"error":self.error,"busy":self.picker.busy.get(),
                    "layout":self.layout_label,"panel_count":self.panels.len(),"controls":self.controls,
                    "pending_bindings_error":self.picker.settings.borrow().resolve().err(),
                    "remote_status":*self.picker.remote.status.borrow(),
            })
        };
        let extra_fields = serde_json::json!({
            "inspector_target":self.inspection.view().map(|v|v.target.0.to_string()),
            "inspector_loading":self.inspection.loading(),"inspector_error":self.inspection.error,
            "inspector_automatic":self.inspection.automatic,
            "scene_layer_uploads":self.gpu.scene.metrics().layer_uploads,
            "bev_layer_uploads":self.gpu.bev.metrics().layer_uploads,
            "remote_connected_url":remote_address,"remote_catalog":catalog_id,
            "remote_selected":self.picker.remote.selected,"remote_loading":self.picker.remote.is_loading(),
            "source_revision":self.source_revision,"panel_kinds":self.panels.values().map(Panel::kind_name).collect::<Vec<_>>(),
            "camera_choices":self.panels.values().find_map(|p|match p {Panel::Camera(p)=>Some(p.available_topics().iter().take(32).collect::<Vec<_>>()),_=>None}),
        });
        if let serde_json::Value::Object(fields) = extra_fields {
            snapshot
                .as_object_mut()
                .expect("diagnostic object")
                .extend(fields);
        }
        STATUS.with(|status| *status.borrow_mut() = snapshot.to_string());
        // A network request can hang while playback is paused and Settings is closed.
        // Drive its deadline independently of spinner animation or user input.
        if self.picker.remote.is_loading() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self
            .session
            .as_ref()
            .is_some_and(|s| s.clock().is_playing() || s.has_pending_work())
            || self.inspection.loading()
            || self.pinned_inspections.values().any(Inspection::loading)
            || self.bitmaps.has_pending_work()
            || self.speed_query.as_ref().is_some_and(|q| q.loading)
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
