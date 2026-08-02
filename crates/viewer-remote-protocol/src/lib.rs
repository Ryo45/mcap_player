//! Platform-neutral wire contracts for the filesystem Recording Server.

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
