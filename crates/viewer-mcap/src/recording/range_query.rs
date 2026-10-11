//! Exact, bounded range-query contracts.

use crate::{DataWindowTimeRange, McapOpenError, RawMessage, StreamId};
use std::fmt;

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
    Source(McapOpenError),
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

impl From<McapOpenError> for RangeQueryError {
    fn from(value: McapOpenError) -> Self {
        Self::Source(value)
    }
}
