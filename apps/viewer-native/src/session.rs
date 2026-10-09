#[cfg(feature = "ros2-live")]
use crate::live;
use crate::plot_loader::PlotLoader;
use crate::signal_query::SignalQueryView;
use crate::{
    inspection::{InspectorRequirement, TopicInspection},
    inspector_loader::InspectorLoader,
};
use anyhow::{Context, Result};
use std::{path::Path, time::Duration};
use viewer_core::{
    ArrivalTime, CameraId, FeatureRestoreError, McapPlayback, PlaybackCommand, PlaybackEffect,
    PlaybackPerformance, PlaybackRequirements, PlaybackView, ProcessingCounters, RawMessage,
    RestoreSemantics, SessionPlan, StageTiming, WorkspaceBindings,
};
#[cfg(feature = "ros2-live")]
use viewer_core::{SourceCatalog, StreamDescriptor};

/// The currently open Viewer data session, backed by either Recording or Live input.
///
/// It owns source access and playback/query capabilities, but feature/controller state belongs
/// to the workspace and is updated through exact serialized messages.
pub(crate) struct ViewerSession {
    source: SessionSource,
    plan: SessionPlan,
    topic: String,
    source_name: String,
    plot_loader: PlotLoader,
    odometry_topic: String,
    topic_tree: viewer_core::TopicTree,
    numeric_plots: crate::numeric_plot_loader::NumericPlotLoader,
    inspector_loader: InspectorLoader,
}

pub(crate) struct PlotSignalRequest {
    pub(crate) source: viewer_core::McapSource,
    pub(crate) origin: ArrivalTime,
    pub(crate) max_points: usize,
    pub(crate) odometry_topic: String,
}

enum SessionSource {
    Mcap(Box<McapPlayback>),
    #[cfg(feature = "ros2-live")]
    Ros {
        handle: live::RosLiveHandle,
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
    pub(crate) fn open(
        path: &Path,
        topic: String,
        requirements: &PlaybackRequirements,
        bindings: &WorkspaceBindings,
    ) -> Result<Self> {
        let mut playback =
            McapPlayback::open_file(path).with_context(|| format!("open {}", path.display()))?;
        let plan = SessionPlan::build(playback.catalog(), &topic, requirements, bindings)?;
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
            plot_loader: PlotLoader::default(),
            odometry_topic: bindings.odometry_topic.clone(),
            numeric_plots: Default::default(),
            inspector_loader: InspectorLoader::default(),
        })
    }

    #[cfg(feature = "ros2-live")]
    pub(crate) fn open_live(
        topic: String,
        reliable: bool,
        requirements: &PlaybackRequirements,
        bindings: &WorkspaceBindings,
    ) -> Self {
        let descriptor = StreamDescriptor {
            schema_definition: None,
            id: viewer_core::StreamId(1),
            topic: topic.clone(),
            schema: "sensor_msgs/msg/CompressedImage".into(),
            message_encoding: "cdr".into(),
            timing: viewer_core::StreamTimingSummary::default(),
        };
        let catalog = SourceCatalog {
            time_range: None,
            streams: vec![descriptor],
            capabilities: viewer_core::SourceCapabilities::LIVE,
        };
        let plan = SessionPlan::build(&catalog, &topic, requirements, bindings)
            .expect("the live Camera descriptor matches its configured primary topic");
        Self {
            topic_tree: viewer_core::TopicTree::from_catalog(&catalog),
            source: SessionSource::Ros {
                handle: live::RosLiveHandle::start(topic.clone(), reliable),
            },
            plan,
            topic,
            source_name: format!(
                "ROS 2 live ({})",
                if reliable { "reliable" } else { "best effort" }
            ),
            plot_loader: PlotLoader::default(),
            odometry_topic: bindings.odometry_topic.clone(),
            numeric_plots: Default::default(),
            inspector_loader: InspectorLoader::default(),
        }
    }

    pub(crate) fn take_query_workers_from(&mut self, previous: &mut Self) {
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
        self.numeric_plots.add(viewer_core::NumericSeriesRequest {
            stream_id: stream,
            field_path,
            range,
            max_points: 4000,
        });
    }
    pub(crate) fn remove_numeric_series(&mut self, stream: viewer_core::StreamId, path: &str) {
        self.numeric_plots.remove(stream, path);
    }

    fn recording_source(&self) -> Option<&viewer_core::McapSource> {
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
        Ok(viewer_core::mcap_summary_fingerprint(
            source.summary_identity()?,
        )?)
    }
    pub(crate) fn plan(&self) -> &SessionPlan {
        &self.plan
    }

    pub(crate) fn plot_signal_request(&self, max_points: usize) -> Option<PlotSignalRequest> {
        Some(PlotSignalRequest {
            source: self.recording_source()?.fork_reader(),
            origin: self.playback_view()?.start,
            max_points,
            odometry_topic: self.odometry_topic.clone(),
        })
    }

    pub(crate) fn request_plot_signals(&mut self, max_points: usize) -> Result<()> {
        let Some(request) = self.plot_signal_request(max_points) else {
            self.plot_loader.clear();
            return Ok(());
        };
        self.plot_loader.start_overview(request)
    }

    pub(crate) fn poll_queries(&mut self) {
        let source = match &self.source {
            SessionSource::Mcap(playback) => Some(playback.recording_source()),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => None,
        };
        self.numeric_plots.poll(source);
        self.plot_loader.poll();
        let target = match &self.source {
            SessionSource::Mcap(playback) => Some(playback.clock().view().cursor),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => None,
        };
        self.inspector_loader.poll(source, target);
    }

    pub(crate) fn signal_query_view(
        &self,
        current_odometry: Option<&viewer_core::TelemetryFrame>,
    ) -> SignalQueryView<'_> {
        self.plot_loader
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
            SessionSource::Ros { handle } => {
                if let Some(error) = handle.error() {
                    return Err(anyhow::Error::new(error)).context("ROS executor");
                }
                process(elapsed, handle.take().into_iter().collect());
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
        _latest_camera_arrival: Option<ArrivalTime>,
    ) -> SessionDiagnostics {
        #[cfg(feature = "ros2-live")]
        let source_name = match &self.source {
            SessionSource::Ros { handle } => {
                let age = _latest_camera_arrival.and_then(|arrival| {
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
            session.plot_signal_request(100).unwrap().odometry_topic,
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
