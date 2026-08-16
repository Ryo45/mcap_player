use crate::inspection::{InspectedMessage, InspectorRequirement, TopicInspection};
#[cfg(feature = "ros2-live")]
use crate::live;
use crate::plot_loader::PlotLoader;
use crate::signal_query::SignalQueryView;
#[cfg(feature = "ros2-live")]
use anyhow::bail;
use anyhow::{Context, Result};
use memmap2::Mmap;
use std::{
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};
use viewer_core::{
    ArrivalTime, CameraId, McapPlayback, PlaybackCommand, PlaybackEffect, PlaybackPerformance,
    PlaybackRequirements, PlaybackView, ProcessingCounters, RawMessage, RestoreSemantics,
    SessionPlan, StageTiming, WorkspaceBindings,
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
    recording_path: Option<PathBuf>,
    plot_loader: PlotLoader,
    inspections: Vec<TopicInspection>,
}

pub(crate) struct PlotSignalRequest {
    pub(crate) path: PathBuf,
    pub(crate) origin: ArrivalTime,
    pub(crate) max_points: usize,
    pub(crate) odometry_topic: String,
}

enum SessionSource {
    Mcap(Box<McapPlayback<Mmap>>),
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
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the mapping is read-only and owns an independent reference to the file pages.
        let mapping =
            unsafe { Mmap::map(&file) }.with_context(|| format!("map {}", path.display()))?;
        let mut playback = McapPlayback::new(mapping)?;
        let plan = SessionPlan::build(playback.catalog(), &topic, requirements, bindings)?;
        playback.select_streams(plan.selected_stream_ids());
        let persistent = plan
            .restore_inputs()
            .into_iter()
            .filter(|input| input.semantics == RestoreSemantics::Persistent)
            .map(|input| input.stream_id)
            .collect::<Vec<_>>();
        playback.bootstrap_persistent(&persistent)?;
        playback.clock_mut().toggle();
        Ok(Self {
            source: SessionSource::Mcap(Box::new(playback)),
            plan,
            topic,
            source_name: path.display().to_string(),
            recording_path: Some(path.to_owned()),
            plot_loader: PlotLoader::default(),
            inspections: Vec::new(),
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
            id: viewer_core::StreamId(1),
            topic: topic.clone(),
            schema: "sensor_msgs/msg/CompressedImage".into(),
            message_encoding: "cdr".into(),
            timing: viewer_core::StreamTimingSummary::default(),
        };
        let catalog = SourceCatalog {
            time_range: None,
            streams: vec![descriptor],
        };
        let plan = SessionPlan::build(&catalog, &topic, requirements, bindings)
            .expect("the live Camera descriptor matches its configured primary topic");
        Self {
            source: SessionSource::Ros {
                handle: live::RosLiveHandle::start(topic.clone(), reliable),
            },
            plan,
            topic,
            source_name: format!(
                "ROS 2 live ({})",
                if reliable { "reliable" } else { "best effort" }
            ),
            recording_path: None,
            plot_loader: PlotLoader::default(),
            inspections: Vec::new(),
        }
    }

    pub(crate) fn plan(&self) -> &SessionPlan {
        &self.plan
    }

    pub(crate) fn plot_signal_request(&self, max_points: usize) -> Option<PlotSignalRequest> {
        Some(PlotSignalRequest {
            path: self.recording_path.clone()?,
            origin: self.playback_view()?.start,
            max_points,
            odometry_topic: self.plan.odometry_stream()?.topic.clone(),
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
        self.plot_loader.poll();
    }

    pub(crate) fn signal_query_view(&self) -> SignalQueryView<'_> {
        self.plot_loader.query_view()
    }

    pub(crate) fn inspect_topic(
        &self,
        topic: &str,
        max_messages: usize,
    ) -> Result<Vec<InspectedMessage>> {
        if max_messages == 0 {
            return Ok(Vec::new());
        }
        #[cfg(not(feature = "ros2-live"))]
        let SessionSource::Mcap(playback) = &self.source;
        #[cfg(feature = "ros2-live")]
        let playback = match &self.source {
            SessionSource::Mcap(playback) => playback,
            SessionSource::Ros { .. } => {
                anyhow::bail!("message inspection is unavailable for this source")
            }
        };
        let stream = playback
            .catalog()
            .by_topic(topic)
            .with_context(|| format!("recording has no topic {topic}"))?;
        let recording = playback
            .catalog()
            .time_range
            .context("recording has no indexed time range")?;
        let range =
            viewer_core::DataWindowTimeRange::new(recording.start, recording.end_exclusive)?;
        let result = playback.query_range(&viewer_core::RangeQuery {
            streams: vec![stream.id],
            range,
            limits: viewer_core::QueryLimits::new(max_messages, 16 * 1024 * 1024)?,
        })?;
        Ok(result
            .messages
            .into_iter()
            .map(|message| InspectedMessage {
                arrival_time: message.arrival_time,
                payload_bytes: message.payload.len(),
            })
            .collect())
    }

    pub(crate) fn load_inspections(&mut self, requirements: &[InspectorRequirement]) {
        self.inspections = requirements
            .iter()
            .map(|requirement| {
                match self.inspect_topic(&requirement.topic, requirement.max_messages) {
                    Ok(messages) => TopicInspection {
                        topic: requirement.topic.clone(),
                        messages,
                        error: None,
                    },
                    Err(error) => TopicInspection {
                        topic: requirement.topic.clone(),
                        messages: Vec::new(),
                        error: Some(error.to_string()),
                    },
                }
            })
            .collect();
    }

    pub(crate) fn inspections(&self) -> &[TopicInspection] {
        &self.inspections
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
                    bail!("ROS executor: {error}");
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
        restore: impl FnOnce(ArrivalTime, Vec<RawMessage>),
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
