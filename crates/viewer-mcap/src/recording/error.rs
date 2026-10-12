//! Failures while opening, reading or querying an MCAP source.
use std::fmt;

#[derive(Debug)]
pub enum McapSourceError {
    QueryLimit {
        resource: &'static str,
        maximum: usize,
    },
    Mcap(mcap::McapError),
    SummaryRequired,
    ChunkIndexRequired,
    MessageIndexRequired(crate::IndexedPlanError),
    Decompression {
        format: &'static str,
        source: std::io::Error,
    },
    InvalidChunk(String),
    UnsupportedCompression(String),
    TimestampOverflow,
    QueryCancelled,
    Io(std::io::Error),
    SourceChanged,
    NotRegularFile,
    ReadLockPoisoned,
    ReadOverflow,
    ReadOutOfBounds {
        offset: u64,
        length: usize,
        file_size: u64,
    },
    ReadLimit {
        requested: usize,
        maximum: usize,
    },
}

impl fmt::Display for McapSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueryLimit { resource, maximum } => {
                write!(f, "query exceeds {resource} limit ({maximum})")
            }
            Self::Mcap(error) => write!(f, "MCAP error: {error}"),
            Self::SummaryRequired => write!(f, "MCAP Summary is required for indexed playback"),
            Self::ChunkIndexRequired => {
                write!(f, "MCAP Chunk Index is required for indexed playback")
            }
            Self::Io(error) => write!(f, "source I/O: {error}"),
            Self::SourceChanged => write!(f, "recording changed after open"),
            Self::NotRegularFile => write!(f, "recording is not a regular file"),
            Self::ReadLockPoisoned => write!(f, "source read lock was poisoned"),
            Self::ReadOverflow => write!(f, "source range arithmetic overflow"),
            Self::ReadOutOfBounds {
                offset,
                length,
                file_size,
            } => write!(
                f,
                "source range {offset}+{length} exceeds {file_size} bytes"
            ),
            Self::ReadLimit { requested, maximum } => write!(
                f,
                "source record of {requested} bytes exceeds {maximum} byte limit"
            ),
            Self::QueryCancelled => write!(f, "recording query cancelled"),
            Self::TimestampOverflow => write!(f, "MCAP timestamp exceeds signed nanosecond range"),
            Self::Decompression { format, source } => write!(f, "{format} decompression: {source}"),
            Self::MessageIndexRequired(error) => write!(
                f,
                "MCAP Message Index is required to restore stream {}",
                error.stream.0
            ),
            Self::InvalidChunk(reason) => write!(f, "invalid MCAP Chunk: {reason}"),
            Self::UnsupportedCompression(compression) => {
                write!(f, "unsupported MCAP Chunk compression: {compression}")
            }
        }
    }
}

impl std::error::Error for McapSourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mcap(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Decompression { source, .. } => Some(source),
            Self::MessageIndexRequired(error) => Some(error),
            _ => None,
        }
    }
}
impl From<super::McapChunkError> for McapSourceError {
    fn from(error: super::McapChunkError) -> Self {
        match error {
            super::McapChunkError::Mcap(error) => Self::Mcap(error),
            super::McapChunkError::InvalidRecord(reason) => Self::InvalidChunk(reason.into()),
            super::McapChunkError::TimestampOverflow => Self::TimestampOverflow,
            super::McapChunkError::UnsupportedCompression(format) => {
                Self::UnsupportedCompression(format)
            }
            super::McapChunkError::Decompression { format, source } => {
                Self::Decompression { format, source }
            }
        }
    }
}
impl From<mcap::McapError> for McapSourceError {
    fn from(value: mcap::McapError) -> Self {
        Self::Mcap(value)
    }
}
