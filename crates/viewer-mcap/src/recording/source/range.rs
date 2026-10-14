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
    UnknownStream(StreamId),
    PayloadBytesOverflow,
    InvalidRange(DataWindowTimeRange),
    Source(McapSourceError),
}

impl fmt::Display for RangeQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLimits { .. } => write!(formatter, "range query limits must be non-zero"),
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

impl super::McapSource {
    /// Executes a bounded exact-message query without mutating this source's playback cursor.
    pub fn query_range(&self, query: &RangeQuery) -> Result<RangeQueryResult, RangeQueryError> {
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
        source.seek(query.range.start)?;
        let mut state = source.forward.clone();
        let mut messages = Vec::new();
        let mut payload_bytes = 0_usize;
        while let Some(message) = source.next_forward(
            &mut state,
            viewer_core::ArrivalTime(query.range.end_exclusive.0 - 1),
        )? {
            let next_bytes = payload_bytes
                .checked_add(message.payload.len())
                .ok_or(RangeQueryError::PayloadBytesOverflow)?;
            if messages.len() == query.limits.max_messages
                || next_bytes > query.limits.max_payload_bytes
            {
                return Ok(RangeQueryResult {
                    messages,
                    payload_bytes,
                    complete: false,
                });
            }
            messages.push(message);
            payload_bytes = next_bytes;
        }
        Ok(RangeQueryResult {
            messages,
            payload_bytes,
            complete: true,
        })
    }
}
