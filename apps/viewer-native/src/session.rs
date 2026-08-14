#[cfg(feature = "ros2-live")]
use crate::live;
#[cfg(feature = "ros2-live")]
use anyhow::bail;
use anyhow::{Context, Result};
use memmap2::Mmap;
use std::{fs::File, path::Path, time::Duration};
use viewer_core::{
    CameraId, DomainState, McapPlayback, PipelineCounters, PlaybackCommand, PlaybackEffect,
    PlaybackPerformance, PlaybackView,
};
#[cfg(feature = "ros2-live")]
use viewer_core::{DomainPipelineSet, DomainRoute, DomainTarget};

pub(crate) struct PlaybackSession {
    source: SessionSource,
    topic: String,
    camera_topics: Vec<(CameraId, String)>,
    source_name: String,
}

enum SessionSource {
    Mcap(Box<McapPlayback<Mmap>>),
    #[cfg(feature = "ros2-live")]
    Ros {
        handle: live::RosLiveHandle,
        pipelines: DomainPipelineSet,
        state: DomainState,
    },
}

pub(crate) struct SessionDiagnostics {
    pub(crate) source_name: String,
    pub(crate) primary_topic: String,
    pub(crate) camera_topics: Vec<(CameraId, String)>,
    pub(crate) counters: PipelineCounters,
    pub(crate) playback_performance: Option<PlaybackPerformance>,
}

impl PlaybackSession {
    pub(crate) fn open(path: &Path, topic: String) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the mapping is read-only and owns an independent reference to the file pages.
        let mapping =
            unsafe { Mmap::map(&file) }.with_context(|| format!("map {}", path.display()))?;
        let mut playback = McapPlayback::new(mapping, &topic)?;
        playback.apply_command(PlaybackCommand::Toggle)?;
        let camera_topics = playback.camera_topics().to_vec();
        Ok(Self {
            source: SessionSource::Mcap(Box::new(playback)),
            topic,
            camera_topics,
            source_name: path.display().to_string(),
        })
    }

    #[cfg(feature = "ros2-live")]
    pub(crate) fn open_live(topic: String, reliable: bool) -> Self {
        let descriptor = viewer_core::StreamDescriptor {
            id: viewer_core::StreamId(1),
            topic: topic.clone(),
            schema: "sensor_msgs/msg/CompressedImage".into(),
            message_encoding: "cdr".into(),
        };
        let pipelines = DomainPipelineSet::from_routes(&[DomainRoute {
            stream: descriptor,
            target: DomainTarget::Camera(CameraId(0)),
        }]);
        let camera_topics = vec![(CameraId(0), topic.clone())];
        Self {
            source: SessionSource::Ros {
                handle: live::RosLiveHandle::start(topic.clone(), reliable),
                pipelines,
                state: DomainState::default(),
            },
            topic,
            camera_topics,
            source_name: format!(
                "ROS 2 live ({})",
                if reliable { "reliable" } else { "best effort" }
            ),
        }
    }

    pub(crate) fn tick(&mut self, elapsed: Duration) -> Result<()> {
        match &mut self.source {
            SessionSource::Mcap(playback) => playback.tick(elapsed)?,
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros {
                handle,
                pipelines,
                state,
            } => {
                if let Some(error) = handle.error() {
                    bail!("ROS executor: {error}");
                }
                if let Some(message) = handle.take() {
                    let mut updates = Vec::new();
                    pipelines.decode(message, &mut updates);
                    state.apply_all(updates);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn state(&self) -> &DomainState {
        match &self.source {
            SessionSource::Mcap(playback) => playback.state(),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { state, .. } => state,
        }
    }

    fn counters(&self) -> PipelineCounters {
        match &self.source {
            SessionSource::Mcap(playback) => playback.counters(),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros {
                handle, pipelines, ..
            } => {
                let mut counters = pipelines.counters();
                counters.dropped = handle.coalesced();
                counters
            }
        }
    }

    pub(crate) fn default_focused_camera(&self) -> Option<CameraId> {
        self.camera_topics.first().map(|(camera_id, _)| *camera_id)
    }

    pub(crate) fn set_focused_camera(&mut self, focused_camera: Option<CameraId>) {
        let focused_camera = focused_camera
            .filter(|camera_id| self.camera_topics.iter().any(|(id, _)| id == camera_id))
            .or_else(|| self.camera_topics.first().map(|(camera_id, _)| *camera_id));
        match &mut self.source {
            SessionSource::Mcap(playback) => playback.set_focused_camera(focused_camera),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => {}
        }
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
    ) -> Result<PlaybackEffect> {
        match &mut self.source {
            SessionSource::Mcap(playback) => Ok(playback.apply_command(command)?),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => Ok(PlaybackEffect::None),
        }
    }

    fn playback_performance(&self) -> Option<PlaybackPerformance> {
        match &self.source {
            SessionSource::Mcap(playback) => Some(playback.performance()),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => None,
        }
    }

    pub(crate) fn diagnostics(&self) -> SessionDiagnostics {
        #[cfg(feature = "ros2-live")]
        let source_name = match &self.source {
            SessionSource::Ros { handle, state, .. } => {
                let age = state.camera.latest_by_arrival().and_then(|frame| {
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
            camera_topics: self.camera_topics.clone(),
            counters: self.counters(),
            playback_performance: self.playback_performance(),
        }
    }
}
