use super::{RemoteApiClient, RemoteBatchSource, RemoteCatalog, RemoteSourceMetrics};
use std::{error::Error, fmt, time::Duration};
use viewer_core::{
    CameraId, DomainState, McapPlayback, PipelineCounters, PlaybackClock, PlaybackCommand,
    PlaybackCore, PlaybackEffect, PlaybackLoadState, PlaybackPerformance, PlaybackSpeed,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemotePlaybackError(String);

impl RemotePlaybackError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for RemotePlaybackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for RemotePlaybackError {}

pub(crate) struct RemotePlayback {
    clock: PlaybackClock,
    core: PlaybackCore,
    source: RemoteBatchSource,
    load_state: PlaybackLoadState,
}

impl RemotePlayback {
    pub(crate) fn new(
        client: RemoteApiClient,
        catalog: RemoteCatalog,
    ) -> Result<Self, RemotePlaybackError> {
        let core = PlaybackCore::new(&catalog.core, &catalog.primary_camera_topic)
            .map_err(|error| RemotePlaybackError::new(error.to_string()))?;
        let source = RemoteBatchSource::new(
            client,
            catalog.recording_id,
            catalog.revision,
            catalog.selected_streams,
            catalog.start,
            catalog.end_exclusive,
        )
        .map_err(|error| RemotePlaybackError::new(error.to_string()))?;
        Ok(Self {
            clock: PlaybackClock::new(catalog.start, catalog.end),
            core,
            source,
            load_state: PlaybackLoadState::Ready,
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn tick(&mut self, elapsed: Duration) -> Result<(), RemotePlaybackError> {
        if let Err(error) = self.source.poll_completed_fetch() {
            self.load_state = PlaybackLoadState::Failed {
                message: error.to_string(),
            };
            return Err(RemotePlaybackError::new(error.to_string()));
        }
        let requested = self.clock.cursor_after(elapsed);
        self.source.ensure_available_through(requested);
        if !self.commit_candidate(elapsed, requested) {
            return Ok(());
        }

        let prefetch = viewer_core::ArrivalTime(
            requested
                .0
                .saturating_add(duration_ns(self.source.prefetch_duration()))
                .min(self.clock.end().0),
        );
        self.source.ensure_available_through(prefetch);
        Ok(())
    }

    fn commit_candidate(&mut self, elapsed: Duration, requested: viewer_core::ArrivalTime) -> bool {
        if !self.source.is_complete_through(requested) {
            if !matches!(self.load_state, PlaybackLoadState::Buffering { .. }) {
                self.source.note_buffering();
            }
            self.load_state = PlaybackLoadState::Buffering {
                requested,
                committed: self.clock.cursor(),
            };
            return false;
        }

        let messages = self.source.drain_through(requested);
        self.core.process_forward(elapsed, messages);
        self.clock.commit_cursor(requested);
        self.load_state = PlaybackLoadState::Ready;
        true
    }

    pub(crate) fn apply_command(
        &mut self,
        command: PlaybackCommand,
    ) -> Result<PlaybackEffect, RemotePlaybackError> {
        match command {
            PlaybackCommand::Toggle => {
                self.clock.toggle();
                Ok(PlaybackEffect::None)
            }
            PlaybackCommand::SetSpeed(PlaybackSpeed::Normal) => {
                self.clock.set_speed(PlaybackSpeed::Normal);
                Ok(PlaybackEffect::None)
            }
            PlaybackCommand::SetSpeed(_) => Err(RemotePlaybackError::new(
                "Remote playback currently supports 1x speed only",
            )),
            PlaybackCommand::Seek(_) => Err(RemotePlaybackError::new(
                "Remote exact seek is not implemented yet",
            )),
        }
    }

    pub(crate) fn clock(&self) -> &PlaybackClock {
        &self.clock
    }

    pub(crate) fn state(&self) -> &DomainState {
        self.core.state()
    }

    pub(crate) fn camera_topics(&self) -> &[(CameraId, String)] {
        self.core.camera_topics()
    }

    pub(crate) fn set_focused_camera(&mut self, camera: Option<CameraId>) {
        self.core.set_focused_camera(camera);
    }

    pub(crate) fn counters(&self) -> PipelineCounters {
        self.core.counters()
    }

    pub(crate) fn performance(&self) -> &PlaybackPerformance {
        self.core.performance()
    }

    pub(crate) fn load_state(&self) -> PlaybackLoadState {
        self.load_state.clone()
    }

    pub(crate) fn metrics(&self) -> &RemoteSourceMetrics {
        self.source.metrics()
    }

    pub(crate) fn buffer_ahead(&self) -> Duration {
        let nanoseconds = self
            .source
            .complete_until()
            .0
            .saturating_sub(self.clock.cursor().0)
            .max(0) as u64;
        Duration::from_nanos(nanoseconds)
    }
}

pub(crate) enum WebPlayback {
    Local(McapPlayback<Vec<u8>>),
    Remote(RemotePlayback),
}

impl WebPlayback {
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn tick(&mut self, elapsed: Duration) -> Result<(), String> {
        match self {
            Self::Local(playback) => playback.tick(elapsed).map_err(|error| error.to_string()),
            Self::Remote(playback) => playback.tick(elapsed).map_err(|error| error.to_string()),
        }
    }

    pub(crate) fn apply_command(
        &mut self,
        command: PlaybackCommand,
    ) -> Result<PlaybackEffect, String> {
        match self {
            Self::Local(playback) => playback
                .apply_command(command)
                .map_err(|error| error.to_string()),
            Self::Remote(playback) => playback
                .apply_command(command)
                .map_err(|error| error.to_string()),
        }
    }

    pub(crate) fn clock(&self) -> &PlaybackClock {
        match self {
            Self::Local(playback) => playback.clock(),
            Self::Remote(playback) => playback.clock(),
        }
    }

    pub(crate) fn state(&self) -> &DomainState {
        match self {
            Self::Local(playback) => playback.state(),
            Self::Remote(playback) => playback.state(),
        }
    }

    pub(crate) fn camera_topics(&self) -> &[(CameraId, String)] {
        match self {
            Self::Local(playback) => playback.camera_topics(),
            Self::Remote(playback) => playback.camera_topics(),
        }
    }

    pub(crate) fn set_focused_camera(&mut self, camera: Option<CameraId>) {
        match self {
            Self::Local(playback) => playback.set_focused_camera(camera),
            Self::Remote(playback) => playback.set_focused_camera(camera),
        }
    }

    pub(crate) fn counters(&self) -> PipelineCounters {
        match self {
            Self::Local(playback) => playback.counters(),
            Self::Remote(playback) => playback.counters(),
        }
    }

    pub(crate) fn performance(&self) -> &PlaybackPerformance {
        match self {
            Self::Local(playback) => playback.performance(),
            Self::Remote(playback) => playback.performance(),
        }
    }

    pub(crate) fn load_state(&self) -> PlaybackLoadState {
        match self {
            Self::Local(_) => PlaybackLoadState::Ready,
            Self::Remote(playback) => playback.load_state(),
        }
    }

    pub(crate) fn is_remote(&self) -> bool {
        matches!(self, Self::Remote(_))
    }

    pub(crate) fn remote_diagnostics(&self) -> Option<(&RemoteSourceMetrics, Duration)> {
        match self {
            Self::Local(_) => None,
            Self::Remote(playback) => Some((playback.metrics(), playback.buffer_ahead())),
        }
    }
}

fn duration_ns(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use viewer_core::{
        CompressedImage, MeasurementTime, RawMessage, StreamId, encode_compressed_image_cdr,
    };
    use viewer_remote_protocol::{
        CatalogResponse, RemoteTimeRange, StreamDescriptor, StreamSemantic, TimestampNs,
    };

    fn remote_catalog() -> RemoteCatalog {
        let catalog = CatalogResponse::new(
            "demo".into(),
            "revision".into(),
            RemoteTimeRange {
                start_ns: TimestampNs::new(1_000_000_000),
                end_ns_exclusive: TimestampNs::new(3_000_000_000),
            },
            vec![StreamDescriptor {
                id: 1,
                topic: "/camera".into(),
                semantic: StreamSemantic::Camera,
                representation: "ros2-cdr".into(),
                schema_name: "sensor_msgs/msg/CompressedImage".into(),
                schema_encoding: "ros2msg".into(),
                message_encoding: "cdr".into(),
            }],
        );
        super::super::adapt_catalog(&catalog).unwrap()
    }

    fn camera_message(time: i64) -> RawMessage {
        RawMessage {
            stream_id: StreamId(1),
            arrival_time: viewer_core::ArrivalTime(time),
            payload: Bytes::from(
                encode_compressed_image_cdr(&CompressedImage {
                    measurement_time: MeasurementTime(time),
                    frame_id: "camera".into(),
                    format: "jpeg".into(),
                    jpeg: vec![1, 2, 3],
                })
                .unwrap(),
            ),
        }
    }

    #[test]
    fn buffering_keeps_committed_cursor_and_domain_until_window_is_complete() {
        let client = RemoteApiClient::new("http://localhost").unwrap();
        let mut playback = RemotePlayback::new(client, remote_catalog()).unwrap();
        playback.apply_command(PlaybackCommand::Toggle).unwrap();
        let elapsed = Duration::from_millis(100);
        let requested = playback.clock.cursor_after(elapsed);

        assert!(!playback.commit_candidate(elapsed, requested));
        assert_eq!(
            playback.clock.cursor(),
            viewer_core::ArrivalTime(1_000_000_000)
        );
        assert!(playback.state().camera.latest_by_arrival().is_none());
        assert!(matches!(
            playback.load_state,
            PlaybackLoadState::Buffering { .. }
        ));

        playback.source.inject_completed_window(
            viewer_core::ArrivalTime(2_000_000_000),
            vec![camera_message(1_050_000_000)],
        );
        playback.source.poll_completed_fetch().unwrap();
        assert!(playback.commit_candidate(elapsed, requested));
        assert_eq!(playback.clock.cursor(), requested);
        assert_eq!(
            playback
                .state()
                .camera
                .latest_by_arrival()
                .unwrap()
                .arrival_time,
            viewer_core::ArrivalTime(1_050_000_000)
        );
    }

    #[test]
    fn remote_seek_and_non_normal_speed_are_explicitly_unsupported() {
        let client = RemoteApiClient::new("http://localhost").unwrap();
        let mut playback = RemotePlayback::new(client, remote_catalog()).unwrap();
        assert!(
            playback
                .apply_command(PlaybackCommand::Seek(viewer_core::ArrivalTime(2)))
                .is_err()
        );
        assert!(
            playback
                .apply_command(PlaybackCommand::SetSpeed(PlaybackSpeed::Double))
                .is_err()
        );
    }
}
