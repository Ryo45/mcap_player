use crate::{
    ArrivalTime, McapOpenError, McapSource, PlaybackClock, RawMessage, SourceCatalog, StageTiming,
    StreamId,
};
use std::{fmt, time::Duration};
use web_time::Instant;

const TEMPORARY_RESTORE_PREROLL_NS: i64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlaybackEffect {
    #[default]
    None,
    Seeked,
}

#[derive(Debug)]
pub struct McapPlaybackError(McapOpenError);

impl fmt::Display for McapPlaybackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for McapPlaybackError {}

impl From<McapOpenError> for McapPlaybackError {
    fn from(value: McapOpenError) -> Self {
        Self(value)
    }
}

/// Native sequential MCAP transport.
///
/// This type owns physical source traversal and the playback clock, but no decoded semantic
/// state. The caller routes the returned exact messages to its concrete feature controllers.
pub struct McapPlayback<B: AsRef<[u8]>> {
    source: McapSource<B>,
    clock: PlaybackClock,
    source_read: StageTiming,
}

impl<B: AsRef<[u8]>> McapPlayback<B> {
    pub fn new(backing: B) -> Result<Self, McapPlaybackError> {
        let source = McapSource::new(backing)?;
        let (start, end) = source.time_range();
        Ok(Self {
            source,
            clock: PlaybackClock::new(start, end),
            source_read: StageTiming::default(),
        })
    }

    pub fn catalog(&self) -> &SourceCatalog {
        self.source.catalog()
    }

    pub fn select_streams(&mut self, streams: impl IntoIterator<Item = StreamId>) {
        self.source.select_streams(streams);
    }

    /// Reads the candidate interval, lets the caller process it, then commits the visible cursor.
    /// A source read failure leaves the cursor unchanged.
    pub fn tick(
        &mut self,
        elapsed: Duration,
        process: impl FnOnce(Duration, Vec<RawMessage>),
    ) -> Result<(), McapOpenError> {
        let candidate = self.clock.cursor_after(elapsed);
        let read_started = Instant::now();
        let messages = self.source.read_until(candidate)?;
        self.source_read.record(read_started.elapsed());
        process(elapsed, messages);
        self.clock.commit_cursor(candidate);
        Ok(())
    }

    /// Performs the existing bounded pre-roll read transactionally and commits only after the
    /// caller has synchronously rebuilt its controller state.
    ///
    /// The fixed range is replaced by the catalog-driven RestorePlanner later in this migration.
    pub fn seek_with(
        &mut self,
        cursor: ArrivalTime,
        restore: impl FnOnce(Vec<RawMessage>),
    ) -> Result<(), McapOpenError> {
        let target = ArrivalTime(cursor.0.clamp(self.clock.start().0, self.clock.end().0));
        let mut staging_source = McapSource::new(self.source.backing_bytes())?;
        staging_source.select_streams(self.source.catalog().streams.iter().map(|stream| stream.id));
        let start = self.source.time_range().0;
        let pre_roll = ArrivalTime(
            target
                .0
                .saturating_sub(TEMPORARY_RESTORE_PREROLL_NS)
                .max(start.0),
        );
        staging_source.seek(pre_roll)?;
        let messages = staging_source.read_until(target)?;

        self.source.seek(target)?;
        restore(messages);
        self.clock.seek(target);
        Ok(())
    }

    pub fn clock(&self) -> &PlaybackClock {
        &self.clock
    }

    pub fn clock_mut(&mut self) -> &mut PlaybackClock {
        &mut self.clock
    }

    pub fn source_read_timing(&self) -> StageTiming {
        self.source_read
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
    fn read_failure_does_not_commit_the_candidate_cursor() {
        let (bytes, _) = corrupt_last_chunk(fixture("camera_7_5s.mcap"));
        let mut playback = McapPlayback::new(bytes.as_slice()).unwrap();
        playback.clock_mut().toggle();
        let committed = playback.clock().cursor();
        let mut processed = false;

        assert!(
            playback
                .tick(Duration::from_secs(10), |_, _| processed = true)
                .is_err()
        );
        assert_eq!(playback.clock().cursor(), committed);
        assert!(!processed);
    }

    #[test]
    fn successful_processing_precedes_cursor_commit() {
        let bytes = fixture("camera_front_3s.mcap");
        let mut playback = McapPlayback::new(bytes.as_slice()).unwrap();
        playback.clock_mut().toggle();
        let committed = playback.clock().cursor();
        let requested = playback.clock().cursor_after(Duration::from_millis(100));
        playback
            .tick(Duration::from_millis(100), |_, messages| {
                assert!(!messages.is_empty());
            })
            .unwrap();
        assert_ne!(playback.clock().cursor(), committed);
        assert_eq!(playback.clock().cursor(), requested);
    }

    #[test]
    fn failed_restore_read_never_calls_restore_or_commits_cursor() {
        let (bytes, corrupt_target) = corrupt_last_chunk(fixture("camera_7_5s.mcap"));
        let mut playback = McapPlayback::new(bytes.as_slice()).unwrap();
        let committed = playback.clock().cursor();
        let mut restored = false;

        assert!(
            playback
                .seek_with(corrupt_target, |_| restored = true)
                .is_err()
        );
        assert_eq!(playback.clock().cursor(), committed);
        assert!(!restored);
    }
}
