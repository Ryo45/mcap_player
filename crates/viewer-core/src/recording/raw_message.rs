//! Serialized canonical record routed from a source to continuous features.

use crate::{ArrivalTime, StreamId};
use bytes::Bytes;

/// Source-neutral serialized message supplied to session processing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawMessage {
    pub stream_id: StreamId,
    pub arrival_time: ArrivalTime,
    /// MCAP publish time; independent of log time and ROS header stamp. Live may omit it.
    pub publish_time_ns: Option<u64>,
    pub sequence: Option<u32>,
    pub payload: Bytes,
}
