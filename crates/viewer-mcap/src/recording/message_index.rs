//! Platform-neutral Message Index planning.

use std::{collections::BTreeSet, error::Error, fmt};
use viewer_core::{ArrivalTime, DataWindowTimeRange, StreamId};

/// Exact latest-before selection across chunk Message Index records.
/// Traversal, physical reads and caching remain with the source adapter.
pub struct MessageIndexPredecessor {
    target: u64,
    best: Option<(u64, crate::IndexedMessagePosition)>,
}

impl MessageIndexPredecessor {
    pub fn new(target: u64) -> Self {
        Self { target, best: None }
    }

    /// Entries must be ordered by (log time, record offset). Native parsed spans
    /// already enforce this invariant, so predecessor lookup stays logarithmic.
    pub fn consider_sorted(&mut self, chunk: usize, entries: &[mcap::records::MessageIndexEntry]) {
        let end = entries.partition_point(|entry| entry.log_time <= self.target);
        if let Some(entry) = end.checked_sub(1).and_then(|index| entries.get(index)) {
            self.consider(chunk, entry);
        }
    }

    /// Accept arbitrary entry order without sorting or retaining a copy, for
    /// adapters that consume an index once.
    pub fn consider_unsorted(
        &mut self,
        chunk: usize,
        entries: &[mcap::records::MessageIndexEntry],
    ) {
        for entry in entries {
            self.consider(chunk, entry);
        }
    }

    fn consider(&mut self, chunk: usize, entry: &mcap::records::MessageIndexEntry) {
        if entry.log_time <= self.target
            && self.best.is_none_or(|(time, position)| {
                (entry.log_time, chunk, entry.offset)
                    > (time, position.chunk, position.record_offset)
            })
        {
            self.best = Some((
                entry.log_time,
                crate::IndexedMessagePosition {
                    chunk,
                    record_offset: entry.offset,
                },
            ));
        }
    }

    /// Whether chunks bounded by these maxima can still improve the result.
    /// An overlapping earlier chunk may contain a later message; chunk traversal
    /// order alone is insufficient. Equal times prefer chunk ID, then record offset.
    pub fn can_improve(&self, max_end_time: u64, max_chunk: usize) -> bool {
        self.best.is_none_or(|(time, position)| {
            max_end_time > time || (max_end_time == time && max_chunk >= position.chunk)
        })
    }

    /// Latest log time and source-local record position, if a predecessor exists.
    pub fn result(&self) -> Option<(u64, crate::IndexedMessagePosition)> {
        self.best
    }
}

/// Source-neutral facts needed to decide which physical chunks may satisfy an indexed read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedChunkFact {
    pub start: ArrivalTime,
    pub end_inclusive: ArrivalTime,
    pub indexed_streams: BTreeSet<StreamId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndexedPlanError {
    pub stream: StreamId,
}

impl fmt::Display for IndexedPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Message Index is required for non-empty stream {}",
            self.stream.0
        )
    }
}

impl Error for IndexedPlanError {}

pub fn ensure_indexed(
    facts: &[IndexedChunkFact],
    stream: StreamId,
    message_count: Option<u64>,
) -> Result<(), IndexedPlanError> {
    if message_count == Some(0)
        || facts
            .iter()
            .any(|chunk| chunk.indexed_streams.contains(&stream))
    {
        Ok(())
    } else {
        Err(IndexedPlanError { stream })
    }
}

/// Candidate chunks for a latest-before lookup, newest first.
///
/// Message Index entries inside these chunks still decide the exact record. This function owns
/// the shared time/index filtering policy only; adapters retain their concrete range-read APIs.
pub fn latest_candidate_chunks(
    facts: &[IndexedChunkFact],
    stream: StreamId,
    message_count: Option<u64>,
    target: ArrivalTime,
) -> Result<Vec<usize>, IndexedPlanError> {
    ensure_indexed(facts, stream, message_count)?;
    Ok(facts
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, chunk)| chunk.start <= target && chunk.indexed_streams.contains(&stream))
        .map(|(index, _)| index)
        .collect())
}

pub fn history_candidate_chunks(
    facts: &[IndexedChunkFact],
    stream: StreamId,
    message_count: Option<u64>,
    range: DataWindowTimeRange,
) -> Result<Vec<usize>, IndexedPlanError> {
    ensure_indexed(facts, stream, message_count)?;
    Ok(facts
        .iter()
        .enumerate()
        .filter(|(_, chunk)| {
            chunk.indexed_streams.contains(&stream)
                && chunk.end_inclusive >= range.start
                && chunk.start < range.end_exclusive
        })
        .map(|(index, _)| index)
        .collect())
}

pub fn persistent_candidate_chunks(
    facts: &[IndexedChunkFact],
    stream: StreamId,
    message_count: Option<u64>,
) -> Result<Vec<usize>, IndexedPlanError> {
    ensure_indexed(facts, stream, message_count)?;
    Ok(facts
        .iter()
        .enumerate()
        .filter(|(_, chunk)| chunk.indexed_streams.contains(&stream))
        .map(|(index, _)| index)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_and_unsorted_lookup_match_the_same_flat_reference_for_overlapping_chunks() {
        let chunks = [
            vec![(100, 10), (90, 9), (1, 1), (90, 7)],
            vec![(55, 1)],
            vec![(65, 1)],
            vec![(90, 1), (0, 0)],
        ];
        for target in [0, 39, 90, 95, 120, u64::MAX] {
            let expected = chunks
                .iter()
                .enumerate()
                .flat_map(|(chunk, entries)| {
                    entries
                        .iter()
                        .map(move |(time, offset)| (*time, chunk, *offset))
                })
                .filter(|(time, _, _)| *time <= target)
                .max();
            let mut sorted = MessageIndexPredecessor::new(target);
            let mut unsorted = MessageIndexPredecessor::new(target);
            for (chunk, entries) in chunks.iter().enumerate().rev() {
                let mut entries = entries
                    .iter()
                    .map(|(log_time, offset)| mcap::records::MessageIndexEntry {
                        log_time: *log_time,
                        offset: *offset,
                    })
                    .collect::<Vec<_>>();
                unsorted.consider_unsorted(chunk, &entries);
                entries.sort_by_key(|entry| (entry.log_time, entry.offset));
                sorted.consider_sorted(chunk, &entries);
            }
            for search in [&sorted, &unsorted] {
                assert_eq!(
                    search.result().map(|(time, position)| (
                        time,
                        position.chunk,
                        position.record_offset
                    )),
                    expected
                );
            }
        }
        assert!(MessageIndexPredecessor::new(0).result().is_none());
    }

    #[test]
    fn pruning_preserves_later_times_in_earlier_chunks_and_equal_time_chunk_ties() {
        let mut search = MessageIndexPredecessor::new(95);
        search.consider_unsorted(
            2,
            &[mcap::records::MessageIndexEntry {
                log_time: 65,
                offset: 1,
            }],
        );
        assert!(search.can_improve(100, 0));
        assert!(!search.can_improve(64, usize::MAX));
        search.consider_unsorted(
            0,
            &[mcap::records::MessageIndexEntry {
                log_time: 90,
                offset: 7,
            }],
        );
        assert_eq!(search.result().unwrap().0, 90);
        assert!(search.can_improve(90, 3));
        search.consider_sorted(
            3,
            &[mcap::records::MessageIndexEntry {
                log_time: 90,
                offset: 1,
            }],
        );
        assert!(!search.can_improve(90, 2));
        assert!(search.can_improve(90, 3));
    }

    fn facts() -> Vec<IndexedChunkFact> {
        vec![
            IndexedChunkFact {
                start: ArrivalTime(0),
                end_inclusive: ArrivalTime(9),
                indexed_streams: BTreeSet::from([StreamId(1), StreamId(2)]),
            },
            IndexedChunkFact {
                start: ArrivalTime(10),
                end_inclusive: ArrivalTime(19),
                indexed_streams: BTreeSet::from([StreamId(1)]),
            },
            IndexedChunkFact {
                start: ArrivalTime(20),
                end_inclusive: ArrivalTime(29),
                indexed_streams: BTreeSet::from([StreamId(2)]),
            },
        ]
    }

    #[test]
    fn latest_history_and_persistent_share_one_chunk_selection_policy() {
        assert_eq!(
            latest_candidate_chunks(&facts(), StreamId(1), Some(2), ArrivalTime(25)).unwrap(),
            vec![1, 0]
        );
        assert_eq!(
            history_candidate_chunks(
                &facts(),
                StreamId(2),
                Some(2),
                DataWindowTimeRange::new(ArrivalTime(5), ArrivalTime(22)).unwrap()
            )
            .unwrap(),
            vec![0, 2]
        );
        assert_eq!(
            persistent_candidate_chunks(&facts(), StreamId(2), Some(2)).unwrap(),
            vec![0, 2]
        );
    }

    #[test]
    fn nonempty_unindexed_stream_is_rejected_but_known_empty_stream_is_valid() {
        assert!(ensure_indexed(&facts(), StreamId(3), Some(1)).is_err());
        assert!(ensure_indexed(&facts(), StreamId(3), Some(0)).is_ok());
    }
}
