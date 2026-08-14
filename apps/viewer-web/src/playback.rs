#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

#[cfg(target_arch = "wasm32")]
use crate::local::BrowserMcapRecording;
use crate::{
    data_plane::{RecordingDataPlane, RecordingDataPlaneDiagnostics, WebWindowLoader},
    remote::{RemoteApiClient, RemoteCatalog, RemoteWindowLoader},
};
use std::{error::Error, fmt, time::Duration};
use viewer_core::{
    ArrivalTime, CameraId, DomainRuntime, DomainState, FetchIntent, PipelineCounters,
    PlaybackClock, PlaybackCommand, PlaybackEffect, PlaybackLoadState, PlaybackPerformance,
    SessionPlan, StageTiming,
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
    domain: DomainRuntime,
    data: RecordingDataPlane<WebWindowLoader>,
    load_state: PlaybackLoadState,
    pending_seek: Option<ArrivalTime>,
    buffer_underrun_active: bool,
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
            catalog.plan,
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
            recording.catalog.plan,
            recording.catalog.start,
            recording.catalog.end,
            recording.catalog.end_exclusive,
            WebWindowLoader::LocalFile(recording.loader),
            PlaybackSourceKind::LocalFile,
        )
    }

    fn new(
        plan: SessionPlan,
        start: viewer_core::ArrivalTime,
        end: viewer_core::ArrivalTime,
        end_exclusive: viewer_core::ArrivalTime,
        loader: WebWindowLoader,
        source_kind: PlaybackSourceKind,
    ) -> Result<Self, WebPlaybackError> {
        let domain = DomainRuntime::new(plan);
        let data = RecordingDataPlane::new(loader, start, end_exclusive)
            .map_err(|error| WebPlaybackError::new(error.to_string()))?;
        Ok(Self {
            clock: PlaybackClock::new(start, end),
            domain,
            data,
            load_state: PlaybackLoadState::Ready,
            pending_seek: None,
            buffer_underrun_active: false,
            source_kind,
        })
    }

    pub(crate) fn tick(&mut self, elapsed: Duration) -> Result<PlaybackEffect, WebPlaybackError> {
        if let Err(error) = self.data.poll(self.clock.cursor()) {
            self.load_state = PlaybackLoadState::Failed {
                message: error.to_string(),
            };
            return Err(WebPlaybackError::new(error.to_string()));
        }
        if let Some(target) = self.pending_seek {
            return self.tick_seek(target);
        }

        let requested = self.clock.cursor_after(elapsed);
        let committed = self.clock.cursor();
        let speed = self.clock.speed();
        let intent = if self.clock.is_playing() {
            FetchIntent::PlaybackAhead
        } else {
            FetchIntent::RequiredOnly
        };
        self.data
            .ensure_available_through(committed, requested, speed, intent)
            .map_err(|error| WebPlaybackError::new(error.to_string()))?;
        if !self.commit_candidate(elapsed, requested) {
            return Ok(PlaybackEffect::None);
        }

        self.data
            .ensure_available_through(requested, requested, speed, intent)
            .map_err(|error| WebPlaybackError::new(error.to_string()))?;
        Ok(PlaybackEffect::None)
    }

    fn tick_seek(&mut self, target: ArrivalTime) -> Result<PlaybackEffect, WebPlaybackError> {
        self.data
            .ensure_available_through(
                target,
                target,
                self.clock.speed(),
                FetchIntent::RequiredOnly,
            )
            .map_err(|error| WebPlaybackError::new(error.to_string()))?;
        if !self.data.is_complete_through(target) {
            self.load_state = PlaybackLoadState::Seeking { target };
            return Ok(PlaybackEffect::None);
        }

        let messages = self.data.messages_through(target, target);
        self.domain.reset_for_restore();
        self.domain.process(Duration::ZERO, messages);
        self.clock.seek(target);
        self.pending_seek = None;
        self.buffer_underrun_active = false;
        self.load_state = PlaybackLoadState::Ready;

        if self.clock.is_playing() {
            self.data
                .ensure_available_through(
                    target,
                    target,
                    self.clock.speed(),
                    FetchIntent::PlaybackAhead,
                )
                .map_err(|error| WebPlaybackError::new(error.to_string()))?;
        }
        Ok(PlaybackEffect::Seeked)
    }

    fn commit_candidate(&mut self, elapsed: Duration, requested: viewer_core::ArrivalTime) -> bool {
        if !self.data.is_complete_through(requested) {
            let is_underrun = self.clock.is_playing() && requested > self.clock.cursor();
            if is_underrun && !self.buffer_underrun_active {
                self.data.note_buffer_underrun();
            }
            self.buffer_underrun_active = is_underrun;
            self.load_state = PlaybackLoadState::Buffering {
                requested,
                committed: self.clock.cursor(),
            };
            return false;
        }

        let messages = self.data.messages_through(self.clock.cursor(), requested);
        self.domain.process(elapsed, messages);
        self.clock.commit_cursor(requested);
        self.buffer_underrun_active = false;
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
                self.clock.set_speed(speed);
                Ok(PlaybackEffect::None)
            }
            PlaybackCommand::Seek(cursor) => {
                let target = cursor.clamp(self.clock.start(), self.clock.end());
                if self.pending_seek == Some(target) {
                    return Ok(PlaybackEffect::None);
                }
                self.data
                    .begin_seek(target)
                    .map_err(|error| WebPlaybackError::new(error.to_string()))?;
                self.pending_seek = Some(target);
                self.buffer_underrun_active = false;
                self.load_state = PlaybackLoadState::Seeking { target };
                Ok(PlaybackEffect::None)
            }
        }
    }

    pub(crate) fn clock(&self) -> &PlaybackClock {
        &self.clock
    }

    pub(crate) fn state(&self) -> &DomainState {
        self.domain.state()
    }

    pub(crate) fn camera_topics(&self) -> &[(CameraId, String)] {
        self.domain.camera_topics()
    }

    pub(crate) fn set_focused_camera(&mut self, camera: Option<CameraId>) {
        self.domain.set_focused_camera(camera);
    }

    pub(crate) fn counters(&self) -> PipelineCounters {
        self.domain.counters()
    }

    pub(crate) fn performance(&self) -> PlaybackPerformance {
        PlaybackPerformance::from_parts(StageTiming::default(), self.domain.performance())
    }

    pub(crate) fn load_state(&self) -> PlaybackLoadState {
        self.load_state.clone()
    }

    pub(crate) fn display_cursor(&self) -> ArrivalTime {
        self.pending_seek.unwrap_or_else(|| self.clock.cursor())
    }

    pub(crate) fn source_label(&self) -> &'static str {
        match self.source_kind {
            PlaybackSourceKind::LocalFile => "Browser file",
            PlaybackSourceKind::Remote => "Recording Server",
        }
    }

    pub(crate) fn data_plane_diagnostics(&self) -> RecordingDataPlaneDiagnostics {
        self.data
            .diagnostics(self.clock.cursor(), self.clock.speed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use viewer_core::{
        CompressedImage, DataWindowTimeRange, MeasurementTime, PlaybackSpeed, RawMessage,
        SerializedWindow, StreamId, encode_compressed_image_cdr,
    };
    use viewer_remote_protocol::{
        BatchEncoder, CatalogResponse, RemoteMessageRef, RemoteTimeRange, StreamDescriptor,
        StreamSemantic, TimestampNs,
    };

    fn remote_catalog_until(end_ns_exclusive: u64) -> RemoteCatalog {
        let catalog = CatalogResponse::new(
            "demo".into(),
            "revision".into(),
            RemoteTimeRange {
                start_ns: TimestampNs::new(1_000_000_000),
                end_ns_exclusive: TimestampNs::new(end_ns_exclusive),
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

    fn remote_catalog() -> RemoteCatalog {
        remote_catalog_until(3_000_000_000)
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

    fn loaded_window(
        start: i64,
        end_exclusive: i64,
        messages: Vec<RawMessage>,
    ) -> crate::data_plane::LoadedWindow {
        let resident_bytes = messages.iter().map(|message| message.payload.len()).sum();
        crate::data_plane::LoadedWindow {
            window: SerializedWindow::new(
                DataWindowTimeRange::new(
                    viewer_core::ArrivalTime(start),
                    viewer_core::ArrivalTime(end_exclusive),
                )
                .unwrap(),
                messages,
                resident_bytes,
            )
            .unwrap(),
            diagnostics: crate::data_plane::WindowLoadDiagnostics {
                source_reads: 1,
                source_bytes: resident_bytes,
                decompressed_bytes: 0,
                per_message_copied_bytes: 0,
                latency_ms: 1.0,
                processing_ms: 0.0,
            },
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
        assert_eq!(
            playback
                .data
                .diagnostics(playback.clock.cursor(), playback.clock.speed())
                .buffer_underrun_count,
            1
        );

        assert!(!playback.commit_candidate(elapsed, requested));
        assert_eq!(
            playback
                .data
                .diagnostics(playback.clock.cursor(), playback.clock.speed())
                .buffer_underrun_count,
            1,
            "one continuous buffering period is one underrun"
        );

        playback
            .data
            .ensure_available_through(
                playback.clock.cursor(),
                requested,
                playback.clock.speed(),
                FetchIntent::PlaybackAhead,
            )
            .unwrap();
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
                    decompressed_bytes: 0,
                    per_message_copied_bytes: 0,
                    latency_ms: 1.0,
                    processing_ms: 0.0,
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
    fn initial_paused_loading_is_not_counted_as_a_buffer_underrun() {
        let client = RemoteApiClient::new("http://localhost").unwrap();
        let mut playback = WebPlayback::from_remote(client, remote_catalog()).unwrap();
        let paused_target = playback.clock.cursor();

        assert!(!playback.commit_candidate(Duration::ZERO, paused_target));
        assert_eq!(playback.data_plane_diagnostics().buffer_underrun_count, 0);

        playback.apply_command(PlaybackCommand::Toggle).unwrap();
        let requested = playback.clock.cursor_after(Duration::from_millis(10));
        assert!(!playback.commit_candidate(Duration::from_millis(10), requested));
        assert_eq!(playback.data_plane_diagnostics().buffer_underrun_count, 1);
    }

    #[test]
    fn paused_playback_stops_after_the_required_window_instead_of_prefetching() {
        let client = RemoteApiClient::new("http://localhost").unwrap();
        let mut playback = WebPlayback::from_remote(client, remote_catalog()).unwrap();

        playback.tick(Duration::ZERO).unwrap();
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
                    vec![],
                    0,
                )
                .unwrap(),
                diagnostics: crate::data_plane::WindowLoadDiagnostics {
                    source_reads: 1,
                    source_bytes: 0,
                    decompressed_bytes: 0,
                    per_message_copied_bytes: 0,
                    latency_ms: 1.0,
                    processing_ms: 0.0,
                },
            })
            .unwrap();

        playback.tick(Duration::ZERO).unwrap();

        assert!(
            playback.data.loader_mut().remote_mut().is_idle(),
            "paused playback must not keep filling target-ahead after its cursor is serviceable"
        );
    }

    #[test]
    fn pausing_allows_one_in_flight_window_to_finish_without_starting_another() {
        let client = RemoteApiClient::new("http://localhost").unwrap();
        let mut playback =
            WebPlayback::from_remote(client, remote_catalog_until(5_000_000_000)).unwrap();
        playback.apply_command(PlaybackCommand::Toggle).unwrap();

        playback.tick(Duration::ZERO).unwrap();
        playback
            .data
            .loader_mut()
            .remote_mut()
            .inject_loaded(loaded_window(1_000_000_000, 2_000_000_000, vec![]))
            .unwrap();
        playback.tick(Duration::ZERO).unwrap();
        assert!(
            !playback.data.loader_mut().remote_mut().is_idle(),
            "playing should keep one background prefetch active below target-ahead"
        );

        playback.apply_command(PlaybackCommand::Toggle).unwrap();
        playback
            .data
            .loader_mut()
            .remote_mut()
            .inject_loaded(loaded_window(2_000_000_000, 3_000_000_000, vec![]))
            .unwrap();
        playback.tick(Duration::ZERO).unwrap();

        assert!(
            playback.data.loader_mut().remote_mut().is_idle(),
            "Pause keeps the completed window but must not start another prefetch"
        );
    }

    #[test]
    fn remote_speed_updates_prefetch_target() {
        let client = RemoteApiClient::new("http://localhost").unwrap();
        let mut playback = WebPlayback::from_remote(client, remote_catalog()).unwrap();
        playback
            .apply_command(PlaybackCommand::SetSpeed(PlaybackSpeed::Double))
            .unwrap();
        assert_eq!(playback.clock.speed(), PlaybackSpeed::Double);
        assert_eq!(
            playback.data_plane_diagnostics().target_ahead,
            Duration::from_secs(4)
        );
    }

    #[test]
    fn seek_replaces_old_prefetch_and_commits_only_the_current_generation() {
        const START: i64 = 1_000_000_000;
        const TARGET: i64 = 2_000_000_000;
        const END_EXCLUSIVE: i64 = 3_000_000_000;

        let client = RemoteApiClient::new("http://localhost").unwrap();
        let mut playback = WebPlayback::from_remote(client, remote_catalog()).unwrap();

        playback.tick(Duration::ZERO).unwrap();
        playback
            .data
            .loader_mut()
            .remote_mut()
            .inject_loaded(loaded_window(START, TARGET, vec![camera_message(START)]))
            .unwrap();
        assert_eq!(playback.tick(Duration::ZERO).unwrap(), PlaybackEffect::None);
        assert_eq!(
            playback
                .state()
                .camera
                .latest_by_arrival()
                .unwrap()
                .arrival_time,
            viewer_core::ArrivalTime(START)
        );

        playback.apply_command(PlaybackCommand::Toggle).unwrap();
        playback.tick(Duration::from_millis(10)).unwrap();
        let committed_before_seek = playback.clock.cursor();
        assert!(!playback.data.loader_mut().remote_mut().is_idle());

        assert_eq!(
            playback
                .apply_command(PlaybackCommand::Seek(viewer_core::ArrivalTime(TARGET)))
                .unwrap(),
            PlaybackEffect::None
        );
        assert_eq!(playback.clock.cursor(), committed_before_seek);
        assert_eq!(
            playback
                .state()
                .camera
                .latest_by_arrival()
                .unwrap()
                .arrival_time,
            viewer_core::ArrivalTime(START),
            "committed domain stays visible while the seek window is loading"
        );
        assert_eq!(
            playback.load_state(),
            PlaybackLoadState::Seeking {
                target: viewer_core::ArrivalTime(TARGET)
            }
        );

        playback.tick(Duration::ZERO).unwrap();
        playback
            .data
            .loader_mut()
            .remote_mut()
            .inject_stale(loaded_window(
                TARGET,
                END_EXCLUSIVE,
                vec![camera_message(TARGET)],
            ))
            .unwrap();
        assert_eq!(playback.tick(Duration::ZERO).unwrap(), PlaybackEffect::None);
        assert_eq!(playback.clock.cursor(), committed_before_seek);
        assert_eq!(playback.data_plane_diagnostics().stale_results_discarded, 1);

        playback
            .data
            .loader_mut()
            .remote_mut()
            .inject_loaded(loaded_window(
                TARGET,
                END_EXCLUSIVE,
                vec![camera_message(TARGET)],
            ))
            .unwrap();
        assert_eq!(
            playback.tick(Duration::ZERO).unwrap(),
            PlaybackEffect::Seeked
        );
        assert_eq!(playback.clock.cursor(), viewer_core::ArrivalTime(TARGET));
        assert_eq!(
            playback
                .state()
                .camera
                .latest_by_arrival()
                .unwrap()
                .arrival_time,
            viewer_core::ArrivalTime(TARGET)
        );
        assert_eq!(playback.load_state(), PlaybackLoadState::Ready);
    }

    #[test]
    fn local_indexed_and_remote_batch_windows_reduce_to_the_same_domain_state() {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap"),
        )
        .unwrap();
        let summary = mcap::Summary::read(&bytes).unwrap().unwrap();
        let catalog =
            crate::local::LocalCatalog::from_summary(&summary, "/camera/front/image/compressed")
                .unwrap();
        let window_end = viewer_core::ArrivalTime(
            catalog
                .start
                .0
                .saturating_add(1_000_000_000)
                .min(catalog.end_exclusive.0),
        );
        let range = DataWindowTimeRange::new(catalog.start, window_end).unwrap();
        let local = crate::local::collect_window_from_bytes_for_test(
            &summary,
            &catalog.selected_topics,
            range,
            &bytes,
        )
        .unwrap();
        let camera_stream = catalog
            .core
            .by_topic("/camera/front/image/compressed")
            .unwrap()
            .id;
        let camera_payload_ranges = local
            .window
            .messages
            .iter()
            .filter(|message| message.stream_id == camera_stream)
            .map(|message| {
                let start = message.payload.as_ptr() as usize;
                start..start + message.payload.len()
            })
            .collect::<Vec<_>>();

        let mut encoder = BatchEncoder::new();
        for (sequence, message) in local.window.messages.iter().enumerate() {
            encoder
                .push(RemoteMessageRef {
                    stream_id: message.stream_id.0,
                    sequence: sequence as u32,
                    log_time_ns: message.arrival_time.0 as u64,
                    publish_time_ns: message.arrival_time.0 as u64,
                    payload: &message.payload,
                })
                .unwrap();
        }
        let remote = crate::remote::assemble_pages_for_test(
            range,
            [crate::remote::RemoteBatchPage {
                body: encoder.finish(),
                complete: true,
                next_cursor: None,
                message_count: local.window.messages.len(),
                recording_revision: "test".into(),
            }],
        )
        .unwrap();
        assert_eq!(local.window.messages, remote.window.messages);

        let mut local_domain = DomainRuntime::new(catalog.plan.clone());
        let mut remote_domain = DomainRuntime::new(catalog.plan);
        local_domain.process(Duration::from_secs(1), local.window.messages);
        remote_domain.process(Duration::from_secs(1), remote.window.messages);

        let local_cameras = local_domain
            .state()
            .camera
            .frames()
            .map(|(id, frame)| (*id, frame.clone()))
            .collect::<Vec<_>>();
        let remote_cameras = remote_domain
            .state()
            .camera
            .frames()
            .map(|(id, frame)| (*id, frame.clone()))
            .collect::<Vec<_>>();
        assert_eq!(local_cameras, remote_cameras);
        let retained_jpeg = &local_cameras.first().unwrap().1.jpeg;
        let jpeg_start = retained_jpeg.as_ptr() as usize;
        assert!(camera_payload_ranges.iter().any(|payload| {
            jpeg_start >= payload.start && jpeg_start + retained_jpeg.len() <= payload.end
        }));
        assert_eq!(
            local_domain.state().telemetry.latest(),
            remote_domain.state().telemetry.latest()
        );
        assert_eq!(
            local_domain.state().bev.latest(),
            remote_domain.state().bev.latest()
        );
        assert_eq!(
            local_domain.state().point_cloud.latest(),
            remote_domain.state().point_cloud.latest()
        );
        assert_eq!(local_domain.counters(), remote_domain.counters());
    }
}
