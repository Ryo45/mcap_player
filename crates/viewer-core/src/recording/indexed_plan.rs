//! Platform-neutral Message Index planning.

use crate::{ArrivalTime, DataWindowTimeRange, StreamId};
use std::{collections::BTreeSet, error::Error, fmt};

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
