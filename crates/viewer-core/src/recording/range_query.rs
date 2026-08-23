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
            return Err(RangeQueryError::Invalid(
                "range query limits must be non-zero".into(),
            ));
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
    Invalid(String),
    Source(McapOpenError),
}

impl fmt::Display for RangeQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Source(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RangeQueryError {}

impl From<McapOpenError> for RangeQueryError {
    fn from(value: McapOpenError) -> Self {
        Self::Source(value)
    }
}
