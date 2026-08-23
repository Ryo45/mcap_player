use crate::{
    args::{Args, SourceMode},
    bookmarks::BookmarkState,
    diagnostics::AppDiagnostics,
    graphics::{Graphics, RenderInput},
    interaction::ViewerAction,
    presentation::{PresentationBuildInput, PresentationState, PresentationTransition},
    preview::{PreviewCoordinator, fingerprint_source},
    session::ViewerSession,
    workspace::{NativeWorkspace, WorkspaceEffect},
};
use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use viewer_core::CameraCalibrationSet;
use viewer_core::PlaybackEffect;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::ActiveEventLoop,
    window::{Window, WindowId},
};

pub(crate) struct App {
    pub(crate) args: Args,
    pub(crate) window: Option<Arc<Window>>,
    pub(crate) session: Option<ViewerSession>,
    pub(crate) workspace: NativeWorkspace,
    pub(crate) preview: PreviewCoordinator,
    pub(crate) bookmarks: BookmarkState,
    pub(crate) presentation_state: PresentationState,
    pub(crate) graphics: Option<Graphics>,
    pub(crate) last_frame: Instant,
    pub(crate) diagnostics: AppDiagnostics,
}

impl App {
    fn load(&mut self, path: &Path) {
        let requirements = self.workspace.data_requirements();
        match ViewerSession::open(
            path,
            self.args.topic.clone(),
            &requirements.playback,
            self.workspace.bindings(),
        ) {
            Ok(mut session) => {
                self.diagnostics.reset_for_source();
                self.workspace.configure_session(session.plan());
                let scheduler_camera = Self::scheduler_camera_for(&self.workspace, &session);
                self.workspace.reset_for_source(scheduler_camera);
                if !requirements.signals.is_empty()
                    && let Err(error) = session.request_plot_signals(4_000)
                {
                    log::warn!("Plot unavailable: {error}");
                }
                if let Err(error) = session.request_inspections(&requirements.inspections) {
                    log::warn!("Inspector unavailable: {error}");
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
                Self::apply_presentation_transition(
                    &mut self.presentation_state,
                    self.graphics.as_mut(),
                    PresentationTransition::SourceChanged,
                );
            }
            Err(error) => self.diagnostics.set_playback_error(error.to_string()),
        }
    }

    fn redraw(&mut self, window: &Window, elapsed: Duration) -> Result<(), wgpu::SurfaceError> {
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        if let Err(error) = session.tick(elapsed, |elapsed, messages| {
            self.workspace.process_messages(elapsed, messages);
        }) {
            self.diagnostics.set_playback_error(error.to_string());
        }
        session.poll_queries();

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
        let upload_result =
            { graphics.upload_latest(self.workspace.cameras().state(), &mut camera_updates) };
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
        self.presentation_state.update_camera_overlays(
            self.workspace.cameras(),
            self.workspace.path(),
            self.workspace.transforms(),
            &camera_base_images,
        );

        let playback = session.playback_view();
        let playback_performance = playback.map(|_| {
            self.workspace
                .playback_performance(session.source_read_timing())
        });
        let diagnostics = session.diagnostics(
            self.workspace.counters(),
            playback_performance,
            self.workspace
                .cameras()
                .state()
                .latest_by_arrival()
                .map(|frame| frame.arrival_time),
        );
        let current_odometry = self
            .workspace
            .interaction
            .preview_time
            .is_none()
            .then(|| self.workspace.odometry().state().latest())
            .flatten();
        let signals = session.signal_query_view(current_odometry);
        let workspace_warning = self.workspace.startup_warning();
        let error = self.diagnostics.message(&[
            workspace_warning.as_deref(),
            signals.first_error(),
            self.preview.warning(),
            self.bookmarks.warning(),
        ]);
        let focused_camera = self.workspace.focused_camera();
        let accumulate_points = self.workspace.accumulate_points();
        let (render_result, render_elapsed) = {
            let runtime = self
                .workspace
                .runtime
                .as_ref()
                .expect("workspace is configured for an open session");
            let presentation = self.presentation_state.build(PresentationBuildInput {
                cameras: runtime.cameras(),
                path: runtime.path(),
                odometry: runtime.odometry(),
                transforms: runtime.transforms(),
                scene_controller: runtime.scene(),
                diagnostics,
                focused_camera,
                accumulate_points,
                error,
            });
            let render_started = Instant::now();
            let result = graphics.render(
                window,
                RenderInput {
                    presentation: &presentation.viewer,
                    camera_overlays: presentation.camera_overlays,
                    playback,
                    signals,
                    inspections: session.inspections(),
                    preview: self.preview.snapshot(),
                    preview_speed: self.preview.speed_overview(),
                    bookmarks: self.bookmarks.bookmarks(),
                    bev: presentation.bev,
                    scene: &presentation.scene,
                    static_transform_count: presentation.static_transform_count,
                    dynamic_transform_count: presentation.dynamic_transform_count,
                },
                &self.workspace.layout,
                &mut self.workspace.panels,
                &self.workspace.interaction,
            );
            (result, render_started.elapsed())
        };
        self.presentation_state.record_render(render_elapsed);
        self.presentation_state.advance_metrics(elapsed);

        let output = render_result?;
        let playback_effect = Self::apply_actions(
            session,
            &mut self.workspace,
            &mut self.preview,
            &mut self.diagnostics,
            output.actions,
        );
        if playback_effect == PlaybackEffect::Seeked {
            Self::apply_presentation_transition(
                &mut self.presentation_state,
                Some(graphics),
                PresentationTransition::Seeked,
            );
        }
        Ok(())
    }

    fn apply_presentation_transition(
        presentation_state: &mut PresentationState,
        graphics: Option<&mut Graphics>,
        transition: PresentationTransition,
    ) {
        presentation_state.apply_transition(transition);
        if let Some(graphics) = graphics {
            graphics.apply_transition(transition);
        }
    }

    fn scheduler_camera_for(
        workspace: &NativeWorkspace,
        session: &ViewerSession,
    ) -> Option<viewer_core::CameraId> {
        let default = session.default_focused_camera();
        let Some(topic) = workspace.scheduler_priority_topic() else {
            return default;
        };
        session.camera_id_for_topic(topic).or_else(|| {
            log::warn!(
                "configured scheduler-priority Camera topic {topic:?} is unavailable; using the session primary Camera"
            );
            default
        })
    }

    fn apply_actions(
        session: &mut ViewerSession,
        workspace: &mut NativeWorkspace,
        preview: &mut PreviewCoordinator,
        diagnostics: &mut AppDiagnostics,
        actions: Vec<ViewerAction>,
    ) -> PlaybackEffect {
        let mut playback_effect = PlaybackEffect::None;
        for action in actions {
            match workspace.apply_action(action) {
                WorkspaceEffect::Playback(command) => match session
                    .apply_playback_command(command, |target, messages| {
                        workspace.restore_messages(target, messages)
                    }) {
                    Ok(PlaybackEffect::Seeked) => playback_effect = PlaybackEffect::Seeked,
                    Ok(PlaybackEffect::None) => {}
                    Err(command_error) => {
                        diagnostics.set_playback_error(command_error.to_string());
                    }
                },
                WorkspaceEffect::BeginPreview(time) => {
                    let playing = session
                        .playback_view()
                        .is_some_and(|playback| playback.playing);
                    if preview.drag.begin(playing)
                        && let Err(command_error) = session
                            .apply_playback_command(viewer_core::PlaybackCommand::Toggle, |_, _| {
                                Ok(())
                            })
                    {
                        diagnostics.set_playback_error(command_error.to_string());
                    }
                    preview.update(Some(time));
                }
                WorkspaceEffect::UpdatePreview(time) => preview.update(time),
                WorkspaceEffect::CommitPreview(time) => {
                    match session.apply_playback_command(
                        viewer_core::PlaybackCommand::Seek(time),
                        |target, messages| workspace.restore_messages(target, messages),
                    ) {
                        Ok(PlaybackEffect::Seeked) => playback_effect = PlaybackEffect::Seeked,
                        Ok(PlaybackEffect::None) => {}
                        Err(command_error) => {
                            diagnostics.set_playback_error(command_error.to_string());
                        }
                    }
                    if preview.drag.finish()
                        && let Err(command_error) = session
                            .apply_playback_command(viewer_core::PlaybackCommand::Toggle, |_, _| {
                                Ok(())
                            })
                    {
                        diagnostics.set_playback_error(command_error.to_string());
                    }
                    preview.update(None);
                }
                WorkspaceEffect::None => {}
            }
        }
        playback_effect
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
                let requirements = self.workspace.data_requirements();
                let mut session = ViewerSession::open_live(
                    self.args.topic.clone(),
                    reliable,
                    &requirements.playback,
                    self.workspace.bindings(),
                );
                self.workspace.configure_session(session.plan());
                if let Err(error) = session.request_inspections(&requirements.inspections) {
                    log::warn!("Inspector unavailable: {error}");
                }
                self.preview.clear();
                self.bookmarks = BookmarkState::default();
                Self::apply_presentation_transition(
                    &mut self.presentation_state,
                    self.graphics.as_mut(),
                    PresentationTransition::SourceChanged,
                );
                let scheduler_camera = Self::scheduler_camera_for(&self.workspace, &session);
                self.workspace.reset_for_source(scheduler_camera);
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
    use viewer_core::{ArrivalTime, PlaybackCommand, PlaybackRequirements};

    fn standard_test_requirements() -> PlaybackRequirements {
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_all_cameras();
        requirements
    }

    fn session() -> ViewerSession {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/camera-jpeg/camera_front_3s.mcap");
        ViewerSession::open(
            &path,
            "/camera/front/image/compressed".to_owned(),
            &standard_test_requirements(),
            NativeWorkspace::default().bindings(),
        )
        .unwrap()
    }

    fn configured_workspace(session: &ViewerSession) -> NativeWorkspace {
        let mut workspace = NativeWorkspace::default();
        workspace.configure_session(session.plan());
        workspace.reset_for_source(session.default_focused_camera());
        workspace
    }

    #[test]
    fn playback_action_is_applied_to_the_session() {
        let mut session = session();
        let mut workspace = NativeWorkspace::default();
        let mut preview = PreviewCoordinator::default();
        let mut diagnostics = AppDiagnostics::default();
        assert!(session.playback_view().unwrap().playing);
        let effect = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::Playback(PlaybackCommand::Toggle)],
        );
        assert_eq!(effect, PlaybackEffect::None);
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
        let effect = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut coordinator,
            &mut diagnostics,
            vec![ViewerAction::SetPreviewTime(Some(preview))],
        );
        assert_eq!(effect, PlaybackEffect::None);
        assert_eq!(workspace.interaction.preview_time, Some(preview));
        assert_eq!(session.playback_view().unwrap().cursor, cursor);
        assert!(diagnostics.message(&[]).is_none());
    }

    #[test]
    fn seek_action_still_reaches_the_viewer_session() {
        let mut session = session();
        let playback = session.playback_view().unwrap();
        let target = ArrivalTime((playback.start.0 + playback.end.0) / 2);
        let mut workspace = configured_workspace(&session);
        let mut preview = PreviewCoordinator::default();
        let mut diagnostics = AppDiagnostics::default();
        let effect = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::Playback(PlaybackCommand::Seek(target))],
        );
        assert_eq!(effect, PlaybackEffect::Seeked);
        assert_eq!(session.playback_view().unwrap().cursor, target);
        assert!(diagnostics.message(&[]).is_none());
    }

    #[test]
    fn native_workspace_seek_restores_the_indexed_camera_predecessor() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap");
        let mut workspace = NativeWorkspace::default();
        let mut session = ViewerSession::open(
            &path,
            "/camera/front/image/compressed".to_owned(),
            &workspace.data_requirements().playback,
            workspace.bindings(),
        )
        .unwrap();
        workspace.configure_session(session.plan());
        workspace.reset_for_source(session.default_focused_camera());
        let playback = session.playback_view().unwrap();
        let target = ArrivalTime((playback.start.0 + playback.end.0) / 2);

        let bytes = std::fs::read(&path).unwrap();
        let source = viewer_core::McapSource::new(bytes).unwrap();
        let stream = source
            .catalog()
            .by_topic("/camera/front/image/compressed")
            .unwrap()
            .id;
        let expected = source
            .latest_before(&[stream], target)
            .unwrap()
            .messages
            .pop()
            .unwrap();
        let expected_image = viewer_core::decode_compressed_image_bytes(expected.payload).unwrap();

        let mut preview = PreviewCoordinator::default();
        let mut diagnostics = AppDiagnostics::default();
        let effect = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::Playback(PlaybackCommand::Seek(target))],
        );

        assert_eq!(effect, PlaybackEffect::Seeked);
        let restored = workspace
            .cameras()
            .state()
            .latest_for(session.default_focused_camera().unwrap())
            .unwrap();
        assert_eq!(restored.arrival_time, expected.arrival_time);
        assert_eq!(restored.jpeg, expected_image.jpeg);
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
        let mut workspace = configured_workspace(&session);
        let mut preview = PreviewCoordinator::default();
        let mut diagnostics = AppDiagnostics::default();

        let effect = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![
                ViewerAction::BeginPreview(first),
                ViewerAction::SetPreviewTime(Some(final_target)),
            ],
        );
        assert_eq!(effect, PlaybackEffect::None);
        assert_eq!(session.playback_view().unwrap().cursor, original);
        assert!(!session.playback_view().unwrap().playing);
        assert_eq!(workspace.interaction.preview_time, Some(final_target));

        let effect = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::CommitPreview(final_target)],
        );
        assert_eq!(effect, PlaybackEffect::Seeked);
        assert_eq!(session.playback_view().unwrap().cursor, final_target);
        assert!(session.playback_view().unwrap().playing);
        assert_eq!(workspace.interaction.preview_time, None);
        assert!(diagnostics.message(&[]).is_none());
    }
}
