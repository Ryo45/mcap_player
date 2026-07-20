mod live;

use anyhow::{Context, Result, bail};
use bev_renderer::{BevFrame, BevRenderer};
use egui_wgpu::{Renderer as EguiRenderer, RendererOptions, ScreenDescriptor};
use memmap2::Mmap;
use scene_renderer::{SceneFrame, SceneRenderer};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
#[cfg(feature = "ros2-live")]
use viewer_core::{CameraId, PipelineSet, StreamBinding};
use viewer_core::{CameraState, DomainState, McapPlayback, PipelineCounters, PlaybackClock};
use viewer_renderer::{CameraTextureSlot, decode_jpeg};
use viewer_ui::{TelemetryPresentation, ViewerPresentation, playback_controls, source_status};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

const DEFAULT_FIXTURE: &str = "tests/fixtures/camera-jpeg/camera_front_3s.mcap";
const DEFAULT_TOPIC: &str = "/camera/front/image/compressed";

#[derive(Debug)]
enum SourceMode {
    Mcap,
    #[cfg(feature = "ros2-live")]
    Ros {
        reliable: bool,
    },
}

struct Args {
    mcap: PathBuf,
    topic: String,
    mode: SourceMode,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut mcap = PathBuf::from(DEFAULT_FIXTURE);
        let mut topic = DEFAULT_TOPIC.to_owned();
        let mut live = false;
        let mut reliable = false;
        let mut values = std::env::args().skip(1);
        while let Some(value) = values.next() {
            match value.as_str() {
                "--mcap" => mcap = PathBuf::from(values.next().context("--mcap needs a path")?),
                "--camera-topic" => {
                    topic = values.next().context("--camera-topic needs a topic")?
                }
                "--help" | "-h" => {
                    println!(
                        "viewer-native [--mcap PATH] [--camera-topic TOPIC] [--live [--reliable]]\n\nFiles can also be dropped onto the window."
                    );
                    std::process::exit(0);
                }
                "--live" => live = true,
                "--reliable" => reliable = true,
                unknown => bail!("unknown argument: {unknown}"),
            }
        }
        #[cfg(feature = "ros2-live")]
        let mode = if live {
            SourceMode::Ros { reliable }
        } else {
            SourceMode::Mcap
        };
        #[cfg(not(feature = "ros2-live"))]
        let mode = {
            if live || reliable {
                bail!(
                    "--live requires `cargo run -p viewer-native --features ros2-live -- --live`"
                );
            }
            SourceMode::Mcap
        };
        Ok(Self { mcap, topic, mode })
    }
}

struct PlaybackSession {
    source: SessionSource,
    topic: String,
    source_name: String,
}

enum SessionSource {
    Mcap(Box<McapPlayback<Mmap>>),
    #[cfg(feature = "ros2-live")]
    Ros {
        handle: live::RosLiveHandle,
        pipelines: PipelineSet,
        state: DomainState,
    },
}

impl PlaybackSession {
    fn open(path: &Path, topic: String) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the mapping is read-only and owns an independent reference to the file pages.
        let mapping =
            unsafe { Mmap::map(&file) }.with_context(|| format!("map {}", path.display()))?;
        let mut playback = McapPlayback::new(mapping, &topic)?;
        playback.clock_mut().play();
        Ok(Self {
            source: SessionSource::Mcap(Box::new(playback)),
            topic,
            source_name: path.display().to_string(),
        })
    }

    #[cfg(feature = "ros2-live")]
    fn open_live(topic: String, reliable: bool) -> Self {
        let descriptor = viewer_core::StreamDescriptor {
            id: viewer_core::StreamId(1),
            topic: topic.clone(),
            schema: "sensor_msgs/msg/CompressedImage".into(),
            message_encoding: "cdr".into(),
        };
        let pipelines = PipelineSet::new(
            std::slice::from_ref(&descriptor),
            &[(descriptor.id, StreamBinding::Camera(CameraId(0)))],
        );
        Self {
            source: SessionSource::Ros {
                handle: live::RosLiveHandle::start(topic.clone(), reliable),
                pipelines,
                state: DomainState::default(),
            },
            topic,
            source_name: format!(
                "ROS 2 live ({})",
                if reliable { "reliable" } else { "best effort" }
            ),
        }
    }

    fn tick(&mut self, elapsed: std::time::Duration) -> Result<()> {
        match &mut self.source {
            SessionSource::Mcap(playback) => playback.tick(elapsed)?,
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros {
                handle,
                pipelines,
                state,
            } => {
                if let Some(error) = handle.error() {
                    bail!("ROS executor: {error}");
                }
                if let Some(message) = handle.take() {
                    let mut updates = Vec::new();
                    pipelines.decode(message, &mut updates);
                    state.apply_all(updates);
                }
            }
        }
        Ok(())
    }

    fn seek(&mut self) -> Result<()> {
        match &mut self.source {
            SessionSource::Mcap(playback) => {
                let cursor = playback.clock().cursor();
                playback.seek(cursor)?;
            }
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => {}
        }
        Ok(())
    }

    fn state(&self) -> &DomainState {
        match &self.source {
            SessionSource::Mcap(playback) => playback.state(),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { state, .. } => state,
        }
    }

    fn state_mut(&mut self) -> &mut DomainState {
        match &mut self.source {
            SessionSource::Mcap(playback) => playback.state_mut(),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { state, .. } => state,
        }
    }

    fn clock_mut(&mut self) -> Option<&mut PlaybackClock> {
        match &mut self.source {
            SessionSource::Mcap(playback) => Some(playback.clock_mut()),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => None,
        }
    }

    fn counters(&self) -> PipelineCounters {
        match &self.source {
            SessionSource::Mcap(playback) => playback.counters(),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { pipelines, .. } => pipelines.counters(),
        }
    }

    fn presentation(&self, error: Option<String>) -> ViewerPresentation {
        #[cfg(feature = "ros2-live")]
        let source_name = match &self.source {
            SessionSource::Ros { handle, state, .. } => {
                let age = state.camera.latest().and_then(|frame| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()?;
                    let now = i64::try_from(now.as_nanos()).ok()?;
                    Some((now - frame.arrival_time.0).max(0) as f64 / 1e9)
                });
                let freshness = age.map_or_else(
                    || "waiting".to_owned(),
                    |value| {
                        format!(
                            "age {value:.2}s · {}",
                            if value > 1.0 { "stale" } else { "live" }
                        )
                    },
                );
                format!(
                    "{} · {} · received {} · coalesced {} · CDR copy {} KiB",
                    self.source_name,
                    freshness,
                    handle.received(),
                    handle.coalesced(),
                    handle.copied_bytes() / 1024
                )
            }
            SessionSource::Mcap(_) => self.source_name.clone(),
        };
        #[cfg(not(feature = "ros2-live"))]
        let source_name = self.source_name.clone();
        ViewerPresentation {
            source: source_name,
            topic: self.topic.clone(),
            camera_status: self.state().camera.status(),
            counters: self.counters(),
            telemetry: self
                .state()
                .telemetry
                .latest()
                .map(|frame| TelemetryPresentation {
                    frame_id: frame.frame_id.clone(),
                    child_frame_id: frame.child_frame_id.clone(),
                    position_x: frame.position_x,
                    position_y: frame.position_y,
                    yaw_radians: frame.yaw_radians,
                    forward_velocity: frame.forward_velocity,
                    speed: frame.speed,
                    yaw_rate: frame.yaw_rate,
                }),
            error,
        }
    }
}

struct Graphics {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    egui_context: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: EguiRenderer,
    camera_slot: CameraTextureSlot,
    camera_texture_id: Option<egui::TextureId>,
    uploaded_arrival: Option<i64>,
    bev_renderer: BevRenderer,
    bev_texture_id: egui::TextureId,
    scene_renderer: SceneRenderer,
    scene_texture_id: egui::TextureId,
    accumulate_points: bool,
}

struct UiOutput {
    egui: egui::FullOutput,
    seeked: bool,
    bev_size: egui::Vec2,
    scene_size: egui::Vec2,
    accumulate_points: bool,
    scene_wheel_delta: f32,
    scene_orbit_delta: egui::Vec2,
    reset_scene_camera: bool,
}

impl Graphics {
    async fn new(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("viewer-native device"),
                required_features: wgpu::Features::empty(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                required_limits: wgpu::Limits::default(),
                memory_hints: Default::default(),
                trace: wgpu::Trace::Off,
            })
            .await?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(capabilities.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        let egui_context = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_context.clone(),
            egui_context.viewport_id(),
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let mut egui_renderer = EguiRenderer::new(&device, format, RendererOptions::default());
        let mut bev_renderer = BevRenderer::new(&device, 512, 512);
        bev_renderer.render(&device, &queue, BevFrame::default());
        let bev_texture_id = egui_renderer.register_native_texture(
            &device,
            bev_renderer.view(),
            wgpu::FilterMode::Linear,
        );
        let mut scene_renderer = SceneRenderer::new(&device, 768, 384);
        scene_renderer.render(&device, &queue, SceneFrame::default());
        let scene_texture_id = egui_renderer.register_native_texture(
            &device,
            scene_renderer.view(),
            wgpu::FilterMode::Linear,
        );
        Ok(Self {
            surface,
            device,
            queue,
            config,
            egui_context,
            egui_state,
            egui_renderer,
            camera_slot: CameraTextureSlot::default(),
            camera_texture_id: None,
            uploaded_arrival: None,
            bev_renderer,
            bev_texture_id,
            scene_renderer,
            scene_texture_id,
            accumulate_points: false,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    fn upload_latest(&mut self, session: &CameraState) -> Result<()> {
        let Some(frame) = session.latest() else {
            return Ok(());
        };
        if self.uploaded_arrival == Some(frame.arrival_time.0) {
            return Ok(());
        }
        let image = decode_jpeg(&frame.jpeg)?;
        let recreated = self.camera_slot.update(&self.device, &self.queue, &image);
        if recreated {
            let view = self.camera_slot.view().expect("updated slot has a view");
            if let Some(id) = self.camera_texture_id {
                self.egui_renderer.update_egui_texture_from_wgpu_texture(
                    &self.device,
                    view,
                    wgpu::FilterMode::Linear,
                    id,
                );
            } else {
                self.camera_texture_id = Some(self.egui_renderer.register_native_texture(
                    &self.device,
                    view,
                    wgpu::FilterMode::Linear,
                ));
            }
        }
        self.uploaded_arrival = Some(frame.arrival_time.0);
        Ok(())
    }

    fn hide_camera(&mut self) {
        self.uploaded_arrival = None;
    }

    fn render(
        &mut self,
        window: &Window,
        session: &mut PlaybackSession,
        error: Option<String>,
    ) -> Result<bool, wgpu::SurfaceError> {
        let ui = self.build_ui(window, session, error);
        self.accumulate_points = ui.accumulate_points;
        if ui.scene_wheel_delta != 0.0 {
            self.scene_renderer.zoom(ui.scene_wheel_delta);
        }
        if ui.scene_orbit_delta != egui::Vec2::ZERO {
            self.scene_renderer
                .orbit(ui.scene_orbit_delta.x, ui.scene_orbit_delta.y);
        }
        if ui.reset_scene_camera {
            self.scene_renderer.reset_camera();
        }
        let pixels_per_point = ui.egui.pixels_per_point;
        self.sync_bev(ui.bev_size, session, pixels_per_point);
        self.sync_scene(ui.scene_size, session, pixels_per_point);
        self.paint_egui(window, ui.egui)?;
        Ok(ui.seeked)
    }

    fn build_ui(
        &mut self,
        window: &Window,
        session: &mut PlaybackSession,
        error: Option<String>,
    ) -> UiOutput {
        let input = self.egui_state.take_egui_input(window);
        let model = session.presentation(error);
        let texture_id = self.camera_texture_id;
        let texture_size = self.camera_slot.size();
        let camera_available = session.state().camera.latest().is_some();
        let bev_texture_id = self.bev_texture_id;
        let scene_texture_id = self.scene_texture_id;
        let bev_path_points = self.bev_path_points(session).map_or(0, <[_]>::len);
        let current_scan_points = session
            .state()
            .point_cloud
            .latest()
            .map_or(0, |frame| frame.points.len());
        let visible_scan_points = self.scene_renderer.visible_points();
        let scene_camera_distance = self.scene_renderer.camera().distance;
        let mut accumulate_points = self.accumulate_points;
        let tf_status = session.state().last_tf_route.as_ref().map_or_else(
            || format!("TF waiting · misses {}", session.state().tf_misses),
            |route| {
                format!(
                    "TF {route} · static {} dynamic {} · misses {}",
                    session.state().transforms.static_len(),
                    session.state().transforms.dynamic_len(),
                    session.state().tf_misses
                )
            },
        );
        let mut bev_logical_size = egui::Vec2::ZERO;
        let mut scene_logical_size = egui::Vec2::ZERO;
        let mut scene_wheel_delta = 0.0_f32;
        let mut scene_orbit_delta = egui::Vec2::ZERO;
        let mut reset_scene_camera = false;
        let mut seeked = false;
        let egui = self.egui_context.run(input, |context| {
            if let Some(clock) = session.clock_mut() {
                egui::TopBottomPanel::bottom("playback-controls").show(context, |ui| {
                    seeked = playback_controls(ui, clock).seeked;
                });
            } else {
                egui::TopBottomPanel::bottom("live-status").show(context, |ui| {
                    ui.label("Live mode · timeline and playback clock disabled");
                });
            }
            egui::SidePanel::left("source-status")
                .resizable(true)
                .default_width(260.0)
                .show(context, |ui| source_status(ui, &model));
            egui::CentralPanel::default().show(context, |ui| {
                let top_size = egui::vec2(ui.available_width(), ui.available_height() * 0.52);
                ui.allocate_ui(top_size, |ui| {
                    ui.columns(2, |columns| {
                        columns[0].heading("JPEG CAMERA");
                        columns[0].separator();
                        if let (true, Some(id), Some((width, height))) =
                            (camera_available, texture_id, texture_size)
                        {
                            let available = columns[0].available_size();
                            let scale = (available.x / width as f32)
                                .min(available.y / height as f32)
                                .max(0.0);
                            let size = egui::vec2(width as f32 * scale, height as f32 * scale);
                            columns[0].centered_and_justified(|ui| {
                                ui.add(egui::Image::new((id, size)));
                            });
                        } else {
                            columns[0].centered_and_justified(|ui| {
                                ui.vertical_centered(|ui| {
                                    ui.spinner();
                                    ui.label("Waiting for camera frame");
                                });
                            });
                        }

                        columns[1].heading(format!("BEV · PATH {bev_path_points} pts"));
                        columns[1].separator();
                        bev_logical_size = columns[1].available_size().max(egui::vec2(1.0, 1.0));
                        columns[1].add(egui::Image::new((bev_texture_id, bev_logical_size)));
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.heading(format!("3D VIEW · SCAN {current_scan_points} pts"));
                    ui.separator();
                    ui.checkbox(&mut accumulate_points, "Accumulate scans");
                    if accumulate_points {
                        ui.label(format!("visible {visible_scan_points}"));
                    }
                    ui.label(format!("camera {scene_camera_distance:.1} m"));
                    ui.label(tf_status.as_str());
                });
                scene_logical_size = ui.available_size().max(egui::vec2(1.0, 1.0));
                let response = ui
                    .add(
                        egui::Image::new((scene_texture_id, scene_logical_size))
                            .sense(egui::Sense::drag()),
                    )
                    .on_hover_text("Wheel: zoom · Drag: orbit · Double-click: reset");
                if response.hovered() {
                    scene_wheel_delta = ui.input(|input| input.smooth_scroll_delta.y);
                }
                if response.dragged_by(egui::PointerButton::Primary) {
                    scene_orbit_delta = ui.input(|input| input.pointer.delta());
                }
                reset_scene_camera = response.double_clicked();
            });
        });
        UiOutput {
            egui,
            seeked,
            bev_size: bev_logical_size,
            scene_size: scene_logical_size,
            accumulate_points,
            scene_wheel_delta,
            scene_orbit_delta,
            reset_scene_camera,
        }
    }

    fn sync_bev(
        &mut self,
        logical_size: egui::Vec2,
        session: &PlaybackSession,
        pixels_per_point: f32,
    ) {
        let (bev_width, bev_height) = texture_size(logical_size, pixels_per_point);
        let bev_resized = self
            .bev_renderer
            .resize(&self.device, bev_width, bev_height);
        if bev_resized {
            self.egui_renderer.update_egui_texture_from_wgpu_texture(
                &self.device,
                self.bev_renderer.view(),
                wgpu::FilterMode::Linear,
                self.bev_texture_id,
            );
        }
        let bev_frame = BevFrame {
            revision: session.state().bev.revision(),
            path: self.bev_path_points(session).unwrap_or(&[]),
        };
        if bev_resized || self.bev_renderer.needs_render(bev_frame) {
            self.bev_renderer
                .render(&self.device, &self.queue, bev_frame);
        }
    }

    fn sync_scene(
        &mut self,
        logical_size: egui::Vec2,
        session: &PlaybackSession,
        pixels_per_point: f32,
    ) {
        let (scene_width, scene_height) = texture_size(logical_size, pixels_per_point);
        let scene_resized = self
            .scene_renderer
            .resize(&self.device, scene_width, scene_height);
        if scene_resized {
            self.egui_renderer.update_egui_texture_from_wgpu_texture(
                &self.device,
                self.scene_renderer.view(),
                wgpu::FilterMode::Linear,
                self.scene_texture_id,
            );
        }
        let telemetry = session.state().telemetry.latest();
        let telemetry_revision = telemetry.map_or(0, |frame| frame.arrival_time.0 as u64);
        let scene_frame = SceneFrame {
            revision: session.state().bev.revision().rotate_left(17) ^ telemetry_revision,
            cloud_revision: session.state().point_cloud.revision(),
            ego_position: telemetry.map_or([0.0, 0.0], |frame| {
                [frame.position_x as f32, frame.position_y as f32]
            }),
            ego_yaw: telemetry.map_or(0.0, |frame| frame.yaw_radians as f32),
            path: self.bev_path_points(session).unwrap_or(&[]),
            cloud: session
                .state()
                .point_cloud
                .latest()
                .map_or(&[], |frame| frame.points.as_slice()),
            accumulate: self.accumulate_points,
        };
        if scene_resized || self.scene_renderer.needs_render(scene_frame) {
            self.scene_renderer
                .render(&self.device, &self.queue, scene_frame);
        }
    }

    fn paint_egui(
        &mut self,
        window: &Window,
        output: egui::FullOutput,
    ) -> Result<(), wgpu::SurfaceError> {
        let egui::FullOutput {
            platform_output,
            textures_delta,
            shapes,
            pixels_per_point,
            ..
        } = output;
        self.egui_state
            .handle_platform_output(window, platform_output);
        for (id, delta) in &textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, delta);
        }
        let paint_jobs = self.egui_context.tessellate(shapes, pixels_per_point);
        let screen = ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: window.scale_factor() as f32,
        };
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("viewer-native encoder"),
            });
        let callbacks = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen,
        );
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("viewer-native egui pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.015,
                                g: 0.02,
                                b: 0.025,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            self.egui_renderer.render(&mut pass, &paint_jobs, &screen);
        }
        self.queue
            .submit(callbacks.into_iter().chain([encoder.finish()]));
        frame.present();
        for id in &textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        Ok(())
    }

    fn bev_path_points<'a>(&self, session: &'a PlaybackSession) -> Option<&'a [[f32; 2]]> {
        session
            .state()
            .bev
            .latest()
            .map(|frame| frame.points.as_slice())
    }
}

fn texture_size(logical_size: egui::Vec2, pixels_per_point: f32) -> (u32, u32) {
    (
        (logical_size.x * pixels_per_point)
            .round()
            .clamp(1.0, 4096.0) as u32,
        (logical_size.y * pixels_per_point)
            .round()
            .clamp(1.0, 4096.0) as u32,
    )
}

struct App {
    args: Args,
    window: Option<Arc<Window>>,
    graphics: Option<Graphics>,
    session: Option<PlaybackSession>,
    last_frame: Instant,
    error: Option<String>,
}

impl App {
    fn load(&mut self, path: &Path) {
        match PlaybackSession::open(path, self.args.topic.clone()) {
            Ok(session) => {
                self.args.mcap = path.to_owned();
                self.session = Some(session);
                self.error = None;
                if let Some(graphics) = &mut self.graphics {
                    graphics.hide_camera();
                }
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("MCAP Camera + BEV + 3D")
            .with_inner_size(winit::dpi::LogicalSize::new(1200.0, 860.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.error = Some(error.to_string());
                event_loop.exit();
                return;
            }
        };
        match pollster::block_on(Graphics::new(window.clone())) {
            Ok(graphics) => self.graphics = Some(graphics),
            Err(error) => {
                self.error = Some(error.to_string());
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
        match self.args.mode {
            SourceMode::Mcap => {
                let path = self.args.mcap.clone();
                self.load(&path);
            }
            #[cfg(feature = "ros2-live")]
            SourceMode::Ros { reliable } => {
                self.session = Some(PlaybackSession::open_live(
                    self.args.topic.clone(),
                    reliable,
                ));
            }
        }
        self.last_frame = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if let Some(graphics) = &mut self.graphics {
            let response = graphics.egui_state.on_window_event(&window, &event);
            if response.repaint {
                window.request_redraw();
            }
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::DroppedFile(path) => self.load(&path),
            WindowEvent::Resized(size) => {
                if let Some(graphics) = &mut self.graphics {
                    graphics.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let elapsed = now.saturating_duration_since(self.last_frame);
                self.last_frame = now;
                if let Some(session) = &mut self.session {
                    if let Err(error) = session.tick(elapsed) {
                        self.error = Some(error.to_string());
                    }
                    if let Some(graphics) = &mut self.graphics {
                        if let Err(error) = graphics.upload_latest(&session.state().camera) {
                            self.error = Some(error.to_string());
                            session.state_mut().camera.set_error();
                        }
                        match graphics.render(&window, session, self.error.clone()) {
                            Ok(seeked) => {
                                if seeked {
                                    if let Err(error) = session.seek() {
                                        self.error = Some(error.to_string());
                                    }
                                    graphics.hide_camera();
                                }
                            }
                            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                                graphics.resize(graphics.config.width, graphics.config.height)
                            }
                            Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                            Err(error) => self.error = Some(error.to_string()),
                        }
                    }
                }
                window.request_redraw();
            }
            _ => {}
        }
    }
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse()?;
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        args,
        window: None,
        graphics: None,
        session: None,
        last_frame: Instant::now(),
        error: None,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
