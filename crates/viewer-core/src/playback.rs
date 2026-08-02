use crate::{
    ArrivalTime, CameraId, DomainState, DomainUpdate, McapOpenError, McapSource, PipelineCounters,
    PipelineSet, PlaybackClock, RawMessage, StreamBinding, StreamId, camera_topics,
    standard_bindings,
};
use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    time::Duration,
};
use web_time::Instant;

const TF_SEEK_PREROLL_NS: i64 = 1_000_000_000;

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
    fn record(&mut self, elapsed: Duration) {
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

#[derive(Debug)]
pub enum McapPlaybackError {
    Source(McapOpenError),
    Binding(String),
}

impl fmt::Display for McapPlaybackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(f),
            Self::Binding(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for McapPlaybackError {}

impl From<McapOpenError> for McapPlaybackError {
    fn from(value: McapOpenError) -> Self {
        Self::Source(value)
    }
}

pub struct McapPlayback<B: AsRef<[u8]>> {
    source: McapSource<B>,
    pipelines: PipelineSet,
    clock: PlaybackClock,
    state: DomainState,
    transform_streams: Vec<StreamId>,
    camera_streams: HashMap<StreamId, CameraId>,
    camera_topics: Vec<(CameraId, String)>,
    focused_camera: Option<CameraId>,
    presentation_elapsed: Duration,
    next_camera_presentation: BTreeMap<CameraId, Duration>,
    pending_camera_messages: BTreeMap<CameraId, RawMessage>,
    camera_dropped: u64,
    performance: PlaybackPerformance,
}

impl<B: AsRef<[u8]>> McapPlayback<B> {
    pub fn new(backing: B, camera_topic: &str) -> Result<Self, McapPlaybackError> {
        let source = McapSource::new(backing)?;
        let catalog_cameras =
            camera_topics(source.catalog(), camera_topic).map_err(McapPlaybackError::Binding)?;
        let bindings = standard_bindings(source.catalog(), camera_topic)
            .map_err(McapPlaybackError::Binding)?;
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
        let camera_topics: Vec<(CameraId, String)> = catalog_cameras
            .into_iter()
            .enumerate()
            .map(|(index, (_, topic))| (CameraId(index as u16), topic))
            .collect();
        let focused_camera = camera_topics.first().map(|(camera_id, _)| *camera_id);
        let pipelines = PipelineSet::new(&source.catalog().streams, &bindings);
        let (start, end) = source.time_range();
        let mut playback = Self {
            source,
            pipelines,
            clock: PlaybackClock::new(start, end),
            state: DomainState::default(),
            transform_streams,
            camera_streams,
            camera_topics,
            focused_camera,
            presentation_elapsed: Duration::ZERO,
            next_camera_presentation: BTreeMap::new(),
            pending_camera_messages: BTreeMap::new(),
            camera_dropped: 0,
            performance: PlaybackPerformance::default(),
        };
        playback.reset_camera_schedule();
        Ok(playback)
    }

    pub fn tick(&mut self, elapsed: Duration) -> Result<(), McapOpenError> {
        self.presentation_elapsed = self.presentation_elapsed.saturating_add(elapsed);
        let cursor = self.clock.advance(elapsed);
        let read_started = Instant::now();
        let messages = self.source.read_until(cursor)?;
        self.performance.source_read.record(read_started.elapsed());

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
        Ok(())
    }

    pub fn seek(&mut self, cursor: ArrivalTime) -> Result<(), McapOpenError> {
        self.clock.seek(cursor);
        self.state.cold_seek();
        self.presentation_elapsed = Duration::ZERO;
        self.reset_camera_schedule();
        self.pending_camera_messages.clear();

        let start = self.source.time_range().0;
        let pre_roll = ArrivalTime(cursor.0.saturating_sub(TF_SEEK_PREROLL_NS).max(start.0));
        self.source.seek(pre_roll)?;
        let mut updates = Vec::new();
        for message in self.source.read_until(cursor)? {
            if self.transform_streams.contains(&message.stream_id) {
                self.pipelines.decode(message, &mut updates);
            }
        }
        for update in updates {
            if matches!(update, DomainUpdate::Transforms(_)) {
                self.state.apply(update);
            }
        }
        // Rewind after internal TF pre-roll so messages exactly at the cursor
        // remain part of normal playback.
        self.source.seek(cursor)?;
        Ok(())
    }

    pub fn clock(&self) -> &PlaybackClock {
        &self.clock
    }

    pub fn clock_mut(&mut self) -> &mut PlaybackClock {
        &mut self.clock
    }

    pub fn state(&self) -> &DomainState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut DomainState {
        &mut self.state
    }

    pub fn counters(&self) -> PipelineCounters {
        let mut counters = self.pipelines.counters();
        counters.dropped = self.camera_dropped;
        counters
    }

    pub fn camera_topics(&self) -> &[(CameraId, String)] {
        &self.camera_topics
    }

    pub fn focused_camera(&self) -> Option<CameraId> {
        self.focused_camera
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

    pub fn performance(&self) -> &PlaybackPerformance {
        &self.performance
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

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg")
                .join(name),
        )
        .unwrap()
    }

    #[test]
    fn owns_common_tick_and_seek_state_transitions() {
        let bytes = fixture("camera_front_3s.mcap");
        let mut playback =
            McapPlayback::new(bytes.as_slice(), "/camera/front/image/compressed").unwrap();
        playback.clock_mut().play();
        playback.tick(Duration::from_secs(10)).unwrap();
        assert!(playback.state().camera.latest_by_arrival().is_some());
        assert_eq!(playback.counters().decoded, 1);
        assert_eq!(playback.counters().dropped, 29);
        assert_eq!(playback.performance().camera_input_frames, 30);
        assert_eq!(playback.performance().camera_presented_frames, 1);

        let midpoint = ArrivalTime(
            playback.clock().start().0
                + (playback.clock().end().0 - playback.clock().start().0) / 2,
        );
        playback.seek(midpoint).unwrap();
        assert_eq!(playback.clock().cursor(), midpoint);
        assert!(playback.state().camera.latest_by_arrival().is_none());
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
    fn limits_seven_cameras_to_ten_and_five_hz() {
        let bytes = fixture("camera_7_5s.mcap");
        let mut playback =
            McapPlayback::new(bytes.as_slice(), "/camera/front/image/compressed").unwrap();
        playback.clock_mut().play();
        let mut previous_presented = 0;
        for _ in 0..250 {
            playback.tick(Duration::from_millis(20)).unwrap();
            let presented = playback.performance().camera_presented_frames;
            assert!(
                presented - previous_presented <= 2,
                "camera work was not staggered"
            );
            previous_presented = presented;
        }

        let performance = playback.performance();
        let focused = performance
            .camera_presented_by_id
            .get(&CameraId(0))
            .copied()
            .unwrap_or_default();
        assert!((45..=51).contains(&focused), "focused frames: {focused}");
        for camera_id in 1..7 {
            let frames = performance
                .camera_presented_by_id
                .get(&CameraId(camera_id))
                .copied()
                .unwrap_or_default();
            assert!(
                (22..=26).contains(&frames),
                "camera {}: {frames} frames",
                camera_id
            );
        }
        assert_eq!(
            playback.counters().dropped
                + performance.camera_presented_frames
                + playback.pending_camera_messages.len() as u64,
            performance.camera_input_frames
        );
    }

    #[test]
    fn raises_the_new_focus_to_ten_hz() {
        let bytes = fixture("camera_7_5s.mcap");
        let mut playback =
            McapPlayback::new(bytes.as_slice(), "/camera/front/image/compressed").unwrap();
        playback.clock_mut().play();
        for _ in 0..50 {
            playback.tick(Duration::from_millis(20)).unwrap();
        }
        let before = playback
            .performance()
            .camera_presented_by_id
            .get(&CameraId(1))
            .copied()
            .unwrap_or_default();

        playback.set_focused_camera(Some(CameraId(1)));
        for _ in 0..50 {
            playback.tick(Duration::from_millis(20)).unwrap();
        }
        let after = playback
            .performance()
            .camera_presented_by_id
            .get(&CameraId(1))
            .copied()
            .unwrap_or_default();

        assert!((9..=11).contains(&(after - before)));
        assert_eq!(playback.focused_camera(), Some(CameraId(1)));
    }
}
