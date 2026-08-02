use crate::PreviewMcapError;
use mcap::{Summary, read::footer};
use viewer_core::SourceFingerprint;

pub const SOURCE_FINGERPRINT_ALGORITHM: &str = "mcap-summary-identity-v1";

pub fn source_fingerprint(bytes: &[u8]) -> Result<SourceFingerprint, PreviewMcapError> {
    let footer = footer(bytes)?;
    let summary = Summary::read(bytes)?
        .ok_or_else(|| PreviewMcapError::invalid("source MCAP has no summary"))?;
    let stats = summary
        .stats
        .as_ref()
        .ok_or_else(|| PreviewMcapError::invalid("source MCAP summary has no statistics"))?;
    let value = format!(
        "{}:{:08x}:{}:{}:{}:{}:{}:{}",
        bytes.len(),
        footer.summary_crc,
        stats.message_start_time,
        stats.message_end_time,
        stats.message_count,
        stats.schema_count,
        stats.channel_count,
        summary.chunk_indexes.len(),
    );
    SourceFingerprint::new(SOURCE_FINGERPRINT_ALGORITHM, value)
        .map_err(|error| PreviewMcapError::invalid(error.to_string()))
}
