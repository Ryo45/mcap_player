use crate::{
    args::{Args, SourceMode},
    graphics::{Graphics, RenderInput},
    interaction::ViewerAction,
    plot_loader::PlotLoader,
    presentation::PresentationState,
    session::PlaybackSession,
    workspace::WorkspaceState,
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
    pub(crate) workspace: WorkspaceState,
    pub(crate) plot_loader: PlotLoader,
    pub(crate) presentation_state: PresentationState,
    pub(crate) graphics: Option<Graphics>,
    pub(crate) last_frame: Instant,
    pub(crate) error: Option<String>,
}

impl App {
    fn load(&mut self, path: &Path) {
        match PlaybackSession::open(path, self.args.topic.clone()) {
            Ok(mut session) => {
                let plot_origin = session
                    .playback_view()
                    .expect("MCAP session has a playback view")
                    .start;
                self.workspace
                    .reset_for_source(session.default_focused_camera());
                session.set_focused_camera(self.workspace.camera.focused_camera);
                self.plot_loader.clear();
                if let Err(error) =
                    self.plot_loader
                        .start_speed_overview(path.to_owned(), plot_origin, 4_000)
                {
                    self.error = Some(error.to_string());
                }
                self.args.mcap = path.to_owned();
                self.session = Some(session);
                if self.plot_loader.error().is_none() {
                    self.error = None;
                }
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
        self.plot_loader.poll();

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
                &self.workspace.camera,
                &self.workspace.scene,
                self.error.clone(),
            );
            let render_started = Instant::now();
            let result = graphics.render(
                window,
                RenderInput {
                    presentation: &presentation.viewer,
                    playback,
                    speed_signal: self.plot_loader.signal(),
                    plot_loading: self.plot_loader.is_loading(),
                    plot_error: self.plot_loader.error(),
                    bev: presentation.bev,
                    scene: &presentation.scene,
                    static_transform_count: presentation.static_transform_count,
                    dynamic_transform_count: presentation.dynamic_transform_count,
                },
                &mut self.workspace,
            );
            (result, render_started.elapsed())
        };
        self.presentation_state.record_render(render_elapsed);
        self.presentation_state.advance_metrics(elapsed);

        let output = render_result?;
        let _view_requests = output.view_requests;
        let seeked = Self::apply_actions(
            session,
            &mut self.workspace,
            &mut self.error,
            output.actions,
        );
        if seeked {
            self.presentation_state.reset();
            graphics.hide_camera();
            graphics.clear_scene_history();
        }
        Ok(())
    }

    fn apply_actions(
        session: &mut PlaybackSession,
        workspace: &mut WorkspaceState,
        error: &mut Option<String>,
        actions: Vec<ViewerAction>,
    ) -> bool {
        let mut seeked = false;
        for action in actions {
            let focused_camera_changed = matches!(action, ViewerAction::SetFocusedCamera(_));
            if let Some(command) = workspace.apply_action(action) {
                match session.apply_playback_command(command) {
                    Ok(command_seeked) => seeked |= command_seeked,
                    Err(command_error) => *error = Some(command_error.to_string()),
                }
            }
            if focused_camera_changed {
                session.set_focused_camera(workspace.camera.focused_camera);
            }
        }
        seeked
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
                self.plot_loader.clear();
                self.workspace
                    .reset_for_source(session.default_focused_camera());
                session.set_focused_camera(self.workspace.camera.focused_camera);
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

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{ArrivalTime, PlaybackCommand};

    fn session() -> PlaybackSession {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/camera-jpeg/camera_front_3s.mcap");
        PlaybackSession::open(&path, "/camera/front/image/compressed".to_owned()).unwrap()
    }

    #[test]
    fn playback_action_is_applied_to_the_session() {
        let mut session = session();
        let mut workspace = WorkspaceState::default();
        let mut error = None;
        assert!(session.playback_view().unwrap().playing);
        let seeked = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut error,
            vec![ViewerAction::Playback(PlaybackCommand::Toggle)],
        );
        assert!(!seeked);
        assert!(!session.playback_view().unwrap().playing);
        assert!(error.is_none());
    }

    #[test]
    fn preview_action_does_not_seek_playback() {
        let mut session = session();
        let cursor = session.playback_view().unwrap().cursor;
        let preview = ArrivalTime(cursor.0 + 123);
        let mut workspace = WorkspaceState::default();
        let mut error = None;
        let seeked = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut error,
            vec![ViewerAction::SetPreviewTime(Some(preview))],
        );
        assert!(!seeked);
        assert_eq!(workspace.interaction.preview_time, Some(preview));
        assert_eq!(session.playback_view().unwrap().cursor, cursor);
        assert!(error.is_none());
    }
}
