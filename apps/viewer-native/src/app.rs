use crate::{
    args::{Args, SourceMode},
    bookmarks::BookmarkState,
    diagnostics::AppDiagnostics,
    graphics::{Graphics, RenderInput},
    interaction::ViewerAction,
    plot_loader::PlotLoader,
    presentation::PresentationState,
    preview::{PreviewCoordinator, fingerprint_source},
    session::PlaybackSession,
    workspace::{NativeWorkspace, WorkspaceEffect},
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
    pub(crate) workspace: NativeWorkspace,
    pub(crate) plot_loader: PlotLoader,
    pub(crate) preview: PreviewCoordinator,
    pub(crate) bookmarks: BookmarkState,
    pub(crate) presentation_state: PresentationState,
    pub(crate) graphics: Option<Graphics>,
    pub(crate) last_frame: Instant,
    pub(crate) diagnostics: AppDiagnostics,
}

impl App {
    fn load(&mut self, path: &Path) {
        match PlaybackSession::open(path, self.args.topic.clone()) {
            Ok(mut session) => {
                self.diagnostics.reset_for_source();
                let plot_origin = session
                    .playback_view()
                    .expect("MCAP session has a playback view")
                    .start;
                self.workspace
                    .reset_for_source(session.default_focused_camera());
                session.set_focused_camera(self.workspace.focused_camera());
                self.plot_loader.clear();
                if let Err(error) =
                    self.plot_loader
                        .start_speed_overview(path.to_owned(), plot_origin, 4_000)
                {
                    log::warn!("Plot unavailable: {error}");
                }
                self.args.mcap = path.to_owned();
                self.session = Some(session);
                match fingerprint_source(path) {
                    Ok(fingerprint) => {
                        self.preview.load_for_source(path, &fingerprint);
                        self.bookmarks.load_for_source(path, &fingerprint);
                    }
                    Err(error) => {
                        self.preview.clear();
                        self.bookmarks = BookmarkState::default();
                        self.diagnostics.add_sidecar_warning(format!(
                            "Preview and bookmarks unavailable: {error}"
                        ));
                    }
                }
                self.presentation_state.reset();
                if let Some(graphics) = &mut self.graphics {
                    graphics.hide_camera();
                    graphics.clear_preview();
                    graphics.clear_scene_history();
                }
            }
            Err(error) => self.diagnostics.set_playback_error(error.to_string()),
        }
    }

    fn redraw(&mut self, window: &Window, elapsed: Duration) -> Result<(), wgpu::SurfaceError> {
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        if let Err(error) = session.tick(elapsed) {
            self.diagnostics.set_playback_error(error.to_string());
        }
        self.plot_loader.poll();

        let Some(graphics) = &mut self.graphics else {
            return Ok(());
        };
        let mut presentation_errors = Vec::new();
        if self.workspace.interaction.preview_time.is_some()
            && let Some(snapshot) = self.preview.snapshot()
            && let Err(error) = graphics.upload_preview(snapshot.camera_frames())
        {
            presentation_errors.push(error.to_string());
        }
        let mut camera_updates = Vec::new();
        let upload_result = {
            let state = session.state();
            graphics.upload_latest(&state.camera, &mut camera_updates)
        };
        if let Err(error) = upload_result {
            presentation_errors.push(error.to_string());
        }
        if !presentation_errors.is_empty() {
            self.diagnostics
                .set_presentation_error(presentation_errors.join("; "));
        }
        self.presentation_state
            .record_camera_updates(camera_updates);
        let camera_base_images = graphics.camera_base_images().collect::<Vec<_>>();
        self.presentation_state
            .update_camera_overlays(session.state(), &camera_base_images);

        let diagnostics = session.diagnostics();
        let playback = session.playback_view();
        let workspace_warning = self.workspace.startup_warning();
        let error = self.diagnostics.message(&[
            workspace_warning.as_deref(),
            self.plot_loader.error(),
            self.preview.warning(),
            self.bookmarks.warning(),
        ]);
        let (render_result, render_elapsed) = {
            let presentation = self.presentation_state.build(
                session.state(),
                diagnostics,
                self.workspace.focused_camera(),
                self.workspace.accumulate_points(),
                error,
            );
            let render_started = Instant::now();
            let result = graphics.render(
                window,
                RenderInput {
                    presentation: &presentation.viewer,
                    camera_overlays: presentation.camera_overlays,
                    playback,
                    speed_signal: self.plot_loader.signal(),
                    plot_loading: self.plot_loader.is_loading(),
                    plot_error: self.plot_loader.error(),
                    preview: self.preview.snapshot(),
                    preview_speed: self.preview.speed_overview(),
                    bookmarks: self.bookmarks.bookmarks(),
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
        let seeked = Self::apply_actions(
            session,
            &mut self.workspace,
            &mut self.preview,
            &mut self.diagnostics,
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
        workspace: &mut NativeWorkspace,
        preview: &mut PreviewCoordinator,
        diagnostics: &mut AppDiagnostics,
        actions: Vec<ViewerAction>,
    ) -> bool {
        let mut seeked = false;
        for action in actions {
            match workspace.apply_action(action) {
                WorkspaceEffect::Playback(command) => match session.apply_playback_command(command)
                {
                    Ok(command_seeked) => seeked |= command_seeked,
                    Err(command_error) => {
                        diagnostics.set_playback_error(command_error.to_string());
                    }
                },
                WorkspaceEffect::FocusedCameraChanged(camera_id) => {
                    session.set_focused_camera(camera_id);
                }
                WorkspaceEffect::BeginPreview(time) => {
                    let playing = session
                        .playback_view()
                        .is_some_and(|playback| playback.playing);
                    if preview.drag.begin(playing)
                        && let Err(command_error) =
                            session.apply_playback_command(viewer_core::PlaybackCommand::Toggle)
                    {
                        diagnostics.set_playback_error(command_error.to_string());
                    }
                    preview.update(Some(time));
                }
                WorkspaceEffect::UpdatePreview(time) => preview.update(time),
                WorkspaceEffect::CommitPreview(time) => {
                    match session.apply_playback_command(viewer_core::PlaybackCommand::Seek(time)) {
                        Ok(command_seeked) => seeked |= command_seeked,
                        Err(command_error) => {
                            diagnostics.set_playback_error(command_error.to_string());
                        }
                    }
                    if preview.drag.finish()
                        && let Err(command_error) =
                            session.apply_playback_command(viewer_core::PlaybackCommand::Toggle)
                    {
                        diagnostics.set_playback_error(command_error.to_string());
                    }
                    preview.update(None);
                }
                WorkspaceEffect::None => {}
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
                self.diagnostics.set_presentation_error(error.to_string());
                event_loop.exit();
                return;
            }
        };
        let calibration_json = match fs::read_to_string(&self.args.calibration) {
            Ok(json) => json,
            Err(error) => {
                self.diagnostics.set_presentation_error(format!(
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
                self.diagnostics.set_presentation_error(error.to_string());
                event_loop.exit();
                return;
            }
        };
        self.presentation_state
            .set_camera_calibrations(calibrations);
        match pollster::block_on(Graphics::new(window.clone())) {
            Ok(graphics) => self.graphics = Some(graphics),
            Err(error) => {
                self.diagnostics.set_presentation_error(error.to_string());
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
                self.preview.clear();
                self.bookmarks = BookmarkState::default();
                if let Some(graphics) = &mut self.graphics {
                    graphics.clear_preview();
                }
                self.workspace
                    .reset_for_source(session.default_focused_camera());
                session.set_focused_camera(self.workspace.focused_camera());
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
                    Err(error) => self.diagnostics.set_presentation_error(error.to_string()),
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
        let mut workspace = NativeWorkspace::default();
        let mut preview = PreviewCoordinator::default();
        let mut diagnostics = AppDiagnostics::default();
        assert!(session.playback_view().unwrap().playing);
        let seeked = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::Playback(PlaybackCommand::Toggle)],
        );
        assert!(!seeked);
        assert!(!session.playback_view().unwrap().playing);
        assert!(diagnostics.message(&[]).is_none());
    }

    #[test]
    fn preview_action_does_not_seek_playback() {
        let mut session = session();
        let cursor = session.playback_view().unwrap().cursor;
        let preview = ArrivalTime(cursor.0 + 123);
        let mut workspace = NativeWorkspace::default();
        let mut coordinator = PreviewCoordinator::default();
        let mut diagnostics = AppDiagnostics::default();
        let seeked = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut coordinator,
            &mut diagnostics,
            vec![ViewerAction::SetPreviewTime(Some(preview))],
        );
        assert!(!seeked);
        assert_eq!(workspace.interaction.preview_time, Some(preview));
        assert_eq!(session.playback_view().unwrap().cursor, cursor);
        assert!(diagnostics.message(&[]).is_none());
    }

    #[test]
    fn seek_action_still_reaches_the_playback_session() {
        let mut session = session();
        let playback = session.playback_view().unwrap();
        let target = ArrivalTime((playback.start.0 + playback.end.0) / 2);
        let mut workspace = NativeWorkspace::default();
        let mut preview = PreviewCoordinator::default();
        let mut diagnostics = AppDiagnostics::default();
        let seeked = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::Playback(PlaybackCommand::Seek(target))],
        );
        assert!(seeked);
        assert_eq!(session.playback_view().unwrap().cursor, target);
        assert!(diagnostics.message(&[]).is_none());
    }

    #[test]
    fn preview_drag_pauses_without_seek_and_release_seeks_once_then_resumes() {
        let mut session = session();
        let playback = session.playback_view().unwrap();
        let original = playback.cursor;
        let first = ArrivalTime((playback.start.0 * 2 + playback.end.0) / 3);
        let final_target = ArrivalTime((playback.start.0 + playback.end.0 * 2) / 3);
        let mut workspace = NativeWorkspace::default();
        let mut preview = PreviewCoordinator::default();
        let mut diagnostics = AppDiagnostics::default();

        let seeked = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![
                ViewerAction::BeginPreview(first),
                ViewerAction::SetPreviewTime(Some(final_target)),
            ],
        );
        assert!(!seeked);
        assert_eq!(session.playback_view().unwrap().cursor, original);
        assert!(!session.playback_view().unwrap().playing);
        assert_eq!(workspace.interaction.preview_time, Some(final_target));

        let seeked = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::CommitPreview(final_target)],
        );
        assert!(seeked);
        assert_eq!(session.playback_view().unwrap().cursor, final_target);
        assert!(session.playback_view().unwrap().playing);
        assert_eq!(workspace.interaction.preview_time, None);
        assert!(diagnostics.message(&[]).is_none());
    }
}
