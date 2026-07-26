use crate::{
    args::{Args, SourceMode},
    graphics::{Graphics, RenderInput},
    presentation::PresentationState,
    session::PlaybackSession,
    settings::ViewerSettings,
};
use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use viewer_core::CameraCalibrationSet;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::ActiveEventLoop,
    window::{Window, WindowId},
};

pub(crate) struct App {
    pub(crate) args: Args,
    pub(crate) window: Option<Arc<Window>>,
    pub(crate) session: Option<PlaybackSession>,
    pub(crate) viewer_settings: ViewerSettings,
    pub(crate) presentation_state: PresentationState,
    pub(crate) graphics: Option<Graphics>,
    pub(crate) last_frame: Instant,
    pub(crate) error: Option<String>,
}

impl App {
    fn load(&mut self, path: &Path) {
        match PlaybackSession::open(path, self.args.topic.clone()) {
            Ok(mut session) => {
                self.viewer_settings.focused_camera = session.default_focused_camera();
                session.set_focused_camera(self.viewer_settings.focused_camera);
                self.args.mcap = path.to_owned();
                self.session = Some(session);
                self.error = None;
                self.presentation_state.reset();
                if let Some(graphics) = &mut self.graphics {
                    graphics.hide_camera();
                    graphics.clear_scene_history();
                }
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn redraw(&mut self, window: &Window, elapsed: Duration) -> Result<(), wgpu::SurfaceError> {
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        if let Err(error) = session.tick(elapsed) {
            self.error = Some(error.to_string());
        }

        let Some(graphics) = &mut self.graphics else {
            return Ok(());
        };
        let mut camera_updates = Vec::new();
        let upload_result = {
            let state = session.state();
            graphics.upload_latest(
                &state.camera,
                state.bev.latest(),
                &state.transforms,
                &mut camera_updates,
            )
        };
        if let Err(error) = upload_result {
            let camera_id = error.camera_id;
            self.error = Some(error.to_string());
            session.state_mut().camera.set_error_for(camera_id);
        }
        self.presentation_state
            .record_camera_updates(camera_updates);

        let diagnostics = session.diagnostics();
        let playback = session.playback_view();
        let (render_result, render_elapsed) = {
            let presentation = self.presentation_state.build(
                session.state(),
                diagnostics,
                &self.viewer_settings,
                self.error.clone(),
            );
            let render_started = Instant::now();
            let result = graphics.render(
                window,
                RenderInput {
                    presentation: &presentation.viewer,
                    playback,
                    settings: &self.viewer_settings,
                    bev: presentation.bev,
                    scene: &presentation.scene,
                    static_transform_count: presentation.static_transform_count,
                    dynamic_transform_count: presentation.dynamic_transform_count,
                },
            );
            (result, render_started.elapsed())
        };
        self.presentation_state.record_render(render_elapsed);
        self.presentation_state.advance_metrics(elapsed);

        let output = render_result?;
        self.viewer_settings.focused_camera = output.focused_camera;
        self.viewer_settings.accumulate_points = output.accumulate_points;
        session.set_focused_camera(self.viewer_settings.focused_camera);

        let mut seeked = false;
        for command in output.playback_commands {
            match session.apply_playback_command(command) {
                Ok(command_seeked) => seeked |= command_seeked,
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        if seeked {
            self.presentation_state.reset();
            graphics.hide_camera();
            graphics.clear_scene_history();
        }
        Ok(())
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
        let calibration_json = match fs::read_to_string(&self.args.calibration) {
            Ok(json) => json,
            Err(error) => {
                self.error = Some(format!(
                    "read camera calibration {}: {error}",
                    self.args.calibration.display()
                ));
                event_loop.exit();
                return;
            }
        };
        let calibrations = match CameraCalibrationSet::from_json(&calibration_json) {
            Ok(calibrations) => calibrations,
            Err(error) => {
                self.error = Some(error.to_string());
                event_loop.exit();
                return;
            }
        };
        match pollster::block_on(Graphics::new(window.clone(), calibrations)) {
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
                let mut session = PlaybackSession::open_live(self.args.topic.clone(), reliable);
                self.viewer_settings.focused_camera = session.default_focused_camera();
                session.set_focused_camera(self.viewer_settings.focused_camera);
                self.session = Some(session);
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
                match self.redraw(&window, elapsed) {
                    Ok(()) => {}
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        if let Some(graphics) = &mut self.graphics {
                            graphics.resize(graphics.config.width, graphics.config.height);
                        }
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                    Err(error) => self.error = Some(error.to_string()),
                }
                window.request_redraw();
            }
            _ => {}
        }
    }
}
