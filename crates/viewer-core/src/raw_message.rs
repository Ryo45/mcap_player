use crate::{ArrivalTime, StreamId};
use bytes::Bytes;

/// Source-neutral serialized message supplied to session processing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawMessage {
    pub stream_id: StreamId,
    pub arrival_time: ArrivalTime,
    pub payload: Bytes,
}
