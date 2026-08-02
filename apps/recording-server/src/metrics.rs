use std::time::Instant;

use crate::file_reader::ReadMetrics;

#[derive(Debug)]
pub(crate) struct RequestMetrics {
    pub(crate) request_id: u64,
    pub(crate) reads: ReadMetrics,
    pub(crate) chunk_count: u64,
    pub(crate) chunk_decompress_ms: f64,
    pub(crate) message_filter_ms: f64,
    pub(crate) batch_encode_ms: f64,
    pub(crate) started: Instant,
}

impl RequestMetrics {
    pub(crate) fn new(request_id: u64) -> Self {
        Self {
            request_id,
            reads: ReadMetrics::default(),
            chunk_count: 0,
            chunk_decompress_ms: 0.0,
            message_filter_ms: 0.0,
            batch_encode_ms: 0.0,
            started: Instant::now(),
        }
    }
}
