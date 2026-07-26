use crate::{
    args::{Args, SourceMode},
    graphics::{Graphics, RenderInput},
    session::PlaybackSession,
};
use std::{fs, path::Path, sync::Arc, time::Instant};
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
    pub(crate) graphics: Option<Graphics>,
    pub(crate) session: Option<PlaybackSession>,
    pub(crate) last_frame: Instant,
    pub(crate) error: Option<String>,
}

impl App {
    fn load(&mut self, path: &Path) {
        match PlaybackSession::open(path, self.args.topic.clone()) {
            Ok(session) => {
                self.args.mcap = path.to_owned();
                self.session = Some(session);
                self.error = None;
                if let Some(graphics) = &mut self.graphics {
                    graphics.reset_camera_presentation();
                    graphics.clear_scene_history();
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
                        if let Err(error) = graphics.upload_latest(session.state()) {
                            let camera_id = error.camera_id;
                            self.error = Some(error.to_string());
                            session.state_mut().camera.set_error_for(camera_id);
                        }
                        let presentation = session.presentation(
                            self.error.clone(),
                            graphics.presentation_snapshot(),
                            graphics.overlay_status(),
                        );
                        let playback = session.playback_view();
                        let render_started = Instant::now();
                        let render_result = graphics.render(
                            &window,
                            RenderInput {
                                state: session.state(),
                                presentation: &presentation,
                                playback,
                            },
                        );
                        graphics.record_render(render_started.elapsed());
                        graphics.advance_presentation_metrics(elapsed);
                        match render_result {
                            Ok(output) => {
                                session.set_focused_camera(output.focused_camera);
                                let mut seeked = false;
                                for command in output.playback_commands {
                                    match session.apply_playback_command(command) {
                                        Ok(command_seeked) => seeked |= command_seeked,
                                        Err(error) => self.error = Some(error.to_string()),
                                    }
                                }
                                if seeked {
                                    graphics.hide_camera();
                                    graphics.clear_scene_history();
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
