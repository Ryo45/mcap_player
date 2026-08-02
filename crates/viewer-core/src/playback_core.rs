use crate::{
    CameraId, DomainState, DomainUpdate, PipelineCounters, PipelineSet, RawMessage, StreamBinding,
    StreamCatalog, StreamId, camera_topics, standard_bindings,
};
use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    time::Duration,
};
use web_time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CameraPresentationPolicy {
    focused_interval: Duration,
    background_interval: Duration,
}

impl CameraPresentationPolicy {
    fn focused_hz(self) -> f64 {
        interval_hz(self.focused_interval)
    }

    fn background_hz(self) -> f64 {
        interval_hz(self.background_interval)
    }

    fn interval_for(self, focused: bool) -> Duration {
        if focused {
            self.focused_interval
        } else {
            self.background_interval
        }
    }
}

const CAMERA_PRESENTATION_POLICY: CameraPresentationPolicy = CameraPresentationPolicy {
    focused_interval: Duration::from_millis(100),
    background_interval: Duration::from_millis(200),
};

fn interval_hz(interval: Duration) -> f64 {
    1.0 / interval.as_secs_f64()
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StageTiming {
    pub last_ms: f64,
    pub average_ms: f64,
    pub max_ms: f64,
    samples: u64,
}

impl StageTiming {
    pub(crate) fn record(&mut self, elapsed: Duration) {
        let milliseconds = elapsed.as_secs_f64() * 1_000.0;
        self.last_ms = milliseconds;
        self.max_ms = self.max_ms.max(milliseconds);
        self.samples = self.samples.saturating_add(1);
        self.average_ms += (milliseconds - self.average_ms) / self.samples as f64;
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaybackPerformance {
    pub source_read: StageTiming,
    pub pipeline_decode: StageTiming,
    pub state_apply: StageTiming,
    pub camera_input_frames: u64,
    pub camera_presented_frames: u64,
    pub camera_presented_by_id: BTreeMap<CameraId, u64>,
}

impl PlaybackPerformance {
    pub fn focused_camera_hz(&self) -> f64 {
        CAMERA_PRESENTATION_POLICY.focused_hz()
    }

    pub fn background_camera_hz(&self) -> f64 {
        CAMERA_PRESENTATION_POLICY.background_hz()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaybackCoreError(String);

impl fmt::Display for PlaybackCoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PlaybackCoreError {}

/// Source-independent playback reduction, camera scheduling, and domain state.
pub struct PlaybackCore {
    pipelines: PipelineSet,
    state: DomainState,
    transform_streams: Vec<StreamId>,
    camera_streams: HashMap<StreamId, CameraId>,
    camera_topics: Vec<(CameraId, String)>,
    primary_camera_topic: String,
    focused_camera: Option<CameraId>,
    presentation_elapsed: Duration,
    next_camera_presentation: BTreeMap<CameraId, Duration>,
    pending_camera_messages: BTreeMap<CameraId, RawMessage>,
    camera_dropped: u64,
    performance: PlaybackPerformance,
}

impl PlaybackCore {
    pub fn new(
        catalog: &StreamCatalog,
        primary_camera_topic: &str,
    ) -> Result<Self, PlaybackCoreError> {
        let catalog_cameras =
            camera_topics(catalog, primary_camera_topic).map_err(PlaybackCoreError)?;
        let bindings =
            standard_bindings(catalog, primary_camera_topic).map_err(PlaybackCoreError)?;
        let transform_streams = bindings
            .iter()
            .filter_map(|(id, binding)| {
                matches!(binding, StreamBinding::Transforms { .. }).then_some(*id)
            })
            .collect();
        let camera_streams = bindings
            .iter()
            .filter_map(|(id, binding)| match binding {
                StreamBinding::Camera(camera_id) => Some((*id, *camera_id)),
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        let camera_topics: Vec<_> = catalog_cameras
            .into_iter()
            .enumerate()
            .map(|(index, (_, topic))| (CameraId(index as u16), topic))
            .collect();
        let focused_camera = camera_topics.first().map(|(camera_id, _)| *camera_id);
        let pipelines = PipelineSet::new(&catalog.streams, &bindings);
        let mut core = Self {
            pipelines,
            state: DomainState::default(),
            transform_streams,
            camera_streams,
            camera_topics,
            primary_camera_topic: primary_camera_topic.to_owned(),
            focused_camera,
            presentation_elapsed: Duration::ZERO,
            next_camera_presentation: BTreeMap::new(),
            pending_camera_messages: BTreeMap::new(),
            camera_dropped: 0,
            performance: PlaybackPerformance::default(),
        };
        core.reset_camera_schedule();
        Ok(core)
    }

    pub fn process_forward(
        &mut self,
        elapsed: Duration,
        messages: impl IntoIterator<Item = RawMessage>,
    ) {
        self.presentation_elapsed = self.presentation_elapsed.saturating_add(elapsed);
        let mut updates = Vec::new();
        let decode_started = Instant::now();
        for message in messages {
            if let Some(camera_id) = self.camera_streams.get(&message.stream_id).copied() {
                self.performance.camera_input_frames =
                    self.performance.camera_input_frames.saturating_add(1);
                if self
                    .pending_camera_messages
                    .insert(camera_id, message)
                    .is_some()
                {
                    self.camera_dropped = self.camera_dropped.saturating_add(1);
                }
            } else {
                self.pipelines.decode(message, &mut updates);
            }
        }

        let due_cameras = self
            .pending_camera_messages
            .keys()
            .copied()
            .filter(|camera_id| self.camera_is_due(*camera_id))
            .collect::<Vec<_>>();
        for camera_id in due_cameras {
            let message = self
                .pending_camera_messages
                .remove(&camera_id)
                .expect("due camera came from pending messages");
            self.pipelines.decode(message, &mut updates);
            self.next_camera_presentation.insert(
                camera_id,
                self.presentation_elapsed
                    .saturating_add(self.camera_interval(camera_id)),
            );
            self.performance.camera_presented_frames =
                self.performance.camera_presented_frames.saturating_add(1);
            *self
                .performance
                .camera_presented_by_id
                .entry(camera_id)
                .or_default() += 1;
        }
        self.performance
            .pipeline_decode
            .record(decode_started.elapsed());

        let apply_started = Instant::now();
        self.state.apply_all(updates);
        self.performance.state_apply.record(apply_started.elapsed());
    }

    pub fn reset_for_restore(&mut self) {
        self.state.cold_seek();
        self.presentation_elapsed = Duration::ZERO;
        self.reset_camera_schedule();
        self.pending_camera_messages.clear();
    }

    pub fn apply_transform_restore(&mut self, messages: impl IntoIterator<Item = RawMessage>) {
        let mut updates = Vec::new();
        for message in messages {
            if self.transform_streams.contains(&message.stream_id) {
                self.pipelines.decode(message, &mut updates);
            }
        }
        for update in updates {
            if matches!(update, DomainUpdate::Transforms(_)) {
                self.state.apply(update);
            }
        }
    }

    pub fn set_focused_camera(&mut self, focused_camera: Option<CameraId>) {
        let focused_camera = focused_camera
            .filter(|camera_id| self.camera_topics.iter().any(|(id, _)| id == camera_id))
            .or_else(|| self.camera_topics.first().map(|(camera_id, _)| *camera_id));
        if self.focused_camera != focused_camera {
            if let Some(previous) = self.focused_camera {
                self.next_camera_presentation.insert(
                    previous,
                    self.presentation_elapsed
                        .saturating_add(CAMERA_PRESENTATION_POLICY.background_interval),
                );
            }
            self.focused_camera = focused_camera;
            if let Some(camera_id) = focused_camera {
                self.next_camera_presentation
                    .insert(camera_id, self.presentation_elapsed);
            }
        }
    }

    pub fn state(&self) -> &DomainState {
        &self.state
    }

    pub fn camera_topics(&self) -> &[(CameraId, String)] {
        &self.camera_topics
    }

    pub fn focused_camera(&self) -> Option<CameraId> {
        self.focused_camera
    }

    pub fn counters(&self) -> PipelineCounters {
        let mut counters = self.pipelines.counters();
        counters.dropped = self.camera_dropped;
        counters
    }

    pub fn performance(&self) -> &PlaybackPerformance {
        &self.performance
    }

    pub(crate) fn performance_mut(&mut self) -> &mut PlaybackPerformance {
        &mut self.performance
    }

    pub(crate) fn staging_for_restore(&self, catalog: &StreamCatalog) -> Self {
        let mut staged = Self::new(catalog, &self.primary_camera_topic)
            .expect("an already validated catalog remains valid while seeking");
        staged.state = self.state.clone();
        staged.focused_camera = self.focused_camera;
        staged.reset_for_restore();
        staged
    }

    pub(crate) fn commit_restore(&mut self, staged: Self) {
        self.pipelines.add_counters(staged.pipelines.counters());
        self.state = staged.state;
        self.presentation_elapsed = Duration::ZERO;
        self.pending_camera_messages.clear();
        self.reset_camera_schedule();
    }

    #[cfg(test)]
    pub(crate) fn pending_camera_count(&self) -> usize {
        self.pending_camera_messages.len()
    }

    fn camera_is_due(&self, camera_id: CameraId) -> bool {
        self.next_camera_presentation
            .get(&camera_id)
            .is_none_or(|deadline| self.presentation_elapsed >= *deadline)
    }

    fn camera_interval(&self, camera_id: CameraId) -> Duration {
        CAMERA_PRESENTATION_POLICY.interval_for(Some(camera_id) == self.focused_camera)
    }

    fn reset_camera_schedule(&mut self) {
        self.next_camera_presentation.clear();
        let background = self
            .camera_topics
            .iter()
            .map(|(camera_id, _)| *camera_id)
            .filter(|camera_id| Some(*camera_id) != self.focused_camera)
            .collect::<Vec<_>>();
        let background_count = background.len().max(1) as f64;
        for (index, camera_id) in background.into_iter().enumerate() {
            let phase = Duration::from_secs_f64(
                CAMERA_PRESENTATION_POLICY.background_interval.as_secs_f64() * index as f64
                    / background_count,
            );
            self.next_camera_presentation.insert(camera_id, phase);
        }
        if let Some(camera_id) = self.focused_camera {
            self.next_camera_presentation
                .insert(camera_id, Duration::ZERO);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{McapPlayback, McapSource, PlaybackCommand};

    fn camera_fixture() -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg/camera_front_3s.mcap"),
        )
        .unwrap()
    }

    #[test]
    fn camera_rates_are_derived_from_the_scheduling_intervals() {
        assert_eq!(CAMERA_PRESENTATION_POLICY.focused_hz(), 10.0);
        assert_eq!(CAMERA_PRESENTATION_POLICY.background_hz(), 5.0);
        assert_eq!(
            PlaybackPerformance::default().focused_camera_hz(),
            CAMERA_PRESENTATION_POLICY.focused_hz()
        );
        assert_eq!(
            PlaybackPerformance::default().background_camera_hz(),
            CAMERA_PRESENTATION_POLICY.background_hz()
        );
    }

    #[test]
    fn direct_message_reduction_matches_local_playback() {
        let bytes = camera_fixture();
        let mut source = McapSource::new(bytes.as_slice()).unwrap();
        let mut core =
            PlaybackCore::new(source.catalog(), "/camera/front/image/compressed").unwrap();
        let messages = source.read_until(source.time_range().1).unwrap();
        core.process_forward(Duration::from_secs(10), messages);

        let mut playback =
            McapPlayback::new(bytes.as_slice(), "/camera/front/image/compressed").unwrap();
        playback.apply_command(PlaybackCommand::Toggle).unwrap();
        playback.tick(Duration::from_secs(10)).unwrap();

        assert_eq!(core.counters(), playback.counters());
        assert_eq!(
            core.state()
                .camera
                .latest_by_arrival()
                .map(|frame| frame.arrival_time),
            playback
                .state()
                .camera
                .latest_by_arrival()
                .map(|frame| frame.arrival_time)
        );
        assert_eq!(
            core.performance().camera_input_frames,
            playback.performance().camera_input_frames
        );
        assert_eq!(
            core.performance().camera_presented_frames,
            playback.performance().camera_presented_frames
        );
    }
}
