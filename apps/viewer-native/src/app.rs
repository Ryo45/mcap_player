use crate::{
    args::{Args, SourceMode},
    bookmarks::BookmarkState,
    diagnostics::AppDiagnostics,
    graphics::{Graphics, RenderInput, RenderOutput},
    interaction::ViewerAction,
    presentation::{PresentationBuildInput, PresentationState, PresentationTransition},
    preview::PreviewCoordinator,
    session::ViewerSession,
    workspace::{NativeWorkspace, WorkspaceEffect},
};
use std::{
    fs,
    io::Read,
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

fn read_presentation_config(path: &Path) -> Result<String, String> {
    let mut json = String::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(64 * 1024 + 1)
        .read_to_string(&mut json)
        .map_err(|e| e.to_string())?;
    if json.len() > 64 * 1024 {
        return Err("presentation JSON exceeds 64 KiB".into());
    }
    Ok(json)
}

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
        let candidate = (|| -> anyhow::Result<_> {
            let playback = viewer_mcap::McapPlayback::open_file(path)?;
            let cameras = self
                .workspace
                .panels
                .camera_selection(playback.catalog(), &self.args.topic, None, true)
                .map_err(anyhow::Error::msg)?;
            let requirements = self.workspace.requirements_for_cameras(&cameras);
            let topic = cameras.priority_topic(&self.args.topic).to_owned();
            let session = ViewerSession::from_playback(
                path,
                playback,
                topic,
                &requirements.playback,
                self.workspace.bindings(),
            )?;
            Ok((session, cameras, requirements))
        })();
        match candidate {
            Ok((mut session, cameras, requirements)) => {
                if let Some(previous) = &mut self.session {
                    session.take_query_workers_from(previous);
                }
                self.diagnostics.reset_for_source();
                self.workspace.configure_session(session.plan());
                self.workspace
                    .reset_for_source(session.default_focused_camera());
                self.workspace.panels.bind_cameras(&cameras, session.plan());
                let scheduler_camera = Self::scheduler_camera_for(&self.workspace, &session);
                self.workspace
                    .runtime_mut()
                    .set_scheduling_priority(scheduler_camera);
                if !requirements.signals.is_empty()
                    && let Err(error) = session.request_odometry_signals(4_000)
                {
                    log::warn!("Plot unavailable: {error}");
                }
                if let Err(error) = session.request_inspections(&requirements.inspections) {
                    log::warn!("Inspector unavailable: {error}");
                }
                session.configure_window_queries(&requirements);
                session.select_inspector_topic(self.args.inspect_topic.clone());
                if let Some(topic) = &self.args.inspect_topic
                    && session.selected_inspector_topic().is_none()
                {
                    log::warn!("Initial Inspector topic {topic:?} is absent from this recording");
                }
                self.args.mcap = path.to_owned();
                let fingerprint = session.source_fingerprint();
                self.session = Some(session);
                match fingerprint {
                    Ok(fingerprint) => {
                        self.preview.load_for_source(path, &fingerprint);
                        self.preview
                            .bind_cameras(self.session.as_ref().expect("source committed").plan());
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
        if self.session.is_none() {
            return Ok(());
        }
        self.advance_session(elapsed);
        if self.graphics.is_none() {
            return Ok(());
        }
        let graphics = self.graphics.as_mut().expect("redraw requires graphics");
        let camera_queue_error = graphics
            .queue_camera_frames(self.workspace.cameras().state())
            .err()
            .map(|error| error.to_string());
        // Decoding can finish while FIFO waits for the display. Collect only after
        // that wait, so a completed image does not miss this frame's UI build.
        let acquire_started = Instant::now();
        let frame = graphics.acquire_frame()?;
        let surface_wait = acquire_started.elapsed();
        self.update_camera_presentation(camera_queue_error);
        let output = self.render_frame(window, elapsed, frame, surface_wait);
        self.apply_frame_actions(output.actions);
        Ok(())
    }

    fn advance_session(&mut self, elapsed: Duration) {
        let session = self.session.as_mut().expect("redraw requires a session");
        if let Err(error) = session.tick(elapsed, |elapsed, messages| {
            self.workspace.process_messages(elapsed, messages);
        }) {
            self.diagnostics.set_playback_error(error.to_string());
        }
        session.poll_queries();
    }

    fn update_camera_presentation(&mut self, camera_queue_error: Option<String>) {
        let graphics = self.graphics.as_mut().expect("redraw requires graphics");
        let mut presentation_errors = camera_queue_error.into_iter().collect::<Vec<_>>();
        if self.workspace.interaction.preview_time.is_some()
            && let Some(snapshot) = self.preview.snapshot()
            && let Err(error) = graphics.upload_preview(snapshot.camera_frames())
        {
            presentation_errors.push(error.to_string());
        }
        let mut camera_updates = Vec::new();
        let upload_result = graphics.upload_ready_camera_frames(&mut camera_updates);
        if let Err(error) = upload_result {
            presentation_errors.push(error.to_string());
        }
        if !presentation_errors.is_empty() {
            self.diagnostics
                .set_presentation_error(presentation_errors.join("; "));
        }
        self.presentation_state
            .record_camera_updates(camera_updates);
        let camera_base_images = graphics
            .presented_camera_frames()
            .map(|(frame, size)| (frame.clone(), size))
            .collect::<Vec<_>>();
        self.presentation_state.update_camera_overlays(
            self.workspace.path(),
            self.workspace.transforms(),
            &camera_base_images,
        );
    }

    fn render_frame(
        &mut self,
        window: &Window,
        elapsed: Duration,
        frame: wgpu::SurfaceTexture,
        surface_wait: Duration,
    ) -> RenderOutput {
        let session = self.session.as_ref().expect("redraw requires a session");
        let graphics = self.graphics.as_mut().expect("redraw requires graphics");
        let playback = session.playback_view();
        let playback_performance = playback.map(|_| {
            self.workspace
                .playback_performance(session.source_read_timing())
        });
        let diagnostics = session.diagnostics(self.workspace.counters(), playback_performance);
        let current_odometry = if self.workspace.interaction.preview_time.is_none() {
            self.workspace.odometry().state().latest()
        } else {
            None
        };
        let signals = session.signal_view(current_odometry);
        let workspace_warning = self.workspace.startup_warning();
        let signal_error = signals.first_error().map(ToString::to_string);
        let error = self.diagnostics.message(&[
            workspace_warning.as_deref(),
            signal_error.as_deref(),
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
                point_cloud_controller: runtime.point_cloud(),
                diagnostics,
                focused_camera,
                accumulate_points,
                coloring: self.workspace.point_cloud_coloring(),
                error,
            });
            let render_started = Instant::now();
            let result = graphics.render(
                window,
                frame,
                RenderInput {
                    presentation: &presentation.viewer,
                    camera_overlays: presentation.camera_overlays,
                    playback,
                    signals,
                    inspections: session.inspections(),
                    topic_health: session.topic_health(),
                    producer_diagnostics: session.producer_diagnostics(),
                    inspector_topic: session.selected_inspector_topic(),
                    topic_tree: session.topic_tree(),
                    numeric_series: session.numeric_series(),
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
        self.presentation_state
            .record_render(render_elapsed.saturating_add(surface_wait));
        self.presentation_state.advance_metrics(elapsed);

        render_result
    }

    fn apply_frame_actions(&mut self, actions: Vec<ViewerAction>) {
        let session = self.session.as_mut().expect("redraw requires a session");
        let graphics = self.graphics.as_mut().expect("redraw requires graphics");
        let preview_binding_revision = self.preview.camera_binding_revision();
        let playback_effect = Self::apply_actions(
            session,
            &mut self.workspace,
            &mut self.preview,
            &mut self.diagnostics,
            actions,
        );
        if self.preview.camera_binding_revision() != preview_binding_revision {
            graphics.clear_preview();
        }
        if playback_effect == PlaybackEffect::Seeked {
            Self::apply_presentation_transition(
                &mut self.presentation_state,
                Some(graphics),
                PresentationTransition::Seeked,
            );
        }
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
                WorkspaceEffect::SelectCameraTopic(panel_id, topic) => {
                    let result = (|| -> anyhow::Result<bool> {
                        let catalog = session.camera_catalog();
                        let cameras = workspace
                            .panels
                            .camera_selection(
                                catalog,
                                session.plan().primary_camera_topic().unwrap_or(""),
                                Some((&panel_id, &topic)),
                                false,
                            )
                            .map_err(anyhow::Error::msg)?;
                        let requirements = workspace.requirements_for_cameras(&cameras);
                        let priority = cameras.priority_topic(&topic);
                        let plan = viewer_core::SessionPlan::build(
                            catalog,
                            priority,
                            &requirements.playback,
                            workspace.bindings(),
                        )?;
                        if plan.selected_stream_ids() == session.plan().selected_stream_ids() {
                            let camera_id = session
                                .camera_id_for_topic(&topic)
                                .expect("unchanged selection contains the validated image topic");
                            workspace.runtime_mut().present_pending_camera(camera_id)?;
                            workspace.panels.bind_cameras(&cameras, session.plan());
                            let scheduler = workspace.scheduler_priority_topic().unwrap_or(&topic);
                            let scheduler =
                                session.camera_id_for_topic(scheduler).or(Some(camera_id));
                            workspace.runtime_mut().set_scheduling_priority(scheduler);
                            return Ok(false);
                        }
                        let enable_point_cloud = workspace.panels.has_scene();
                        let priority = cameras.priority_topic(&topic);
                        let mut candidate = None;
                        session.select_camera_topics(
                            priority,
                            &requirements.playback,
                            workspace.bindings(),
                            |plan, target, messages| {
                                let mut runtime =
                                    viewer_core::FeatureRuntime::new(plan, enable_point_cloud);
                                runtime.set_scheduling_priority(plan.primary_camera());
                                runtime.restore_transactional(target, &messages)?;
                                candidate = Some(runtime);
                                Ok(())
                            },
                        )?;
                        workspace.runtime = candidate;
                        workspace.panels.bind_cameras(&cameras, session.plan());
                        let scheduler = Self::scheduler_camera_for(workspace, session);
                        workspace.runtime_mut().set_scheduling_priority(scheduler);
                        workspace.interaction.preview_time = None;
                        preview.bind_cameras(session.plan());
                        Ok(true)
                    })();
                    match result {
                        Ok(true) => playback_effect = PlaybackEffect::Seeked,
                        Ok(false) => {}
                        Err(error) => diagnostics.set_playback_error(error.to_string()),
                    }
                }
                WorkspaceEffect::SelectInspectorTopic(topic) => {
                    session.select_inspector_topic(topic)
                }
                WorkspaceEffect::AddNumericSeries(stream, path) => {
                    session.add_numeric_series(stream, path)
                }
                WorkspaceEffect::RemoveNumericSeries(stream, path) => {
                    session.remove_numeric_series(stream, &path)
                }
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
        let calibrations = read_presentation_config(&self.args.calibration)
            .map_err(|e| e.to_string())
            .and_then(|json| CameraCalibrationSet::from_json(&json).map_err(|e| e.to_string()));
        let calibrations = match calibrations {
            Ok(c) => c,
            Err(e) => {
                self.diagnostics
                    .set_presentation_error(format!("Camera calibration unavailable: {e}"));
                CameraCalibrationSet::default()
            }
        };
        self.presentation_state
            .set_camera_calibrations(calibrations);
        let platform = self
            .args
            .platform_geometry
            .as_ref()
            .map(|path| {
                read_presentation_config(path)
                    .map_err(|e| e.to_string())
                    .and_then(|json| {
                        serde_json::from_str::<viewer_core::PlatformGeometry>(&json)
                            .map_err(|e| e.to_string())
                    })
            })
            .transpose();
        match platform {
            Ok(g) => self.presentation_state.set_platform(g),
            Err(e) => {
                self.diagnostics
                    .set_presentation_error(format!("Platform geometry: {e}"));
                event_loop.exit();
                return;
            }
        }
        self.presentation_state
            .set_path_projection(viewer_core::PathProjectionPolicy {
                fixed_frame: self.args.fixed_frame.clone(),
                ground_frame: self.args.ground_frame.clone(),
            });
        match pollster::block_on(Graphics::new(window.clone())) {
            Ok(graphics) => self.graphics = Some(graphics),
            Err(error) => {
                self.diagnostics.set_presentation_error(error.to_string());
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
        match &self.args.mode {
            SourceMode::Mcap => {
                let path = self.args.mcap.clone();
                self.load(&path);
            }
            #[cfg(feature = "ros2-live")]
            SourceMode::Ros {
                reliable,
                raw_camera_topics,
            } => {
                let requirements = self.workspace.data_requirements();
                let session = ViewerSession::open_live(
                    self.args.topic.clone(),
                    *reliable,
                    raw_camera_topics,
                    &requirements.playback,
                    self.workspace.bindings(),
                );
                let session = match session {
                    Ok(session) => session,
                    Err(error) => {
                        self.diagnostics.set_playback_error(error.to_string());
                        return;
                    }
                };
                self.diagnostics.reset_for_source();
                self.workspace.configure_session(session.plan());
                self.preview.clear();
                self.bookmarks = BookmarkState::default();
                Self::apply_presentation_transition(
                    &mut self.presentation_state,
                    self.graphics.as_mut(),
                    PresentationTransition::SourceChanged,
                );
                let scheduler_camera = Self::scheduler_camera_for(&self.workspace, &session);
                self.workspace.reset_for_source(scheduler_camera);
                let cameras = self
                    .workspace
                    .panels
                    .camera_selection(session.camera_catalog(), &self.args.topic, None, true)
                    .expect("initial camera selection has no requested change");
                self.workspace.panels.bind_cameras(&cameras, session.plan());
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

    fn camera_workspace() -> (NativeWorkspace, ViewerSession) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/camera-formats/camera_formats.mcap");
        let playback = viewer_mcap::McapPlayback::open_file(&path).unwrap();
        let mut workspace =
            NativeWorkspace::load_bundled_or_fallback(crate::workspace::WorkspaceLayout::Monitor);
        let cameras = workspace
            .panels
            .camera_selection(playback.catalog(), "/missing/project/camera", None, true)
            .unwrap();
        let requirements = workspace.requirements_for_cameras(&cameras);
        let session = ViewerSession::from_playback(
            &path,
            playback,
            cameras.priority_topic("/missing/project/camera").into(),
            &requirements.playback,
            workspace.bindings(),
        )
        .unwrap();
        workspace.configure_session(session.plan());
        workspace.reset_for_source(session.default_focused_camera());
        workspace.panels.bind_cameras(&cameras, session.plan());
        (workspace, session)
    }

    fn all_camera_workspace(path: &Path) -> (NativeWorkspace, ViewerSession) {
        let mut workspace = NativeWorkspace::from_json(r#"{
          "schemaVersion":1,"root":{"kind":"split","direction":"row","children":[
            {"weight":1,"node":{"kind":"panel","id":"camera","panelType":"camera","configVersion":1,"config":{"showThumbnails":true}}},
            {"weight":1,"node":{"kind":"panel","id":"scene","panelType":"scene-3d","configVersion":1,"config":{"optionalData":true}}}
          ]}}
        "#).unwrap();
        let playback = viewer_mcap::McapPlayback::open_file(path).unwrap();
        let cameras = workspace
            .panels
            .camera_selection(playback.catalog(), "", None, true)
            .unwrap();
        let requirements = workspace.requirements_for_cameras(&cameras);
        let session = ViewerSession::from_playback(
            path,
            playback,
            cameras.priority_topic("").into(),
            &requirements.playback,
            workspace.bindings(),
        )
        .unwrap();
        workspace.configure_session(session.plan());
        workspace.reset_for_source(session.default_focused_camera());
        workspace.panels.bind_cameras(&cameras, session.plan());
        (workspace, session)
    }

    #[test]
    fn focus_only_keeps_camera_ids_scene_and_clock_and_presents_pending_image() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap");
        let (mut workspace, mut session) = all_camera_workspace(&path);
        let mut preview = PreviewCoordinator::default();
        let mut diagnostics = AppDiagnostics::default();
        let view = session.playback_view().unwrap();
        let target = ArrivalTime(view.start.0 + (view.end.0 - view.start.0) / 2);
        App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::Playback(PlaybackCommand::Seek(target))],
        );
        session
            .tick(Duration::from_millis(100), |dt, messages| {
                workspace.process_messages(dt, messages)
            })
            .unwrap();
        App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::Playback(PlaybackCommand::Toggle)],
        );
        let before = session.playback_view().unwrap();
        let topics = session.plan().camera_topics();
        let (camera_id, topic) = topics.last().unwrap();
        let path_revision = workspace.path().state().revision();
        let tf_revision = workspace.transforms().state().revision();
        let cloud = workspace
            .runtime()
            .point_cloud()
            .unwrap()
            .point_cloud()
            .latest()
            .unwrap();
        let points = cloud.points.as_ptr() as usize;
        let expected = session
            .plan()
            .camera_routes()
            .iter()
            .find(|r| r.camera_id == *camera_id)
            .unwrap()
            .stream
            .id;
        let source = viewer_mcap::McapSource::open_file(&path).unwrap();
        let expected = source
            .latest_before(&[expected], before.cursor)
            .unwrap()
            .messages
            .pop()
            .unwrap()
            .arrival_time;
        let effect = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::SelectCameraTopic {
                panel_id: viewer_layout::PanelId::new("camera").unwrap(),
                topic: topic.clone(),
            }],
        );
        assert_eq!(effect, PlaybackEffect::None);
        assert_eq!(session.playback_view().unwrap(), before);
        assert_eq!(session.plan().camera_topics(), topics);
        assert_eq!(workspace.focused_camera(), Some(*camera_id));
        assert_eq!(
            workspace
                .cameras()
                .state()
                .latest_for(*camera_id)
                .unwrap()
                .arrival_time,
            expected
        );
        assert_eq!(workspace.path().state().revision(), path_revision);
        assert_eq!(workspace.transforms().state().revision(), tf_revision);
        assert_eq!(
            workspace
                .runtime()
                .point_cloud()
                .unwrap()
                .point_cloud()
                .latest()
                .unwrap()
                .points
                .as_ptr() as usize,
            points
        );
        assert!(diagnostics.message(&[]).is_none());
    }

    #[test]
    #[ignore = "manual release measurement, no timing assertion"]
    fn measure_camera_focus_actions() {
        let path = std::env::var_os("MCAP_CAMERA_FOCUS_RECORDING")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap")
            });
        let (mut workspace, mut session) = all_camera_workspace(&path);
        let topics = session.plan().camera_topics();
        assert!(topics.len() >= 2);
        let view = session.playback_view().unwrap();
        let target = ArrivalTime(view.start.0 + (view.end.0 - view.start.0) / 2);
        let mut preview = PreviewCoordinator::default();
        let mut diagnostics = AppDiagnostics::default();
        App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::Playback(PlaybackCommand::Seek(target))],
        );
        let mut times = Vec::new();
        for index in 0..40 {
            let started = Instant::now();
            App::apply_actions(
                &mut session,
                &mut workspace,
                &mut preview,
                &mut diagnostics,
                vec![ViewerAction::SelectCameraTopic {
                    panel_id: viewer_layout::PanelId::new("camera").unwrap(),
                    topic: topics[index % 2].1.clone(),
                }],
            );
            times.push(started.elapsed().as_secs_f64() * 1000.0);
            assert!(diagnostics.message(&[]).is_none());
        }
        times.sort_by(f64::total_cmp);
        println!(
            "CAMERA_FOCUS_MS median={:.6} p95={:.6} max={:.6}",
            times[20], times[38], times[39]
        );
    }

    #[test]
    fn camera_selection_restores_raw_and_compressed_at_the_cursor_and_prunes_other_images() {
        let (mut workspace, mut session) = camera_workspace();
        assert_eq!(session.plan().camera_routes().len(), 1);
        let cameras = viewer_core::camera_streams(session.camera_catalog())
            .map(|stream| (stream.topic.clone(), stream.schema.clone()))
            .collect::<Vec<_>>();
        let original = session.playback_view().unwrap();
        let target = ArrivalTime(original.start.0 + 1_550_000_000);
        let mut diagnostics = AppDiagnostics::default();
        let mut preview = PreviewCoordinator::default();
        App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![
                ViewerAction::Playback(PlaybackCommand::Toggle),
                ViewerAction::Playback(PlaybackCommand::Seek(target)),
            ],
        );
        for (topic, _) in cameras
            .iter()
            .filter(|(_, schema)| schema == "sensor_msgs/msg/CompressedImage")
            .take(1)
            .chain(
                cameras
                    .iter()
                    .filter(|(_, schema)| schema == "sensor_msgs/msg/Image")
                    .take(1),
            )
        {
            let effect = App::apply_actions(
                &mut session,
                &mut workspace,
                &mut preview,
                &mut diagnostics,
                vec![ViewerAction::SelectCameraTopic {
                    panel_id: viewer_layout::PanelId::new("monitor-front").unwrap(),
                    topic: topic.clone(),
                }],
            );
            assert_eq!(effect, PlaybackEffect::Seeked);
            let view = session.playback_view().unwrap();
            assert_eq!(view.cursor, target);
            assert!(!view.playing);
            assert_eq!(
                session.plan().selected_topics().as_slice(),
                std::slice::from_ref(topic)
            );
            let route = &session.plan().camera_routes()[0];
            let expected = session.camera_catalog().by_topic(topic).unwrap().id;
            assert_eq!(route.stream.id, expected);
            let frame = workspace
                .cameras()
                .state()
                .latest_for(route.camera_id)
                .unwrap();
            assert_eq!(
                frame.arrival_time,
                ArrivalTime(original.start.0 + 1_500_000_000)
            );
            assert_eq!(workspace.cameras().state().frames().count(), 1);
            assert!(diagnostics.message(&[]).is_none());
        }
        // Forward playback must traverse the newly selected stream after its exact predecessor.
        App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::Playback(PlaybackCommand::Toggle)],
        );
        session
            .tick(Duration::from_millis(100), |elapsed, messages| {
                workspace.process_messages(elapsed, messages)
            })
            .unwrap();
        let frame = workspace
            .cameras()
            .state()
            .latest_for(session.plan().camera_routes()[0].camera_id)
            .unwrap();
        assert_eq!(
            frame.arrival_time,
            ArrivalTime(original.start.0 + 1_600_000_000)
        );
        assert!(session.playback_view().unwrap().playing);
    }

    #[test]
    fn invalid_camera_choice_leaves_binding_clock_and_visible_frame_unchanged() {
        let (mut workspace, mut session) = camera_workspace();
        let mut diagnostics = AppDiagnostics::default();
        let mut preview = PreviewCoordinator::default();
        let target = session.playback_view().unwrap().start;
        App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::Playback(PlaybackCommand::Seek(target))],
        );
        let before = session.playback_view().unwrap();
        let topics = session.plan().selected_topics();
        let frame = workspace
            .cameras()
            .state()
            .frames()
            .next()
            .unwrap()
            .1
            .clone();
        let effect = App::apply_actions(
            &mut session,
            &mut workspace,
            &mut preview,
            &mut diagnostics,
            vec![ViewerAction::SelectCameraTopic {
                panel_id: viewer_layout::PanelId::new("monitor-front").unwrap(),
                topic: "/not-an-image".into(),
            }],
        );
        assert_eq!(effect, PlaybackEffect::None);
        assert_eq!(session.playback_view().unwrap(), before);
        assert_eq!(session.plan().selected_topics(), topics);
        assert_eq!(
            workspace.cameras().state().frames().next().unwrap().1,
            &frame
        );
        assert!(
            diagnostics
                .message(&[])
                .unwrap()
                .contains("Unsupported image topic")
        );
    }

    #[test]
    fn missing_images_and_source_replacement_do_not_keep_old_camera_requirements() {
        let (workspace, session) = camera_workspace();
        let mut catalog = session.camera_catalog().clone();
        catalog.streams.clear();
        let cameras = workspace
            .panels
            .camera_selection(&catalog, "/missing", None, true)
            .unwrap();
        assert!(cameras.available_topics.is_empty());
        let requirements = workspace.requirements_for_cameras(&cameras);
        let plan = viewer_core::SessionPlan::build(
            &catalog,
            "/missing",
            &requirements.playback,
            workspace.bindings(),
        )
        .unwrap();
        assert!(plan.camera_routes().is_empty());
        let mut catalog = session.camera_catalog().clone();
        catalog.streams.truncate(1);
        catalog.streams[0].topic = "/another/project/image".into();
        let cameras = workspace
            .panels
            .camera_selection(&catalog, "/missing", None, true)
            .unwrap();
        assert_eq!(cameras.available_topics, ["/another/project/image"]);
        assert!(
            cameras
                .panels
                .values()
                .all(|topic| topic.as_deref() == Some("/another/project/image"))
        );
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
        let source = viewer_mcap::McapSource::copy_from_slice((bytes).as_ref()).unwrap();
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
        assert_eq!(restored.data, expected_image.data);
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
