use crate::{
    ArrivalTime, CameraId, DomainRuntime, DomainState, McapOpenError, McapSource, PipelineCounters,
    PlaybackClock, PlaybackCommand, PlaybackPerformance, StageTiming,
};
use std::{fmt, time::Duration};
use web_time::Instant;

const TF_SEEK_PREROLL_NS: i64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlaybackEffect {
    #[default]
    None,
    Seeked,
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
    clock: PlaybackClock,
    domain: DomainRuntime,
    source_read: StageTiming,
}

impl<B: AsRef<[u8]>> McapPlayback<B> {
    pub fn new(backing: B, camera_topic: &str) -> Result<Self, McapPlaybackError> {
        let source = McapSource::new(backing)?;
        let domain = DomainRuntime::from_catalog(source.catalog(), camera_topic)
            .map_err(|error| McapPlaybackError::Binding(error.to_string()))?;
        let (start, end) = source.time_range();
        Ok(Self {
            source,
            clock: PlaybackClock::new(start, end),
            domain,
            source_read: StageTiming::default(),
        })
    }

    pub fn tick(&mut self, elapsed: Duration) -> Result<(), McapOpenError> {
        let candidate = self.clock.cursor_after(elapsed);
        let read_started = Instant::now();
        let messages = self.source.read_until(candidate)?;
        self.source_read.record(read_started.elapsed());
        self.domain.process(elapsed, messages);
        self.clock.commit_cursor(candidate);
        Ok(())
    }

    fn seek(&mut self, cursor: ArrivalTime) -> Result<(), McapOpenError> {
        let target = ArrivalTime(cursor.0.clamp(self.clock.start().0, self.clock.end().0));
        let mut staging_source = McapSource::new(self.source.backing_bytes())?;
        let mut staging_domain = self.domain.staging_for_restore();
        let start = self.source.time_range().0;
        let pre_roll = ArrivalTime(target.0.saturating_sub(TF_SEEK_PREROLL_NS).max(start.0));
        staging_source.seek(pre_roll)?;
        let messages = staging_source.read_until(target)?;
        staging_domain.apply_transform_restore(messages);
        // Rewind after internal TF pre-roll so messages exactly at the cursor
        // remain part of normal playback.
        staging_source.seek(target)?;
        drop(staging_source);

        self.source.seek(target)?;
        self.domain.commit_restore(staging_domain);
        self.clock.seek(target);
        Ok(())
    }

    pub fn apply_command(
        &mut self,
        command: PlaybackCommand,
    ) -> Result<PlaybackEffect, McapOpenError> {
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
                self.seek(cursor)?;
                Ok(PlaybackEffect::Seeked)
            }
        }
    }

    pub fn clock(&self) -> &PlaybackClock {
        &self.clock
    }

    pub fn state(&self) -> &DomainState {
        self.domain.state()
    }

    pub fn counters(&self) -> PipelineCounters {
        self.domain.counters()
    }

    pub fn camera_topics(&self) -> &[(CameraId, String)] {
        self.domain.camera_topics()
    }

    pub fn focused_camera(&self) -> Option<CameraId> {
        self.domain.focused_camera()
    }

    pub fn set_focused_camera(&mut self, focused_camera: Option<CameraId>) {
        self.domain.set_focused_camera(focused_camera);
    }

    pub fn performance(&self) -> PlaybackPerformance {
        PlaybackPerformance::from_parts(self.source_read, self.domain.performance())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcap::Summary;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg")
                .join(name),
        )
        .unwrap()
    }

    fn corrupt_last_chunk(mut bytes: Vec<u8>) -> (Vec<u8>, ArrivalTime) {
        let summary = Summary::read(&bytes).unwrap().unwrap();
        assert!(summary.chunk_indexes.len() > 1);
        let last = summary.chunk_indexes.last().unwrap();
        let offset = usize::try_from(last.compressed_data_offset().unwrap()).unwrap();
        let length = usize::try_from(last.compressed_size).unwrap();
        let target = ArrivalTime(i64::try_from(last.message_end_time).unwrap());
        bytes[offset + length / 2] ^= 0x01;
        (bytes, target)
    }

    #[test]
    fn owns_common_tick_and_seek_state_transitions() {
        let bytes = fixture("camera_front_3s.mcap");
        let mut playback =
            McapPlayback::new(bytes.as_slice(), "/camera/front/image/compressed").unwrap();
        playback.apply_command(PlaybackCommand::Toggle).unwrap();
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
        assert_eq!(
            playback
                .apply_command(PlaybackCommand::Seek(midpoint))
                .unwrap(),
            PlaybackEffect::Seeked
        );
        assert_eq!(playback.clock().cursor(), midpoint);
        assert!(playback.state().camera.latest_by_arrival().is_none());
    }

    #[test]
    fn source_read_failure_does_not_commit_the_candidate_cursor() {
        let (bytes, _) = corrupt_last_chunk(fixture("camera_7_5s.mcap"));
        let mut playback =
            McapPlayback::new(bytes.as_slice(), "/camera/front/image/compressed").unwrap();
        playback.apply_command(PlaybackCommand::Toggle).unwrap();
        let committed = playback.clock().cursor();

        assert!(playback.tick(Duration::from_secs(10)).is_err());
        assert_eq!(playback.clock().cursor(), committed);
        assert!(playback.state().camera.latest_by_arrival().is_none());
    }

    #[test]
    fn failed_staging_seek_preserves_committed_clock_and_domain_state() {
        let (bytes, corrupt_target) = corrupt_last_chunk(fixture("camera_7_5s.mcap"));
        let mut playback =
            McapPlayback::new(bytes.as_slice(), "/camera/front/image/compressed").unwrap();
        playback.apply_command(PlaybackCommand::Toggle).unwrap();
        playback.tick(Duration::from_millis(20)).unwrap();
        let committed_cursor = playback.clock().cursor();
        let committed_camera = playback
            .state()
            .camera
            .latest_by_arrival()
            .map(|frame| frame.arrival_time);
        let committed_static = playback.state().transforms.static_len();
        let committed_dynamic = playback.state().transforms.dynamic_len();
        let committed_counters = playback.counters();

        assert!(
            playback
                .apply_command(PlaybackCommand::Seek(corrupt_target))
                .is_err()
        );
        assert_eq!(playback.clock().cursor(), committed_cursor);
        assert_eq!(
            playback
                .state()
                .camera
                .latest_by_arrival()
                .map(|frame| frame.arrival_time),
            committed_camera
        );
        assert_eq!(playback.state().transforms.static_len(), committed_static);
        assert_eq!(playback.state().transforms.dynamic_len(), committed_dynamic);
        assert_eq!(playback.counters(), committed_counters);
    }

    #[test]
    fn limits_seven_cameras_to_ten_and_five_hz() {
        let bytes = fixture("camera_7_5s.mcap");
        let mut playback =
            McapPlayback::new(bytes.as_slice(), "/camera/front/image/compressed").unwrap();
        playback.apply_command(PlaybackCommand::Toggle).unwrap();
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
                + playback.domain.pending_camera_count() as u64,
            performance.camera_input_frames
        );
    }

    #[test]
    fn raises_the_new_focus_to_ten_hz() {
        let bytes = fixture("camera_7_5s.mcap");
        let mut playback =
            McapPlayback::new(bytes.as_slice(), "/camera/front/image/compressed").unwrap();
        playback.apply_command(PlaybackCommand::Toggle).unwrap();
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
