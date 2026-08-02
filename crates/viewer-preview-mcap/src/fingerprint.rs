use crate::PreviewMcapError;
use mcap::{Summary, read::footer};
use viewer_core::{
    MCAP_SUMMARY_IDENTITY_ALGORITHM, McapSummaryIdentity, SourceFingerprint,
    mcap_summary_fingerprint,
};

pub const SOURCE_FINGERPRINT_ALGORITHM: &str = MCAP_SUMMARY_IDENTITY_ALGORITHM;

pub fn source_fingerprint(bytes: &[u8]) -> Result<SourceFingerprint, PreviewMcapError> {
    let footer = footer(bytes)?;
    let summary = Summary::read(bytes)?
        .ok_or_else(|| PreviewMcapError::invalid("source MCAP has no summary"))?;
    let stats = summary
        .stats
        .as_ref()
        .ok_or_else(|| PreviewMcapError::invalid("source MCAP summary has no statistics"))?;
    mcap_summary_fingerprint(McapSummaryIdentity {
        file_size: bytes.len() as u64,
        summary_crc: footer.summary_crc,
        message_start_time: stats.message_start_time,
        message_end_time: stats.message_end_time,
        message_count: stats.message_count,
        schema_count: stats.schema_count,
        channel_count: stats.channel_count,
        chunk_count: summary.chunk_indexes.len(),
    })
    .map_err(|error| PreviewMcapError::invalid(error.to_string()))
}
