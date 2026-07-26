#[cfg(feature = "ros2-live")]
use crate::live;
#[cfg(feature = "ros2-live")]
use anyhow::bail;
use anyhow::{Context, Result};
use memmap2::Mmap;
use std::{fs::File, path::Path, time::Duration};
use viewer_core::{
    CameraId, CameraStatus, DomainState, McapPlayback, PipelineCounters, PlaybackClock,
    PlaybackPerformance, PresentationSnapshot,
};
#[cfg(feature = "ros2-live")]
use viewer_core::{PipelineSet, StreamBinding};
use viewer_ui::{TelemetryPresentation, ViewerPresentation};

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
        pipelines: PipelineSet,
        state: DomainState,
    },
}

impl PlaybackSession {
    pub(crate) fn open(path: &Path, topic: String) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the mapping is read-only and owns an independent reference to the file pages.
        let mapping =
            unsafe { Mmap::map(&file) }.with_context(|| format!("map {}", path.display()))?;
        let mut playback = McapPlayback::new(mapping, &topic)?;
        playback.clock_mut().play();
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
        let pipelines = PipelineSet::new(
            std::slice::from_ref(&descriptor),
            &[(descriptor.id, StreamBinding::Camera(CameraId(0)))],
        );
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

    pub(crate) fn seek(&mut self) -> Result<()> {
        match &mut self.source {
            SessionSource::Mcap(playback) => {
                let cursor = playback.clock().cursor();
                playback.seek(cursor)?;
            }
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => {}
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

    pub(crate) fn state_mut(&mut self) -> &mut DomainState {
        match &mut self.source {
            SessionSource::Mcap(playback) => playback.state_mut(),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { state, .. } => state,
        }
    }

    pub(crate) fn clock_mut(&mut self) -> Option<&mut PlaybackClock> {
        match &mut self.source {
            SessionSource::Mcap(playback) => Some(playback.clock_mut()),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => None,
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

    pub(crate) fn camera_topics(&self) -> &[(CameraId, String)] {
        &self.camera_topics
    }

    pub(crate) fn set_focused_camera(&mut self, focused_camera: Option<CameraId>) {
        match &mut self.source {
            SessionSource::Mcap(playback) => playback.set_focused_camera(focused_camera),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => {}
        }
    }

    fn playback_performance(&self) -> Option<&PlaybackPerformance> {
        match &self.source {
            SessionSource::Mcap(playback) => Some(playback.performance()),
            #[cfg(feature = "ros2-live")]
            SessionSource::Ros { .. } => None,
        }
    }

    pub(crate) fn presentation(
        &self,
        error: Option<String>,
        focused_camera: Option<CameraId>,
        presentation_performance: PresentationSnapshot,
    ) -> ViewerPresentation {
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
        ViewerPresentation {
            source: source_name,
            topic: self.topic.clone(),
            camera_status: focused_camera
                .map_or(CameraStatus::WaitingForCameraFrame, |camera_id| {
                    self.state().camera.status_for(camera_id)
                }),
            counters: self.counters(),
            playback_performance: self.playback_performance().cloned(),
            presentation_performance,
            focused_camera,
            telemetry: self
                .state()
                .telemetry
                .latest()
                .map(|frame| TelemetryPresentation {
                    frame_id: frame.frame_id.clone(),
                    child_frame_id: frame.child_frame_id.clone(),
                    position_x: frame.position_x,
                    position_y: frame.position_y,
                    yaw_radians: frame.yaw_radians,
                    forward_velocity: frame.forward_velocity,
                    speed: frame.speed,
                    yaw_rate: frame.yaw_rate,
                }),
            error,
        }
    }
}
