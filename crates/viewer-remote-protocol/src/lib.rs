//! Platform-neutral wire contracts for the filesystem Recording Server.

use serde::{Deserialize, Serialize};

mod batch;
mod catalog;
mod error;

pub use batch::{BATCH_CONTENT_TYPE, BATCH_VERSION, BatchDecoder, BatchEncoder, RemoteMessageRef};
pub use catalog::{
    CatalogCapabilities, CatalogResponse, RecordingDescriptor, RecordingsResponse, RemoteTimeRange,
    StreamDescriptor, StreamSemantic, TimestampNs,
};
pub use error::ProtocolError;

pub const REMOTE_PROTOCOL_SCHEMA_VERSION: u32 = 1;
pub const RECORDING_REVISION_HEADER: &str = "x-av-recording-revision";
pub const BATCH_COMPLETE_HEADER: &str = "x-av-batch-complete";
pub const NEXT_CURSOR_HEADER: &str = "x-av-next-cursor";
pub const MESSAGE_COUNT_HEADER: &str = "x-av-message-count";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RemoteErrorResponse {
    pub code: String,
    pub message: String,
}
