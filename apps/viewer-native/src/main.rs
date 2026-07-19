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
use viewer_core::{
    ArrivalTime, BevState, CameraId, CameraState, DomainUpdate, McapSource, PipelineSet,
    PlaybackClock, PointCloudState, StreamBinding, TelemetryState, TransformState,
};
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
const PATH_TOPIC: &str = "/planning/path";
const ODOM_TOPIC: &str = "/odom";
const SCAN_TOPIC: &str = "/scan";
const TF_TOPIC: &str = "/tf";
const TF_STATIC_TOPIC: &str = "/tf_static";
const TF_SEEK_PREROLL_NS: i64 = 1_000_000_000;

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
    pipelines: PipelineSet,
    clock: Option<PlaybackClock>,
    camera: CameraState,
    bev: BevState,
    telemetry: TelemetryState,
    point_cloud: PointCloudState,
    transforms: TransformState,
    tf_misses: u64,
    last_tf_route: Option<String>,
    topic: String,
    source_name: String,
}

enum SessionSource {
    Mcap(Box<McapSource<Mmap>>),
    #[cfg(feature = "ros2-live")]
    Ros(live::RosLiveHandle),
}

impl PlaybackSession {
    fn open(path: &Path, topic: String) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the mapping is read-only and owns an independent reference to the file pages.
        let mapping =
            unsafe { Mmap::map(&file) }.with_context(|| format!("map {}", path.display()))?;
        let source = McapSource::new(mapping)?;
        let descriptor = source
            .catalog()
            .by_topic(&topic)
            .with_context(|| format!("topic {topic} is not present"))?;
        let mut bindings = vec![(descriptor.id, StreamBinding::Camera(CameraId(0)))];
        if let Some(path) = source.catalog().by_topic(PATH_TOPIC) {
            bindings.push((path.id, StreamBinding::Path));
        }
        if let Some(odometry) = source.catalog().by_topic(ODOM_TOPIC) {
            bindings.push((odometry.id, StreamBinding::Odometry));
        }
        if let Some(scan) = source.catalog().by_topic(SCAN_TOPIC) {
            bindings.push((scan.id, StreamBinding::LaserScan));
        }
        if let Some(transforms) = source.catalog().by_topic(TF_TOPIC) {
            bindings.push((
                transforms.id,
                StreamBinding::Transforms { is_static: false },
            ));
        }
        if let Some(transforms) = source.catalog().by_topic(TF_STATIC_TOPIC) {
            bindings.push((transforms.id, StreamBinding::Transforms { is_static: true }));
        }
        let pipelines = PipelineSet::new(&source.catalog().streams, &bindings);
        let (start, end) = source.time_range();
        let mut clock = PlaybackClock::new(start, end);
        clock.play();
        Ok(Self {
            source: SessionSource::Mcap(Box::new(source)),
            pipelines,
            clock: Some(clock),
            camera: CameraState::default(),
            bev: BevState::default(),
            telemetry: TelemetryState::default(),
            point_cloud: PointCloudState::default(),
            transforms: TransformState::default(),
            tf_misses: 0,
            last_tf_route: None,
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
            source: SessionSource::Ros(live::RosLiveHandle::start(topic.clone(), reliable)),
            pipelines,
            clock: None,
            camera: CameraState::default(),
            bev: BevState::default(),
            telemetry: TelemetryState::default(),
            point_cloud: PointCloudState::default(),
            transforms: TransformState::default(),
            tf_misses: 0,
            last_tf_route: None,
            topic,
            source_name: format!(
                "ROS 2 live ({})",
                if reliable { "reliable" } else { "best effort" }
            ),
        }
    }

    fn tick(&mut self, elapsed: std::time::Duration) -> Result<()> {
        let generation = self.camera.generation();
        let bev_generation = self.bev.generation();
        let telemetry_generation = self.telemetry.generation();
        let point_cloud_generation = self.point_cloud.generation();
        let mut updates = vec![];
        match &mut self.source {
            SessionSource::Mcap(source) => {
                let cursor = self
                    .clock
                    .as_mut()
                    .expect("MCAP has a clock")
                    .advance(elapsed);
                for message in source.read_until(cursor)? {
                    self.pipelines.decode(message.raw, &mut updates);
                }
            }
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros(source) => {
                if let Some(error) = source.error() {
                    bail!("ROS executor: {error}");
                }
                if let Some(message) = source.take() {
                    self.pipelines.decode(message, &mut updates);
                }
            }
        }
        for update in updates {
            match update {
                DomainUpdate::Camera(frame) => {
                    self.camera.apply(generation, frame);
                }
                DomainUpdate::Path(frame) => {
                    self.bev.apply(bev_generation, frame);
                }
                DomainUpdate::Telemetry(frame) => {
                    self.telemetry.apply(telemetry_generation, frame);
                }
                DomainUpdate::PointCloud(frame) => {
                    let target_frame = self
                        .telemetry
                        .latest()
                        .map_or("odom", |telemetry| telemetry.frame_id.as_str());
                    let source_frame = frame.frame_id.clone();
                    let Some(points) = self.transforms.transform_points_at(
                        &source_frame,
                        target_frame,
                        frame.measurement_time,
                        &frame.points,
                    ) else {
                        self.tf_misses = self.tf_misses.saturating_add(1);
                        continue;
                    };
                    let mut frame = frame;
                    frame.points = points;
                    frame.frame_id = target_frame.to_owned();
                    self.last_tf_route = Some(format!("{source_frame} → {target_frame}"));
                    self.point_cloud.apply(point_cloud_generation, frame);
                }
                DomainUpdate::Transforms(batch) => {
                    self.transforms.apply(batch);
                }
            }
        }
        Ok(())
    }

    fn seek(&mut self) -> Result<()> {
        if let (SessionSource::Mcap(source), Some(clock)) = (&mut self.source, &self.clock) {
            let cursor = clock.cursor();
            self.camera.cold_seek();
            self.bev.cold_seek();
            self.telemetry.cold_seek();
            self.point_cloud.cold_seek();
            self.transforms.clear_dynamic();

            let start = source.time_range().0;
            let pre_roll = ArrivalTime(cursor.0.saturating_sub(TF_SEEK_PREROLL_NS).max(start.0));
            source.seek(pre_roll)?;
            let mut transform_updates = Vec::new();
            for message in source.read_until(cursor)? {
                if message.topic == TF_TOPIC || message.topic == TF_STATIC_TOPIC {
                    self.pipelines.decode(message.raw, &mut transform_updates);
                }
            }
            for update in transform_updates {
                if let DomainUpdate::Transforms(batch) = update {
                    self.transforms.apply(batch);
                }
            }
            // Rewind to the requested cursor after the internal TF pre-roll so
            // ordinary domain messages at the cursor remain visible to playback.
            source.seek(cursor)?;
        }
        Ok(())
    }

    fn presentation(&self, error: Option<String>) -> ViewerPresentation {
        #[cfg(feature = "ros2-live")]
        let source_name = match &self.source {
            SessionSource::Ros(source) => {
                let age = self.camera.latest().and_then(|frame| {
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
                    source.received(),
                    source.coalesced(),
                    source.copied_bytes() / 1024
                )
            }
            SessionSource::Mcap(_) => self.source_name.clone(),
        };
        #[cfg(not(feature = "ros2-live"))]
        let source_name = self.source_name.clone();
        ViewerPresentation {
            source: source_name,
            topic: self.topic.clone(),
            camera_status: self.camera.status(),
            counters: self.pipelines.counters(),
            telemetry: self.telemetry.latest().map(|frame| TelemetryPresentation {
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

    fn clear_camera(&mut self) {
        self.camera_slot.clear();
        self.uploaded_arrival = None;
    }

    fn render(
        &mut self,
        window: &Window,
        session: &mut PlaybackSession,
        error: Option<String>,
    ) -> Result<bool, wgpu::SurfaceError> {
        let input = self.egui_state.take_egui_input(window);
        let model = session.presentation(error);
        let texture_id = self.camera_texture_id;
        let texture_size = self.camera_slot.size();
        let bev_texture_id = self.bev_texture_id;
        let scene_texture_id = self.scene_texture_id;
        let bev_path_points = self.bev_path_points(session).map_or(0, <[_]>::len);
        let current_scan_points = session
            .point_cloud
            .latest()
            .map_or(0, |frame| frame.points.len());
        let visible_scan_points = self.scene_renderer.visible_points();
        let scene_camera_distance = self.scene_renderer.camera().distance;
        let mut accumulate_points = self.accumulate_points;
        let tf_status = session.last_tf_route.as_ref().map_or_else(
            || format!("TF waiting · misses {}", session.tf_misses),
            |route| {
                format!(
                    "TF {route} · static {} dynamic {} · misses {}",
                    session.transforms.static_len(),
                    session.transforms.dynamic_len(),
                    session.tf_misses
                )
            },
        );
        let mut bev_logical_size = egui::Vec2::ZERO;
        let mut scene_logical_size = egui::Vec2::ZERO;
        let mut scene_wheel_delta = 0.0_f32;
        let mut scene_orbit_delta = egui::Vec2::ZERO;
        let mut reset_scene_camera = false;
        let mut seeked = false;
        let output = self.egui_context.run(input, |context| {
            if let Some(clock) = &mut session.clock {
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
                        if let (Some(id), Some((width, height))) = (texture_id, texture_size) {
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
        self.accumulate_points = accumulate_points;
        if scene_wheel_delta != 0.0 {
            self.scene_renderer.zoom(scene_wheel_delta);
        }
        if scene_orbit_delta != egui::Vec2::ZERO {
            self.scene_renderer
                .orbit(scene_orbit_delta.x, scene_orbit_delta.y);
        }
        if reset_scene_camera {
            self.scene_renderer.reset_camera();
        }
        self.egui_state
            .handle_platform_output(window, output.platform_output);

        let pixels_per_point = output.pixels_per_point;
        let bev_width = (bev_logical_size.x * pixels_per_point)
            .round()
            .clamp(1.0, 4096.0) as u32;
        let bev_height = (bev_logical_size.y * pixels_per_point)
            .round()
            .clamp(1.0, 4096.0) as u32;
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
            revision: session.bev.revision(),
            path: self.bev_path_points(session).unwrap_or(&[]),
        };
        if bev_resized || self.bev_renderer.needs_render(bev_frame) {
            self.bev_renderer
                .render(&self.device, &self.queue, bev_frame);
        }
        let scene_width = (scene_logical_size.x * pixels_per_point)
            .round()
            .clamp(1.0, 4096.0) as u32;
        let scene_height = (scene_logical_size.y * pixels_per_point)
            .round()
            .clamp(1.0, 4096.0) as u32;
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
        let telemetry = session.telemetry.latest();
        let telemetry_revision = telemetry.map_or(0, |frame| frame.arrival_time.0 as u64);
        let scene_frame = SceneFrame {
            revision: session.bev.revision().rotate_left(17) ^ telemetry_revision,
            cloud_revision: session.point_cloud.revision(),
            ego_position: telemetry.map_or([0.0, 0.0], |frame| {
                [frame.position_x as f32, frame.position_y as f32]
            }),
            ego_yaw: telemetry.map_or(0.0, |frame| frame.yaw_radians as f32),
            path: self.bev_path_points(session).unwrap_or(&[]),
            cloud: session
                .point_cloud
                .latest()
                .map_or(&[], |frame| frame.points.as_slice()),
            accumulate: self.accumulate_points,
        };
        if scene_resized || self.scene_renderer.needs_render(scene_frame) {
            self.scene_renderer
                .render(&self.device, &self.queue, scene_frame);
        }
        for (id, delta) in &output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, delta);
        }
        let paint_jobs = self
            .egui_context
            .tessellate(output.shapes, output.pixels_per_point);
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
        for id in &output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        Ok(seeked)
    }

    fn bev_path_points<'a>(&self, session: &'a PlaybackSession) -> Option<&'a [[f32; 2]]> {
        session.bev.latest().map(|frame| frame.points.as_slice())
    }
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
                    graphics.clear_camera();
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
                        if let Err(error) = graphics.upload_latest(&session.camera) {
                            self.error = Some(error.to_string());
                            session.camera.set_error();
                        }
                        match graphics.render(&window, session, self.error.clone()) {
                            Ok(seeked) => {
                                if seeked {
                                    if let Err(error) = session.seek() {
                                        self.error = Some(error.to_string());
                                    }
                                    graphics.clear_camera();
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
