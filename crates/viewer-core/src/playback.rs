use crate::{
    ArrivalTime, CameraId, DomainState, McapOpenError, McapSource, PipelineCounters, PlaybackClock,
    PlaybackCommand, PlaybackCore, PlaybackPerformance,
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
    core: PlaybackCore,
}

impl<B: AsRef<[u8]>> McapPlayback<B> {
    pub fn new(backing: B, camera_topic: &str) -> Result<Self, McapPlaybackError> {
        let source = McapSource::new(backing)?;
        let core = PlaybackCore::new(source.catalog(), camera_topic)
            .map_err(|error| McapPlaybackError::Binding(error.to_string()))?;
        let (start, end) = source.time_range();
        Ok(Self {
            source,
            clock: PlaybackClock::new(start, end),
            core,
        })
    }

    pub fn tick(&mut self, elapsed: Duration) -> Result<(), McapOpenError> {
        let candidate = self.clock.cursor_after(elapsed);
        let read_started = Instant::now();
        let messages = self.source.read_until(candidate)?;
        self.core
            .performance_mut()
            .source_read
            .record(read_started.elapsed());
        self.core.process_forward(elapsed, messages);
        self.clock.commit_cursor(candidate);
        Ok(())
    }

    fn seek(&mut self, cursor: ArrivalTime) -> Result<(), McapOpenError> {
        self.clock.seek(cursor);
        self.core.reset_for_restore();

        let start = self.source.time_range().0;
        let pre_roll = ArrivalTime(cursor.0.saturating_sub(TF_SEEK_PREROLL_NS).max(start.0));
        self.source.seek(pre_roll)?;
        let messages = self.source.read_until(cursor)?;
        self.core.apply_transform_restore(messages);
        // Rewind after internal TF pre-roll so messages exactly at the cursor
        // remain part of normal playback.
        self.source.seek(cursor)?;
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
        self.core.state()
    }

    pub fn counters(&self) -> PipelineCounters {
        self.core.counters()
    }

    pub fn camera_topics(&self) -> &[(CameraId, String)] {
        self.core.camera_topics()
    }

    pub fn focused_camera(&self) -> Option<CameraId> {
        self.core.focused_camera()
    }

    pub fn set_focused_camera(&mut self, focused_camera: Option<CameraId>) {
        self.core.set_focused_camera(focused_camera);
    }

    pub fn performance(&self) -> &PlaybackPerformance {
        self.core.performance()
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
                + playback.core.pending_camera_count() as u64,
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
