//! Exact, bounded range-query contracts and execution.

use crate::McapSourceError;

use std::fmt;
use viewer_core::{DataWindowTimeRange, RawMessage, StreamId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryLimits {
    pub max_messages: usize,
    pub max_payload_bytes: usize,
}

impl QueryLimits {
    pub fn new(max_messages: usize, max_payload_bytes: usize) -> Result<Self, RangeQueryError> {
        if max_messages == 0 || max_payload_bytes == 0 {
            return Err(RangeQueryError::ZeroLimits {
                max_messages,
                max_payload_bytes,
            });
        }
        Ok(Self {
            max_messages,
            max_payload_bytes,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RangeQuery {
    pub streams: Vec<StreamId>,
    pub range: DataWindowTimeRange,
    pub limits: QueryLimits,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RangeQueryResult {
    pub messages: Vec<RawMessage>,
    pub payload_bytes: usize,
    pub complete: bool,
}

#[derive(Debug)]
pub enum RangeQueryError {
    ZeroLimits {
        max_messages: usize,
        max_payload_bytes: usize,
    },
    EmptySelection,
    WorkLimit,
    InvalidCursor,
    UnknownStream(StreamId),
    PayloadBytesOverflow,
    InvalidRange(DataWindowTimeRange),
    Source(McapSourceError),
}

impl fmt::Display for RangeQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLimits { .. } => write!(formatter, "range query limits must be non-zero"),
            Self::WorkLimit => write!(
                formatter,
                "query read/scan budget exceeded; narrow the time range"
            ),
            Self::InvalidCursor => write!(formatter, "invalid query page cursor"),
            Self::EmptySelection => write!(formatter, "range query requires at least one stream"),
            Self::UnknownStream(stream) => write!(
                formatter,
                "range query references unknown stream {}",
                stream.0
            ),
            Self::PayloadBytesOverflow => write!(formatter, "range query payload size overflow"),
            Self::InvalidRange(range) => write!(
                formatter,
                "invalid query range {}..{}",
                range.start.0, range.end_exclusive.0
            ),
            Self::Source(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RangeQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            _ => None,
        }
    }
}

impl From<McapSourceError> for RangeQueryError {
    fn from(value: McapSourceError) -> Self {
        Self::Source(value)
    }
}

/// Continuation within an exact (log time, stream) tie, preserving duplicate messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RangePageCursor {
    log_time: viewer_core::ArrivalTime,
    stream: StreamId,
    ordinal: usize,
}

impl super::McapSource {
    /// Executes a bounded exact-message query without mutating this source's playback cursor.
    pub fn query_range(&self, query: &RangeQuery) -> Result<RangeQueryResult, RangeQueryError> {
        self.query_range_page_cancellable(query, None, usize::MAX, || false)
            .map(|v| v.0)
    }

    /// Conservative input + expansion work from index facts, without reading payloads.
    pub fn range_work_bytes(
        &self,
        streams: &[StreamId],
        range: DataWindowTimeRange,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<u64, RangeQueryError> {
        let mut total = 0u64;
        for (index, fact) in self.index_facts.iter().enumerate() {
            if cancelled() {
                return Err(McapSourceError::QueryCancelled.into());
            }
            if fact.end_inclusive < range.start
                || fact.start >= range.end_exclusive
                || !streams.iter().any(|s| fact.indexed_streams.contains(s))
            {
                continue;
            }
            let chunk = &self.summary.chunk_indexes[index];
            total = total
                .checked_add(chunk.compressed_size)
                .and_then(|n| n.checked_add(chunk.uncompressed_size))
                .ok_or(RangeQueryError::WorkLimit)?;
        }
        Ok(total)
    }

    /// Independent ordered traversal; cancellation also applies while skipping equal-time rows.
    pub fn query_range_page_cancellable(
        &self,
        query: &RangeQuery,
        after: Option<RangePageCursor>,
        max_scanned: usize,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<(RangeQueryResult, Option<RangePageCursor>), RangeQueryError> {
        if cancelled() {
            return Err(McapSourceError::QueryCancelled.into());
        }
        if query.range.start >= query.range.end_exclusive {
            return Err(RangeQueryError::InvalidRange(query.range));
        }
        if query.streams.is_empty() {
            return Err(RangeQueryError::EmptySelection);
        }
        if query.limits.max_messages == 0 || query.limits.max_payload_bytes == 0 {
            return Err(RangeQueryError::ZeroLimits {
                max_messages: query.limits.max_messages,
                max_payload_bytes: query.limits.max_payload_bytes,
            });
        }
        if let Some(unknown) = query
            .streams
            .iter()
            .find(|id| self.catalog.by_id(**id).is_none())
        {
            return Err(RangeQueryError::UnknownStream(*unknown));
        }

        let mut source = self.fork_reader();
        source.select_streams(query.streams.iter().copied())?;
        if after.is_some_and(|c| {
            !query.range.contains(c.log_time) || !query.streams.contains(&c.stream)
        }) {
            return Err(RangeQueryError::InvalidCursor);
        }
        if cancelled() {
            return Err(McapSourceError::QueryCancelled.into());
        }
        source.seek(after.map_or(query.range.start, |c| c.log_time))?;
        let mut state = source.forward.clone();
        let mut messages = Vec::new();
        let mut payload_bytes = 0_usize;
        let mut last_key = None;
        let mut ordinal = 0;
        let mut cursor = None;
        let mut scanned = 0;
        loop {
            if cancelled() {
                return Err(McapSourceError::QueryCancelled.into());
            }
            let Some(message) = source.next_forward(
                &mut state,
                viewer_core::ArrivalTime(query.range.end_exclusive.0 - 1),
            )?
            else {
                break;
            };
            scanned += 1;
            if scanned > max_scanned {
                return Err(RangeQueryError::WorkLimit);
            }
            let key = (message.arrival_time, message.stream_id);
            if last_key == Some(key) {
                ordinal += 1;
            } else {
                ordinal = 0;
                last_key = Some(key);
            }
            let position = RangePageCursor {
                log_time: key.0,
                stream: key.1,
                ordinal,
            };
            if after.is_some_and(|c| {
                (position.log_time, position.stream, position.ordinal)
                    <= (c.log_time, c.stream, c.ordinal)
            }) {
                continue;
            }
            let next_bytes = payload_bytes
                .checked_add(message.payload.len())
                .ok_or(RangeQueryError::PayloadBytesOverflow)?;
            if messages.len() == query.limits.max_messages
                || next_bytes > query.limits.max_payload_bytes
            {
                return Ok((
                    RangeQueryResult {
                        messages,
                        payload_bytes,
                        complete: false,
                    },
                    cursor,
                ));
            }
            cursor = Some(position);
            messages.push(message);
            payload_bytes = next_bytes;
        }
        Ok((
            RangeQueryResult {
                messages,
                payload_bytes,
                complete: true,
            },
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexed_work_estimate_is_checked_without_expanding_chunks() {
        let mut output = std::io::Cursor::new(Vec::new());
        let mut writer =
            mcap::Writer::with_options(&mut output, mcap::WriteOptions::new().compression(None))
                .unwrap();
        let id = writer
            .add_channel(0, "/a", "cdr", &std::collections::BTreeMap::new())
            .unwrap();
        writer
            .write_to_known_channel(
                &mcap::records::MessageHeader {
                    channel_id: id,
                    sequence: 0,
                    log_time: 100,
                    publish_time: 0,
                },
                &[0, 1, 0, 0],
            )
            .unwrap();
        writer.finish().unwrap();
        drop(writer);
        let mut source = super::super::McapSource::from_bytes(output.into_inner().into()).unwrap();
        let range =
            DataWindowTimeRange::new(viewer_core::ArrivalTime(100), viewer_core::ArrivalTime(101))
                .unwrap();
        let old = source.read_diagnostics();
        std::sync::Arc::make_mut(&mut source.summary).chunk_indexes[0].uncompressed_size =
            512 * 1024 * 1024;
        assert!(
            source
                .range_work_bytes(&[StreamId(u32::from(id))], range, || false)
                .unwrap()
                >= 512 * 1024 * 1024
        );
        assert_eq!(source.read_diagnostics(), old);
        assert!(matches!(
            source.query_range_page_cancellable(
                &RangeQuery {
                    streams: vec![StreamId(u32::from(id))],
                    range,
                    limits: QueryLimits::new(64, 1024).unwrap()
                },
                None,
                100,
                || true
            ),
            Err(RangeQueryError::Source(McapSourceError::QueryCancelled))
        ));
        assert_eq!(source.read_diagnostics(), old);
        std::sync::Arc::make_mut(&mut source.summary).chunk_indexes[0].uncompressed_size = u64::MAX;
        assert!(matches!(
            source.range_work_bytes(&[StreamId(u32::from(id))], range, || false),
            Err(RangeQueryError::WorkLimit)
        ));
    }
}
