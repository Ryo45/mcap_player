#[cfg(feature = "ros2-live")]
use crate::live;
use crate::odometry_signals::{OdometrySignalLoader, OdometrySignalRequest};
use crate::signal_view::SignalView;
use crate::{
    inspection::{InspectorRequirement, TopicInspection},
    inspector_loader::InspectorLoader,
};
use anyhow::{Context, Result};
use std::{path::Path, time::Duration};
use viewer_core::{
    ArrivalTime, CameraId, FeatureRestoreError, PlaybackCommand, PlaybackEffect,
    PlaybackPerformance, PlaybackRequirements, PlaybackView, ProcessingCounters, RawMessage,
    RestoreSemantics, SessionPlan, StageTiming, WorkspaceBindings,
};
use viewer_mcap::McapPlayback;

/// The currently open Viewer data session, backed by either Recording or Live input.
///
/// It owns source access and playback/query capabilities, but feature/controller state belongs
/// to the workspace and is updated through exact serialized messages.
pub(crate) struct ViewerSession {
    source: SessionSource,
    plan: SessionPlan,
    topic: String,
    source_name: String,
    odometry_signals: OdometrySignalLoader,
    odometry_topic: String,
    topic_tree: viewer_core::TopicTree,
    numeric_plots: crate::numeric_plot_loader::NumericPlotLoader,
    inspector_loader: InspectorLoader,
    window_queries: crate::window_query_loader::WindowQueryLoader,
    #[cfg(feature = "plugin-panels")]
    plugin_queries: crate::plugin_queries::PluginQueries,
}

enum SessionSource {
    Mcap(Box<McapPlayback>),
    #[cfg(feature = "ros2-live")]
    Ros {
        handle: live::RosLiveHandle,
        catalog: viewer_core::SourceCatalog,
    },
}

pub(crate) struct SessionDiagnostics {
    pub(crate) source_name: String,
    pub(crate) primary_topic: String,
    pub(crate) camera_topics: Vec<(CameraId, String)>,
    pub(crate) counters: ProcessingCounters,
    pub(crate) playback_performance: Option<PlaybackPerformance>,
}

impl ViewerSession {
    #[cfg(test)]
    pub(crate) fn open(
        path: &Path,
        topic: String,
        requirements: &PlaybackRequirements,
        bindings: &WorkspaceBindings,
    ) -> Result<Self> {
        let playback = McapPlayback::open_file(path)?;
        Self::from_playback(path, playback, topic, requirements, bindings)
    }

    pub(crate) fn from_playback(
        path: &Path,
        mut playback: McapPlayback,
        topic: String,
        requirements: &PlaybackRequirements,
        bindings: &WorkspaceBindings,
    ) -> Result<Self> {
        let plan = SessionPlan::build(playback.catalog(), &topic, requirements, bindings)?;
        plan.validate_recording_capabilities(playback.catalog().capabilities)?;
        playback.select_streams(plan.selected_stream_ids())?;
        let persistent = plan
            .restore_inputs()
            .into_iter()
            .filter(|input| input.semantics == RestoreSemantics::Persistent)
            .map(|input| input.stream_id)
            .collect::<Vec<_>>();
        playback.bootstrap_persistent(&persistent)?;
        playback.clock_mut().toggle();
        let topic_tree = viewer_core::TopicTree::from_catalog(playback.catalog());
        Ok(Self {
            topic_tree,
            source: SessionSource::Mcap(Box::new(playback)),
            plan,
            topic,
            source_name: path.display().to_string(),
            odometry_signals: OdometrySignalLoader::default(),
            odometry_topic: bindings.odometry_topic.clone(),
            numeric_plots: Default::default(),
            inspector_loader: InspectorLoader::default(),
            window_queries: Default::default(),
            #[cfg(feature = "plugin-panels")]
            plugin_queries: Default::default(),
        })
    }

    #[cfg(feature = "ros2-live")]
    pub(crate) fn open_live(
        topic: String,
        reliable: bool,
        raw_camera_topics: &std::collections::BTreeSet<String>,
        requirements: &PlaybackRequirements,
        bindings: &WorkspaceBindings,
    ) -> Result<Self> {
        let catalog = live::live_catalog(&topic, requirements, bindings, raw_camera_topics)?;
        let plan = SessionPlan::build(&catalog, &topic, requirements, bindings)?;
        let specs = live::selected_stream_specs(&plan);
        let handle = live::RosLiveHandle::start(specs, reliable).map_err(anyhow::Error::msg)?;
        Ok(Self {
            topic_tree: viewer_core::TopicTree::from_catalog(&catalog),
            source: SessionSource::Ros { handle, catalog },
            plan,
            topic,
            source_name: format!(
                "ROS 2 live ({})",
                if reliable { "reliable" } else { "best effort" }
            ),
            odometry_signals: OdometrySignalLoader::default(),
            odometry_topic: bindings.odometry_topic.clone(),
            numeric_plots: Default::default(),
            inspector_loader: InspectorLoader::default(),
            window_queries: Default::default(),
            #[cfg(feature = "plugin-panels")]
            plugin_queries: Default::default(),
        })
    }

    pub(crate) fn take_query_workers_from(&mut self, previous: &mut Self) {
        #[cfg(feature = "plugin-panels")]
        self.plugin_queries
            .take_worker_from(&mut previous.plugin_queries);
        self.window_queries
            .take_worker_from(&mut previous.window_queries);
        self.numeric_plots
            .take_worker_from(&mut previous.numeric_plots);
        self.inspector_loader
            .take_worker_from(&mut previous.inspector_loader);
    }

    pub(crate) fn topic_tree(&self) -> &viewer_core::TopicTree {
        &self.topic_tree
    }
    pub(crate) fn numeric_series(&self) -> &[crate::numeric_plot_loader::NumericSeriesState] {
        &self.numeric_plots.series
    }
    pub(crate) fn add_numeric_series(&mut self, stream: viewer_core::StreamId, field_path: String) {
        let Some(playback) = self.playback_view() else {
            return;
        };
        let Some(entry) = self
            .topic_tree
            .entries
            .iter()
            .find(|e| e.stream_id == stream)
        else {
            return;
        };
        if !entry.numeric_fields.iter().any(|f| f.path == field_path) {
            return;
        }
        let Some(range) = playback.end.0.checked_add(1).and_then(|end| {
            viewer_core::DataWindowTimeRange::new(playback.start, ArrivalTime(end)).ok()
        }) else {
            return;
        };
        self.numeric_plots.add(viewer_mcap::NumericSeriesRequest {
            stream_id: stream,
            field_path,
            range,
            max_points: 4000,
        });
    }
    pub(crate) fn remove_numeric_series(&mut self, stream: viewer_core::StreamId, path: &str) {
        self.numeric_plots.remove(stream, path);
    }

    fn recording_source(&self) -> Option<&viewer_mcap::McapSource> {
        match &self.source {
            SessionSource::Mcap(playback) => Some(playback.recording_source()),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => None,
        }
    }
    pub(crate) fn source_fingerprint(&self) -> anyhow::Result<viewer_core::SourceFingerprint> {
        let source = self
            .recording_source()
            .context("live sources have no recording fingerprint")?;
        Ok(viewer_mcap::mcap_summary_fingerprint(
            source.summary_identity()?,
        )?)
    }
    pub(crate) fn plan(&self) -> &SessionPlan {
        &self.plan
    }

    pub(crate) fn odometry_signal_request(
        &self,
        max_points: usize,
    ) -> Option<OdometrySignalRequest> {
        Some(OdometrySignalRequest {
            source: self.recording_source()?.fork_reader(),
            origin: self.playback_view()?.start,
            max_points,
            odometry_topic: self.odometry_topic.clone(),
        })
    }

    pub(crate) fn request_odometry_signals(&mut self, max_points: usize) -> Result<()> {
        let Some(request) = self.odometry_signal_request(max_points) else {
            self.odometry_signals.clear();
            return Ok(());
        };
        self.odometry_signals.start_overview(request)
    }

    pub(crate) fn poll_queries(&mut self) {
        #[cfg(feature = "plugin-panels")]
        let plugin_cursor = self.playback_view();
        let source = match &self.source {
            SessionSource::Mcap(playback) => Some(playback.recording_source()),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => None,
        };
        self.numeric_plots.poll(source);
        self.odometry_signals.poll();
        let target = match &self.source {
            SessionSource::Mcap(playback) => Some(playback.clock().view().cursor),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => None,
        };
        self.inspector_loader.poll(source, target);
        self.window_queries.poll(source, target);
        #[cfg(feature = "plugin-panels")]
        self.plugin_queries.poll(source, plugin_cursor);
    }

    pub(crate) fn configure_window_queries(
        &mut self,
        requirements: &crate::panels::PanelDataRequirements,
    ) {
        #[cfg(feature = "plugin-panels")]
        self.plugin_queries.configure(requirements.plugins.clone());
        self.window_queries
            .configure_health(&requirements.topic_health);
        self.window_queries
            .configure_diagnostics(&requirements.diagnostics);
    }
    #[cfg(feature = "plugin-panels")]
    pub(crate) fn plugin_data(
        &self,
    ) -> (
        &std::collections::BTreeMap<String, viewer_extension::panel::PanelUpdate>,
        u64,
    ) {
        (&self.plugin_queries.data, self.plugin_queries.generation)
    }
    pub(crate) fn producer_diagnostics(
        &self,
    ) -> &[crate::window_query_loader::DiagnosticsQueryState] {
        self.window_queries.diagnostics()
    }
    pub(crate) fn topic_health(&self) -> &[crate::window_query_loader::TopicHealthQueryState] {
        self.window_queries.health()
    }

    pub(crate) fn signal_view(
        &self,
        current_odometry: Option<&viewer_core::TelemetryFrame>,
    ) -> SignalView<'_> {
        self.odometry_signals
            .query_view()
            .with_current_odometry(current_odometry)
    }

    pub(crate) fn request_inspections(
        &mut self,
        requirements: &[InspectorRequirement],
    ) -> Result<()> {
        self.inspector_loader.configure(requirements);
        Ok(())
    }
    pub(crate) fn select_inspector_topic(&mut self, topic: Option<String>) {
        if topic.as_ref().is_none_or(|topic| {
            self.topic_tree
                .entries
                .iter()
                .any(|entry| &entry.topic == topic)
        }) {
            self.inspector_loader.select_topic(topic);
        }
    }
    pub(crate) fn selected_inspector_topic(&self) -> Option<&str> {
        self.inspector_loader.selected_topic()
    }

    pub(crate) fn inspections(&self) -> &[TopicInspection] {
        self.inspector_loader.inspections()
    }

    pub(crate) fn tick(
        &mut self,
        elapsed: Duration,
        process: impl FnOnce(Duration, Vec<RawMessage>),
    ) -> Result<()> {
        match &mut self.source {
            SessionSource::Mcap(playback) => playback.tick(elapsed, process)?,
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { handle, .. } => {
                if let Some(error) = handle.error() {
                    return Err(anyhow::Error::msg(error));
                }
                process(elapsed, handle.take_ready());
            }
        }
        Ok(())
    }

    pub(crate) fn default_focused_camera(&self) -> Option<CameraId> {
        self.plan.primary_camera()
    }

    pub(crate) fn camera_id_for_topic(&self, topic: &str) -> Option<CameraId> {
        self.plan
            .camera_routes()
            .iter()
            .find(|route| route.stream.topic == topic)
            .map(|route| route.camera_id)
    }

    pub(crate) fn playback_view(&self) -> Option<PlaybackView> {
        match &self.source {
            SessionSource::Mcap(playback) => Some(playback.clock().view()),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => None,
        }
    }

    pub(crate) fn camera_catalog(&self) -> &viewer_core::SourceCatalog {
        match &self.source {
            SessionSource::Mcap(playback) => playback.catalog(),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { catalog, .. } => catalog,
        }
    }

    pub(crate) fn select_camera_topics(
        &mut self,
        priority: &str,
        requirements: &PlaybackRequirements,
        bindings: &WorkspaceBindings,
        restore: impl FnOnce(
            &SessionPlan,
            ArrivalTime,
            Vec<RawMessage>,
        ) -> Result<(), FeatureRestoreError>,
    ) -> Result<()> {
        match &mut self.source {
            SessionSource::Mcap(playback) => {
                let plan =
                    SessionPlan::build(playback.catalog(), priority, requirements, bindings)?;
                plan.validate_recording_capabilities(playback.catalog().capabilities)?;
                let restore_plan = viewer_core::RestorePlanner::new(playback.catalog())
                    .plan(playback.clock().cursor(), plan.restore_inputs())?;
                playback.reselect_with(
                    plan.selected_stream_ids(),
                    &restore_plan,
                    |target, messages| restore(&plan, target, messages),
                )?;
                self.plan = plan;
                self.topic = priority.to_owned();
                Ok(())
            }
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => {
                anyhow::bail!("Changing live subscriptions requires reconnecting")
            }
        }
    }

    pub(crate) fn apply_playback_command(
        &mut self,
        command: PlaybackCommand,
        restore: impl FnOnce(ArrivalTime, Vec<RawMessage>) -> Result<(), FeatureRestoreError>,
    ) -> Result<PlaybackEffect> {
        match &mut self.source {
            SessionSource::Mcap(playback) => match command {
                PlaybackCommand::Toggle => {
                    playback.clock_mut().toggle();
                    Ok(PlaybackEffect::None)
                }
                PlaybackCommand::SetSpeed(speed) => {
                    playback.clock_mut().set_speed(speed);
                    Ok(PlaybackEffect::None)
                }
                PlaybackCommand::Seek(cursor) => {
                    let target = cursor.clamp(playback.clock().start(), playback.clock().end());
                    let restore_plan = viewer_core::RestorePlanner::new(playback.catalog())
                        .plan(target, self.plan.restore_inputs())?;
                    playback.seek_with(&restore_plan, restore)?;
                    self.window_queries.invalidate();
                    #[cfg(feature = "plugin-panels")]
                    self.plugin_queries.invalidate();
                    Ok(PlaybackEffect::Seeked)
                }
            },
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => Ok(PlaybackEffect::None),
        }
    }

    pub(crate) fn source_read_timing(&self) -> StageTiming {
        match &self.source {
            SessionSource::Mcap(playback) => playback.source_read_timing(),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => StageTiming::default(),
        }
    }

    pub(crate) fn diagnostics(
        &self,
        counters: ProcessingCounters,
        playback_performance: Option<PlaybackPerformance>,
    ) -> SessionDiagnostics {
        #[cfg(feature = "ros2-live")]
        let source_name = match &self.source {
            SessionSource::Ros { handle, .. } => {
                let live = handle.diagnostics();
                let age = live.latest_arrival.and_then(|arrival| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()?;
                    let now = i64::try_from(now.as_nanos()).ok()?;
                    Some((now - arrival.0).max(0) as f64 / 1e9)
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
                    "{} · {} · received {} · coalesced {} · overflow {} · resident {} · adapter errors {} · CDR output {} KiB",
                    self.source_name,
                    freshness,
                    live.received_messages,
                    live.coalesced_messages,
                    live.ordered_queue_overflows,
                    live.resident_messages,
                    live.adapter_errors,
                    live.payload_copy_bytes / 1024
                )
            }
            SessionSource::Mcap(_) => self.source_name.clone(),
        };
        #[cfg(not(feature = "ros2-live"))]
        let source_name = self.source_name.clone();
        SessionDiagnostics {
            source_name,
            primary_topic: self.topic.clone(),
            camera_topics: self.plan.camera_topics(),
            counters,
            playback_performance,
        }
    }
}

#[cfg(test)]
mod numeric_demand_tests {
    use super::*;
    #[test]
    fn adding_and_removing_field_plots_preserves_fixed_selection_and_cursor() {
        let mut output = std::io::Cursor::new(vec![]);
        {
            let mut writer = mcap::Writer::new(&mut output).unwrap();
            let schema = writer
                .add_schema("demo/msg/Scalar", "ros2msg", b"float64 value\n")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/scalar", "cdr", &std::collections::BTreeMap::new())
                .unwrap();
            for sequence in 0..2 {
                writer
                    .write_to_known_channel(
                        &mcap::records::MessageHeader {
                            channel_id: channel,
                            sequence,
                            log_time: 100 + sequence as u64,
                            publish_time: 99 + sequence as u64,
                        },
                        &[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 240, 63],
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        let path = std::env::temp_dir().join(format!(
            "mcap-player-numeric-demand-{}.mcap",
            std::process::id()
        ));
        std::fs::write(&path, output.into_inner()).unwrap();
        let bindings = WorkspaceBindings {
            path_topic: "/path".into(),
            odometry_topic: "/custom/odom".into(),
            point_cloud_topic: "/scan".into(),
            dynamic_tf_topic: "/tf".into(),
            static_tf_topic: "/tf_static".into(),
        };
        let explorer = crate::workspace::NativeWorkspace::load_bundled_or_fallback(
            crate::workspace::WorkspaceLayout::Explore,
        );
        assert!(explorer.startup_warning().is_none());
        let requirements = explorer.data_requirements();
        assert_eq!(requirements.playback, PlaybackRequirements::empty());
        assert!(requirements.signals.is_empty());
        let mut session =
            ViewerSession::open(&path, "/camera".into(), &requirements.playback, &bindings)
                .unwrap();
        let selected = session.plan().selected_stream_ids();
        let cursor = session.playback_view().unwrap().cursor;
        let stream = session.topic_tree().entries[0].stream_id;
        session.select_inspector_topic(Some("/scalar".into()));
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            session.poll_queries();
            if session.inspections().first().is_some_and(|i| !i.loading) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(session.inspections()[0].metadata.unwrap().log_time, cursor);
        assert!(session.inspections()[0].message.is_some());
        assert_eq!(session.plan().selected_stream_ids(), selected);
        assert_eq!(session.playback_view().unwrap().cursor, cursor);
        session.add_numeric_series(stream, "value".into());
        assert_eq!(session.numeric_series().len(), 1);
        assert_eq!(session.plan().selected_stream_ids(), selected);
        assert!(selected.is_empty());
        assert_eq!(session.playback_view().unwrap().cursor, cursor);
        // Builtin query configuration is also available independently of continuous Odometry.
        assert_eq!(
            session.odometry_signal_request(100).unwrap().odometry_topic,
            "/custom/odom"
        );
        session.remove_numeric_series(stream, "value");
        assert!(session.numeric_series().is_empty());
        assert_eq!(session.plan().selected_stream_ids(), selected);
        assert_eq!(session.playback_view().unwrap().cursor, cursor);
        drop(session);
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(all(test, feature = "plugin-panels"))]
mod plugin_panel_tests {
    use super::*;
    #[test]
    #[ignore = "build RobotStatus guest and generate target/plugin-panel fixtures first"]
    fn actual_robot_status_session_preserves_playback_routes_and_seek_boundary() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../");
        let mut workspace = crate::workspace::NativeWorkspace::load_bundled_or_fallback(
            crate::workspace::WorkspaceLayout::Explore,
        );
        workspace
            .add_plugin_panel(&root.join("examples/robot-status-panel/panel.json"))
            .unwrap();
        let requirements = workspace.data_requirements();
        assert_eq!(requirements.playback, PlaybackRequirements::empty());
        assert_eq!(requirements.plugins.len(), 1);
        let path = root.join("target/plugin-panel/recording.mcap");
        let mut session = ViewerSession::open(
            &path,
            "/camera".into(),
            &requirements.playback,
            workspace.bindings(),
        )
        .unwrap();
        session.configure_window_queries(&requirements);
        assert!(session.plan().selected_stream_ids().is_empty());
        let target = session.playback_view().unwrap().end;
        session
            .apply_playback_command(PlaybackCommand::Seek(target), |_, messages| {
                assert!(messages.is_empty());
                Ok(())
            })
            .unwrap();
        let generation = session.plugin_data().1;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while session.plugin_data().0.is_empty() {
            session.poll_queries();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(
            session.plugin_data().0.values().next().unwrap().status,
            viewer_extension::panel::InputStatus::Ready
        );
        assert_eq!(session.playback_view().unwrap().cursor, target);
        assert!(session.plan().selected_stream_ids().is_empty());
        let start = session.playback_view().unwrap().start;
        let previous_data = session.plugin_data().0.clone();
        assert!(
            session
                .apply_playback_command(PlaybackCommand::Seek(start), |target, _| Err(
                    FeatureRestoreError {
                        stream_id: viewer_core::StreamId(u32::MAX),
                        arrival_time: target,
                        kind: viewer_core::FeatureRestoreErrorKind::Unrouted
                    }
                ))
                .is_err()
        );
        assert_eq!(session.plugin_data().1, generation);
        assert_eq!(session.plugin_data().0, &previous_data);
        assert_eq!(session.playback_view().unwrap().cursor, target);
        session
            .apply_playback_command(PlaybackCommand::Seek(start), |_, _| Ok(()))
            .unwrap();
        assert_ne!(session.plugin_data().1, generation);
        assert!(session.plugin_data().0.is_empty());
        assert_eq!(session.playback_view().unwrap().cursor, start);
    }
}
