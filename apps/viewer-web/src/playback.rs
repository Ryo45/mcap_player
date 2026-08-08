#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

#[cfg(target_arch = "wasm32")]
use crate::local::BrowserMcapRecording;
use crate::{
    data_plane::{RecordingDataPlane, RecordingDataPlaneDiagnostics, WebWindowLoader},
    remote::{RemoteApiClient, RemoteCatalog, RemoteWindowLoader},
};
use std::{error::Error, fmt, time::Duration};
use viewer_core::{
    CameraId, DomainState, PipelineCounters, PlaybackClock, PlaybackCommand, PlaybackCore,
    PlaybackEffect, PlaybackLoadState, PlaybackPerformance, PlaybackSpeed, StreamCatalog,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlaybackSourceKind {
    LocalFile,
    Remote,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WebPlaybackError(String);

impl WebPlaybackError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for WebPlaybackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for WebPlaybackError {}

/// Browser playback shared by local File.slice and Recording Server sources.
pub(crate) struct WebPlayback {
    clock: PlaybackClock,
    core: PlaybackCore,
    data: RecordingDataPlane<WebWindowLoader>,
    load_state: PlaybackLoadState,
    source_kind: PlaybackSourceKind,
}

impl WebPlayback {
    pub(crate) fn from_remote(
        client: RemoteApiClient,
        catalog: RemoteCatalog,
    ) -> Result<Self, WebPlaybackError> {
        let loader = RemoteWindowLoader::new(
            client,
            catalog.recording_id,
            catalog.revision,
            catalog.selected_streams,
        )
        .map_err(|error| WebPlaybackError::new(error.to_string()))?;
        Self::new(
            &catalog.core,
            &catalog.primary_camera_topic,
            catalog.start,
            catalog.end,
            catalog.end_exclusive,
            WebWindowLoader::Remote(loader),
            PlaybackSourceKind::Remote,
        )
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn from_local(recording: BrowserMcapRecording) -> Result<Self, WebPlaybackError> {
        Self::new(
            &recording.catalog.core,
            &recording.catalog.primary_camera_topic,
            recording.catalog.start,
            recording.catalog.end,
            recording.catalog.end_exclusive,
            WebWindowLoader::LocalFile(recording.loader),
            PlaybackSourceKind::LocalFile,
        )
    }

    fn new(
        catalog: &StreamCatalog,
        primary_camera_topic: &str,
        start: viewer_core::ArrivalTime,
        end: viewer_core::ArrivalTime,
        end_exclusive: viewer_core::ArrivalTime,
        loader: WebWindowLoader,
        source_kind: PlaybackSourceKind,
    ) -> Result<Self, WebPlaybackError> {
        let core = PlaybackCore::new(catalog, primary_camera_topic)
            .map_err(|error| WebPlaybackError::new(error.to_string()))?;
        let data = RecordingDataPlane::new(loader, start, end_exclusive)
            .map_err(|error| WebPlaybackError::new(error.to_string()))?;
        Ok(Self {
            clock: PlaybackClock::new(start, end),
            core,
            data,
            load_state: PlaybackLoadState::Ready,
            source_kind,
        })
    }

    pub(crate) fn tick(&mut self, elapsed: Duration) -> Result<(), WebPlaybackError> {
        if let Err(error) = self.data.poll(self.clock.cursor()) {
            self.load_state = PlaybackLoadState::Failed {
                message: error.to_string(),
            };
            return Err(WebPlaybackError::new(error.to_string()));
        }
        let requested = self.clock.cursor_after(elapsed);
        self.data
            .ensure_available_through(requested)
            .map_err(|error| WebPlaybackError::new(error.to_string()))?;
        if !self.commit_candidate(elapsed, requested) {
            return Ok(());
        }

        self.data
            .ensure_available_through(requested)
            .map_err(|error| WebPlaybackError::new(error.to_string()))?;
        Ok(())
    }

    fn commit_candidate(&mut self, elapsed: Duration, requested: viewer_core::ArrivalTime) -> bool {
        if !self.data.is_complete_through(requested) {
            if !matches!(self.load_state, PlaybackLoadState::Buffering { .. }) {
                self.data.note_buffering();
            }
            self.load_state = PlaybackLoadState::Buffering {
                requested,
                committed: self.clock.cursor(),
            };
            return false;
        }

        let messages = self.data.messages_through(self.clock.cursor(), requested);
        self.core.process_forward(elapsed, messages);
        self.clock.commit_cursor(requested);
        self.load_state = PlaybackLoadState::Ready;
        true
    }

    pub(crate) fn apply_command(
        &mut self,
        command: PlaybackCommand,
    ) -> Result<PlaybackEffect, WebPlaybackError> {
        match command {
            PlaybackCommand::Toggle => {
                self.clock.toggle();
                Ok(PlaybackEffect::None)
            }
            PlaybackCommand::SetSpeed(speed) => {
                if self.is_remote() && speed != PlaybackSpeed::Normal {
                    return Err(WebPlaybackError::new(
                        "Remote playback currently supports 1x speed only",
                    ));
                }
                self.clock.set_speed(speed);
                Ok(PlaybackEffect::None)
            }
            PlaybackCommand::Seek(_) => Err(WebPlaybackError::new(
                "Web DataPlane seek is not implemented yet",
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

    pub(crate) fn is_remote(&self) -> bool {
        self.source_kind == PlaybackSourceKind::Remote
    }

    pub(crate) fn source_label(&self) -> &'static str {
        match self.source_kind {
            PlaybackSourceKind::LocalFile => "Browser file",
            PlaybackSourceKind::Remote => "Recording Server",
        }
    }

    pub(crate) fn data_plane_diagnostics(&self) -> RecordingDataPlaneDiagnostics {
        self.data.diagnostics(self.clock.cursor())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use viewer_core::{
        CompressedImage, DataWindowTimeRange, MeasurementTime, RawMessage, SerializedWindow,
        StreamId, encode_compressed_image_cdr,
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
        crate::remote::adapt_catalog(&catalog).unwrap()
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
        let mut playback = WebPlayback::from_remote(client, remote_catalog()).unwrap();
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

        playback.data.ensure_available_through(requested).unwrap();
        playback
            .data
            .loader_mut()
            .remote_mut()
            .inject_loaded(crate::data_plane::LoadedWindow {
                window: SerializedWindow::new(
                    DataWindowTimeRange::new(
                        viewer_core::ArrivalTime(1_000_000_000),
                        viewer_core::ArrivalTime(2_000_000_000),
                    )
                    .unwrap(),
                    vec![camera_message(1_050_000_000)],
                    64,
                )
                .unwrap(),
                diagnostics: crate::data_plane::WindowLoadDiagnostics {
                    source_reads: 1,
                    source_bytes: 64,
                    latency_ms: 1.0,
                },
            })
            .unwrap();
        playback.data.poll(playback.clock.cursor()).unwrap();
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
    fn web_data_plane_seek_and_remote_non_normal_speed_are_explicitly_unsupported() {
        let client = RemoteApiClient::new("http://localhost").unwrap();
        let mut playback = WebPlayback::from_remote(client, remote_catalog()).unwrap();
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
