use crate::{
    McapOpenError, McapSource, PlaybackClock, RangeQuery, RangeQueryError, RangeQueryResult,
    RawMessage, RestorePlan, SourceCatalog, StageTiming, StreamId,
};
use std::{fmt, time::Duration};
use web_time::Instant;

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

#[derive(Debug)]
pub enum McapSeekError<E> {
    Source(McapOpenError),
    Restore(E),
}

impl<E> From<McapOpenError> for McapSeekError<E> {
    fn from(error: McapOpenError) -> Self {
        Self::Source(error)
    }
}

impl<E: fmt::Display> fmt::Display for McapSeekError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(formatter),
            Self::Restore(error) => error.fmt(formatter),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for McapSeekError<E> {}

/// Native sequential MCAP transport.
///
/// This type owns physical source traversal and the playback clock, but no decoded semantic
/// state. The caller routes the returned exact messages to its concrete feature controllers.
pub struct McapPlayback {
    source: McapSource,
    clock: PlaybackClock,
    source_read: StageTiming,
    persistent_archive: Vec<RawMessage>,
}

impl McapPlayback {
    pub fn new(backing: impl AsRef<[u8]>) -> Result<Self, McapPlaybackError> {
        let source = McapSource::new(backing)?;
        Self::from_source(source)
    }

    pub fn from_owner<B>(backing: B) -> Result<Self, McapPlaybackError>
    where
        B: AsRef<[u8]> + Send + 'static,
    {
        let source = McapSource::from_owner(backing)?;
        Self::from_source(source)
    }

    fn from_source(source: McapSource) -> Result<Self, McapPlaybackError> {
        let (start, end) = source.time_range();
        Ok(Self {
            source,
            clock: PlaybackClock::new(start, end),
            source_read: StageTiming::default(),
            persistent_archive: Vec::new(),
        })
    }

    pub fn catalog(&self) -> &SourceCatalog {
        self.source.catalog()
    }

    pub fn select_streams(
        &mut self,
        streams: impl IntoIterator<Item = StreamId>,
    ) -> Result<(), McapOpenError> {
        self.source.select_streams(streams)
    }

    pub fn query_range(&self, query: &RangeQuery) -> Result<RangeQueryResult, RangeQueryError> {
        self.source.query_range(query)
    }

    /// Loads explicitly persistent inputs once. The archive is small feature data, not a generic
    /// temporal store, and is replayed transactionally during later seeks.
    pub fn bootstrap_persistent(&mut self, streams: &[StreamId]) -> Result<(), McapOpenError> {
        self.persistent_archive = if streams.is_empty() {
            Vec::new()
        } else {
            self.source.indexed_streams(streams)?.messages
        };
        Ok(())
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

    /// Executes a catalog- and feature-derived restore plan transactionally.
    ///
    /// Every physical read and the forward-source reposition complete before the caller replaces
    /// visible controller state. The exact target message is part of restoration and is skipped by
    /// subsequent forward delivery.
    pub fn seek_with<E>(
        &mut self,
        plan: &RestorePlan,
        restore: impl FnOnce(crate::ArrivalTime, Vec<RawMessage>) -> Result<(), E>,
    ) -> Result<(), McapSeekError<E>> {
        let mut messages = self
            .source
            .latest_before(&plan.latest_before, plan.target)?
            .messages;
        for read in &plan.histories {
            messages.extend(
                self.source
                    .indexed_range(&read.streams, read.range)?
                    .messages,
            );
        }
        let persistent = plan
            .persistent
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        messages.extend(
            self.persistent_archive
                .iter()
                .filter(|message| {
                    persistent.contains(&message.stream_id) && message.arrival_time <= plan.target
                })
                .cloned(),
        );
        messages.sort_by_key(|message| (message.arrival_time, message.stream_id.0));

        let after_target = crate::ArrivalTime(plan.target.0.saturating_add(1));
        let source_position = self.source.prepare_seek(after_target)?;
        restore(plan.target, messages).map_err(McapSeekError::Restore)?;
        self.source.commit_seek(source_position);
        self.clock.seek(plan.target);
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
    use crate::ArrivalTime;
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
        let restore_plan = RestorePlan {
            target: corrupt_target,
            latest_before: Vec::new(),
            histories: vec![crate::RestoreRead {
                streams: playback
                    .catalog()
                    .streams
                    .iter()
                    .map(|stream| stream.id)
                    .collect(),
                range: crate::DataWindowTimeRange::new(
                    playback.clock().start(),
                    crate::ArrivalTime(corrupt_target.0 + 1),
                )
                .unwrap(),
            }],
            persistent: Vec::new(),
        };

        assert!(
            playback
                .seek_with(&restore_plan, |_, _| {
                    restored = true;
                    Ok::<(), std::convert::Infallible>(())
                })
                .is_err()
        );
        assert_eq!(playback.clock().cursor(), committed);
        assert!(!restored);
    }

    #[test]
    fn restore_application_failure_preserves_source_position_and_cursor() {
        let bytes = fixture("camera_front_3s.mcap");
        let mut playback = McapPlayback::new(bytes).unwrap();
        let committed = playback.clock().cursor();
        let target = ArrivalTime((playback.clock().start().0 + playback.clock().end().0) / 2);
        let plan = RestorePlan {
            target,
            latest_before: Vec::new(),
            histories: Vec::new(),
            persistent: Vec::new(),
        };

        let result = playback.seek_with(&plan, |_, _| {
            Err(std::io::Error::other("candidate decode failed"))
        });
        assert!(matches!(result, Err(McapSeekError::Restore(_))));
        assert_eq!(playback.clock().cursor(), committed);

        let mut replayed = Vec::new();
        playback
            .tick(Duration::ZERO, |_, messages| replayed = messages)
            .unwrap();
        assert!(
            replayed
                .iter()
                .all(|message| message.arrival_time <= committed)
        );
    }
}
