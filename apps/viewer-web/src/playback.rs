#[cfg(target_arch = "wasm32")]
use crate::local::{BrowserMcapRecording, BrowserMcapRestoreLoader};
use crate::{
    data_plane::{RecordingDataPlane, RecordingDataPlaneDiagnostics, WebWindowLoader},
    remote::{RemoteApiClient, RemoteCatalog, RemoteRestoreLoader, RemoteWindowLoader},
};
use std::{error::Error, fmt, time::Duration};
use viewer_core::{
    ArrivalTime, CameraController, FeatureRuntime, FetchIntent, PlaybackClock, PlaybackCommand,
    PlaybackEffect, PlaybackLoadState, PlaybackRequirements, ProcessingCounters, RawMessage,
    RestorePlanner, SessionPlan, SourceCatalog, TransformController, WorkspaceBindings,
};
#[cfg(any(test, target_arch = "wasm32"))]
use viewer_core::{CameraId, OdometryController, PathController};
#[cfg(target_arch = "wasm32")]
use viewer_core::{PlaybackPerformance, StageTiming};

const WEB_WORKSPACE_BINDINGS: &str = include_str!("../../../config/workspace_bindings.json");

pub(crate) fn web_workspace_bindings() -> WorkspaceBindings {
    serde_json::from_str(WEB_WORKSPACE_BINDINGS)
        .expect("bundled Web workspace bindings must be valid")
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlaybackSourceKind {
    LocalFile,
    Remote,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingSeek {
    target: ArrivalTime,
    persistent_streams: Vec<viewer_core::StreamId>,
    bootstraps_persistent: bool,
}

enum WebRestoreLoader {
    Remote(RemoteRestoreLoader),
    #[cfg(target_arch = "wasm32")]
    LocalFile(BrowserMcapRestoreLoader),
}

impl WebRestoreLoader {
    fn request(&mut self, plan: viewer_core::RestorePlan) -> Result<(), WebPlaybackError> {
        match self {
            Self::Remote(loader) => loader.request(&plan),
            #[cfg(target_arch = "wasm32")]
            Self::LocalFile(loader) => loader.request(plan),
        }
        .map_err(|error| WebPlaybackError::new(error.to_string()))
    }

    fn poll(&mut self) -> Option<Result<Vec<RawMessage>, WebPlaybackError>> {
        match self {
            Self::Remote(loader) => loader.poll(),
            #[cfg(target_arch = "wasm32")]
            Self::LocalFile(loader) => loader.poll(),
        }
        .map(|result| result.map_err(|error| WebPlaybackError::new(error.to_string())))
    }

    #[cfg(test)]
    fn remote_mut(&mut self) -> &mut RemoteRestoreLoader {
        match self {
            Self::Remote(loader) => loader,
            #[cfg(target_arch = "wasm32")]
            Self::LocalFile(_) => panic!("test expected Remote restore loader"),
        }
    }
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
    runtime: FeatureRuntime,
    catalog: SourceCatalog,
    restore_inputs: Vec<viewer_core::RestoreInput>,
    data: RecordingDataPlane<WebWindowLoader>,
    restore: WebRestoreLoader,
    persistent_archive: Option<Vec<RawMessage>>,
    load_state: PlaybackLoadState,
    pending_seek: Option<PendingSeek>,
    buffer_underrun_active: bool,
    #[cfg(target_arch = "wasm32")]
    source_kind: PlaybackSourceKind,
}

impl WebPlayback {
    pub(crate) fn from_remote(
        client: RemoteApiClient,
        catalog: RemoteCatalog,
    ) -> Result<Self, WebPlaybackError> {
        let restore = RemoteRestoreLoader::new(
            client.clone(),
            catalog.recording_id.clone(),
            catalog.revision.clone(),
        );
        let loader = RemoteWindowLoader::new(
            client,
            catalog.recording_id,
            catalog.revision,
            catalog.selected_streams,
        )
        .map_err(|error| WebPlaybackError::new(error.to_string()))?;
        Self::new(
            catalog.plan,
            catalog.core,
            WebWindowLoader::Remote(loader),
            WebRestoreLoader::Remote(restore),
        )
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn from_local(recording: BrowserMcapRecording) -> Result<Self, WebPlaybackError> {
        let BrowserMcapRecording {
            catalog,
            loader,
            restore,
        } = recording;
        Self::new(
            catalog.plan,
            catalog.core,
            WebWindowLoader::LocalFile(loader),
            WebRestoreLoader::LocalFile(restore),
        )
    }

    fn new(
        plan: SessionPlan,
        catalog: SourceCatalog,
        loader: WebWindowLoader,
        restore: WebRestoreLoader,
    ) -> Result<Self, WebPlaybackError> {
        let recording = catalog
            .time_range
            .ok_or_else(|| WebPlaybackError::new("recording catalog has no time range"))?;
        let start = recording.start;
        let end_exclusive = recording.end_exclusive;
        let end = ArrivalTime(end_exclusive.0 - 1);
        let runtime = FeatureRuntime::new(&plan, false);
        let restore_inputs = plan.restore_inputs();
        #[cfg(target_arch = "wasm32")]
        let source_kind = match &loader {
            WebWindowLoader::Remote(_) => PlaybackSourceKind::Remote,
            WebWindowLoader::LocalFile(_) => PlaybackSourceKind::LocalFile,
        };
        let data = RecordingDataPlane::new(loader, start, end_exclusive)
            .map_err(|error| WebPlaybackError::new(error.to_string()))?;
        Ok(Self {
            clock: PlaybackClock::new(start, end),
            runtime,
            catalog,
            restore_inputs,
            data,
            restore,
            persistent_archive: None,
            load_state: PlaybackLoadState::Ready,
            pending_seek: None,
            buffer_underrun_active: false,
            #[cfg(target_arch = "wasm32")]
            source_kind,
        })
    }

    pub(crate) fn tick(&mut self, elapsed: Duration) -> Result<PlaybackEffect, WebPlaybackError> {
        if let Some(seek) = self.pending_seek.clone() {
            return self.tick_seek(seek);
        }
        if let Err(error) = self.data.poll(self.clock.cursor()) {
            self.load_state = PlaybackLoadState::Failed {
                message: error.to_string(),
            };
            return Err(WebPlaybackError::new(error.to_string()));
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

    fn tick_seek(&mut self, seek: PendingSeek) -> Result<PlaybackEffect, WebPlaybackError> {
        let target = seek.target;
        let Some(result) = self.restore.poll() else {
            self.load_state = PlaybackLoadState::Seeking { target };
            return Ok(PlaybackEffect::None);
        };
        let mut messages = match result {
            Ok(messages) => messages,
            Err(error) => {
                self.pending_seek = None;
                self.load_state = PlaybackLoadState::Failed {
                    message: error.to_string(),
                };
                return Err(error);
            }
        };
        let mut candidate_archive = self.persistent_archive.clone();
        if seek.bootstraps_persistent {
            let persistent = seek
                .persistent_streams
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>();
            let mut archive = Vec::new();
            messages.retain(|message| {
                if persistent.contains(&message.stream_id) {
                    archive.push(message.clone());
                    false
                } else {
                    true
                }
            });
            candidate_archive = Some(archive);
        }
        if let Some(archive) = &candidate_archive {
            messages.extend(
                archive
                    .iter()
                    .filter(|message| {
                        seek.persistent_streams.contains(&message.stream_id)
                            && message.arrival_time <= target
                    })
                    .cloned(),
            );
        }
        messages.sort_by_key(|message| (message.arrival_time, message.stream_id.0));
        let candidate_runtime = match self.runtime.stage_restore(target, &messages) {
            Ok(runtime) => runtime,
            Err(error) => {
                self.pending_seek = None;
                self.load_state = PlaybackLoadState::Failed {
                    message: error.to_string(),
                };
                return Err(WebPlaybackError::new(error.to_string()));
            }
        };
        self.data
            .begin_seek(target)
            .map_err(|error| WebPlaybackError::new(error.to_string()))?;
        self.runtime.commit_restore(candidate_runtime);
        self.persistent_archive = candidate_archive;
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
        self.runtime.process_messages(elapsed, &messages);
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
                if self
                    .pending_seek
                    .as_ref()
                    .is_some_and(|seek| seek.target == target)
                {
                    return Ok(PlaybackEffect::None);
                }
                let mut restore = RestorePlanner::new(&self.catalog)
                    .plan(target, self.restore_inputs.iter().copied())
                    .map_err(|error| WebPlaybackError::new(error.to_string()))?;
                let persistent_streams = restore.persistent.clone();
                let bootstraps_persistent =
                    !persistent_streams.is_empty() && self.persistent_archive.is_none();
                if !bootstraps_persistent {
                    restore.persistent.clear();
                }
                self.data.cancel_pending();
                self.restore.request(restore)?;
                self.pending_seek = Some(PendingSeek {
                    target,
                    persistent_streams,
                    bootstraps_persistent,
                });
                self.buffer_underrun_active = false;
                self.load_state = PlaybackLoadState::Seeking { target };
                Ok(PlaybackEffect::None)
            }
        }
    }

    pub(crate) fn clock(&self) -> &PlaybackClock {
        &self.clock
    }

    pub(crate) fn cameras(&self) -> &CameraController {
        self.runtime.cameras()
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn path(&self) -> &PathController {
        self.runtime.path()
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn odometry(&self) -> &OdometryController {
        self.runtime.odometry()
    }

    pub(crate) fn transforms(&self) -> &TransformController {
        self.runtime.transforms()
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn camera_topics(&self) -> &[(CameraId, String)] {
        self.runtime.cameras().topics()
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn set_focused_camera(&mut self, camera: Option<CameraId>) {
        self.runtime.set_scheduling_priority(camera);
    }

    pub(crate) fn counters(&self) -> ProcessingCounters {
        self.runtime.counters()
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn performance(&self) -> PlaybackPerformance {
        self.runtime.playback_performance(StageTiming::default())
    }

    pub(crate) fn load_state(&self) -> PlaybackLoadState {
        self.load_state.clone()
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn display_cursor(&self) -> ArrivalTime {
        self.clock.cursor()
    }

    #[cfg(target_arch = "wasm32")]
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

pub(crate) fn web_playback_requirements() -> PlaybackRequirements {
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_all_cameras();
    requirements.optional_path();
    requirements.optional_odometry();
    requirements.optional_transforms();
    requirements
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use viewer_core::{
        CompressedImage, DataWindowTimeRange, MeasurementTime, PlaybackSpeed, RawMessage,
        SerializedWindow, StreamId, TransformStamped, encode_compressed_image_cdr,
        encode_tf_message_cdr,
    };
    use viewer_remote_protocol::{
        BatchEncoder, CatalogResponse, RemoteMessageRef, RemoteTimeRange, StreamDescriptor,
        TimestampNs,
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
                schema_name: "sensor_msgs/msg/CompressedImage".into(),
                schema_encoding: "ros2msg".into(),
                message_encoding: "cdr".into(),
                message_count: Some(viewer_remote_protocol::MessageCount::new(30)),
            }],
        );
        crate::remote::adapt_catalog(&catalog).unwrap()
    }

    fn remote_catalog() -> RemoteCatalog {
        remote_catalog_until(3_000_000_000)
    }

    fn remote_catalog_with_static_tf() -> RemoteCatalog {
        let catalog = CatalogResponse::new(
            "demo".into(),
            "revision".into(),
            RemoteTimeRange {
                start_ns: TimestampNs::new(1_000_000_000),
                end_ns_exclusive: TimestampNs::new(3_000_000_000),
            },
            vec![
                StreamDescriptor {
                    id: 1,
                    topic: "/camera".into(),
                    schema_name: "sensor_msgs/msg/CompressedImage".into(),
                    schema_encoding: "ros2msg".into(),
                    message_encoding: "cdr".into(),
                    message_count: Some(viewer_remote_protocol::MessageCount::new(30)),
                },
                StreamDescriptor {
                    id: 2,
                    topic: "/tf_static".into(),
                    schema_name: "tf2_msgs/msg/TFMessage".into(),
                    schema_encoding: "ros2msg".into(),
                    message_encoding: "cdr".into(),
                    message_count: Some(viewer_remote_protocol::MessageCount::new(2)),
                },
            ],
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

    fn static_tf_message(time: i64, x: f64) -> RawMessage {
        RawMessage {
            stream_id: StreamId(2),
            arrival_time: ArrivalTime(time),
            payload: Bytes::from(
                encode_tf_message_cdr(&[TransformStamped {
                    measurement_time: MeasurementTime(time),
                    frame_id: "map".into(),
                    child_frame_id: "base".into(),
                    translation: [x, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                }])
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
    fn buffering_keeps_committed_cursor_and_controller_state_until_window_is_complete() {
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
        assert!(playback.cameras().state().latest_by_arrival().is_none());
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
                .cameras()
                .state()
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
                .cameras()
                .state()
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
                .cameras()
                .state()
                .latest_by_arrival()
                .unwrap()
                .arrival_time,
            viewer_core::ArrivalTime(START),
            "committed controller state stays visible while the seek window is loading"
        );
        assert_eq!(
            playback.load_state(),
            PlaybackLoadState::Seeking {
                target: viewer_core::ArrivalTime(TARGET)
            }
        );

        playback
            .restore
            .remote_mut()
            .inject_stale(vec![camera_message(TARGET)]);
        assert_eq!(playback.tick(Duration::ZERO).unwrap(), PlaybackEffect::None);
        assert_eq!(playback.clock.cursor(), committed_before_seek);

        playback
            .restore
            .remote_mut()
            .inject(vec![camera_message(TARGET)]);
        assert_eq!(
            playback.tick(Duration::ZERO).unwrap(),
            PlaybackEffect::Seeked
        );
        assert_eq!(playback.clock.cursor(), viewer_core::ArrivalTime(TARGET));
        assert_eq!(
            playback
                .cameras()
                .state()
                .latest_by_arrival()
                .unwrap()
                .arrival_time,
            viewer_core::ArrivalTime(TARGET)
        );
        assert_eq!(playback.load_state(), PlaybackLoadState::Ready);
    }

    #[test]
    fn failed_restore_keeps_the_old_cursor_and_visible_controller_state() {
        let client = RemoteApiClient::new("http://localhost").unwrap();
        let mut playback = WebPlayback::from_remote(client, remote_catalog()).unwrap();
        playback
            .runtime
            .process_messages(Duration::ZERO, &[camera_message(1_000_000_000)]);
        let committed = playback.clock().cursor();

        playback
            .apply_command(PlaybackCommand::Seek(ArrivalTime(2_000_000_000)))
            .unwrap();
        playback
            .restore
            .remote_mut()
            .inject_error("indexed restore failed");

        assert!(playback.tick(Duration::ZERO).is_err());
        assert_eq!(playback.clock().cursor(), committed);
        assert_eq!(
            playback
                .cameras()
                .state()
                .latest_for(CameraId(0))
                .unwrap()
                .arrival_time,
            ArrivalTime(1_000_000_000)
        );
    }

    #[test]
    fn malformed_restore_application_keeps_cursor_runtime_and_archive_uncommitted() {
        let client = RemoteApiClient::new("http://localhost").unwrap();
        let mut playback = WebPlayback::from_remote(client, remote_catalog()).unwrap();
        playback
            .runtime
            .process_messages(Duration::ZERO, &[camera_message(1_000_000_000)]);
        let committed = playback.clock().cursor();
        let before = playback.cameras().state().latest_for(CameraId(0)).cloned();
        let counters = playback.counters();
        assert!(playback.persistent_archive.is_none());

        playback
            .apply_command(PlaybackCommand::Seek(ArrivalTime(2_000_000_000)))
            .unwrap();
        playback.restore.remote_mut().inject(vec![RawMessage {
            stream_id: StreamId(1),
            arrival_time: ArrivalTime(1_900_000_000),
            payload: Bytes::from_static(&[0xff]),
        }]);

        assert!(playback.tick(Duration::ZERO).is_err());
        assert_eq!(playback.clock().cursor(), committed);
        assert_eq!(
            playback.cameras().state().latest_for(CameraId(0)),
            before.as_ref()
        );
        assert_eq!(playback.counters(), counters);
        assert!(playback.persistent_archive.is_none());
    }

    #[test]
    fn remote_static_tf_archive_is_bootstrapped_once_and_filtered_per_seek() {
        let client = RemoteApiClient::new("http://localhost").unwrap();
        let mut playback =
            WebPlayback::from_remote(client, remote_catalog_with_static_tf()).unwrap();

        playback
            .apply_command(PlaybackCommand::Seek(ArrivalTime(1_500_000_000)))
            .unwrap();
        playback.restore.remote_mut().inject(vec![
            camera_message(1_400_000_000),
            static_tf_message(1_100_000_000, 1.0),
            static_tf_message(2_500_000_000, 2.0),
        ]);
        assert_eq!(
            playback.tick(Duration::ZERO).unwrap(),
            PlaybackEffect::Seeked
        );
        assert_eq!(playback.persistent_archive.as_ref().unwrap().len(), 2);
        assert_eq!(
            playback
                .transforms()
                .state()
                .transform_points("base", "map", &[[0.0; 3]])
                .unwrap(),
            vec![[1.0, 0.0, 0.0]]
        );

        playback
            .apply_command(PlaybackCommand::Seek(ArrivalTime(2_700_000_000)))
            .unwrap();
        playback
            .restore
            .remote_mut()
            .inject(vec![camera_message(2_600_000_000)]);
        assert_eq!(
            playback.tick(Duration::ZERO).unwrap(),
            PlaybackEffect::Seeked
        );
        assert_eq!(playback.persistent_archive.as_ref().unwrap().len(), 2);
        assert_eq!(
            playback
                .transforms()
                .state()
                .transform_points("base", "map", &[[0.0; 3]])
                .unwrap(),
            vec![[2.0, 0.0, 0.0]]
        );
    }

    #[test]
    fn local_indexed_and_remote_batch_windows_reduce_to_the_same_feature_state() {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap"),
        )
        .unwrap();
        let summary = mcap::Summary::read(&bytes).unwrap().unwrap();
        let catalog =
            crate::local::LocalCatalog::from_summary(&summary, "/camera/front/image/compressed")
                .unwrap();
        let recording = catalog.core.time_range.unwrap();
        let window_end = viewer_core::ArrivalTime(
            recording
                .start
                .0
                .saturating_add(1_000_000_000)
                .min(recording.end_exclusive.0),
        );
        let range = DataWindowTimeRange::new(recording.start, window_end).unwrap();
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

        struct ReducedFeatureState {
            cameras: Vec<(CameraId, viewer_core::CameraFrame)>,
            telemetry: Option<viewer_core::TelemetryFrame>,
            path: Option<viewer_core::BevPathFrame>,
            transform_counts: (usize, usize),
            counters: ProcessingCounters,
        }

        fn reduce(plan: &SessionPlan, messages: &[RawMessage]) -> ReducedFeatureState {
            let mut cameras = CameraController::new(plan);
            let mut path = PathController::new(plan);
            let mut odometry = OdometryController::new(plan);
            let mut transforms = TransformController::new(plan);
            for message in messages {
                cameras.admit(message);
                path.process(message);
                odometry.process(message);
                transforms.process(message);
            }
            cameras.advance(Duration::from_secs(1));
            let mut counters = cameras.counters();
            counters.merge(path.counters());
            counters.merge(odometry.counters());
            counters.merge(transforms.counters());
            ReducedFeatureState {
                cameras: cameras
                    .state()
                    .frames()
                    .map(|(id, frame)| (*id, frame.clone()))
                    .collect(),
                telemetry: odometry.state().latest().cloned(),
                path: path.state().latest().cloned(),
                transform_counts: (
                    transforms.state().static_len(),
                    transforms.state().dynamic_len(),
                ),
                counters,
            }
        }

        let local_state = reduce(&catalog.plan, &local.window.messages);
        let remote_state = reduce(&catalog.plan, &remote.window.messages);
        let local_cameras = &local_state.cameras;
        let remote_cameras = &remote_state.cameras;
        assert_eq!(local_cameras, remote_cameras);
        let retained_jpeg = &local_cameras.first().unwrap().1.jpeg;
        let jpeg_start = retained_jpeg.as_ptr() as usize;
        assert!(camera_payload_ranges.iter().any(|payload| {
            jpeg_start >= payload.start && jpeg_start + retained_jpeg.len() <= payload.end
        }));
        assert_eq!(local_state.telemetry, remote_state.telemetry);
        assert_eq!(local_state.path, remote_state.path);
        assert_eq!(local_state.transform_counts, remote_state.transform_counts);
        assert_eq!(local_state.counters, remote_state.counters);
    }
}
