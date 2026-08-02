use crate::{BookmarkValidationError, SourceFingerprint};

pub const MCAP_SUMMARY_IDENTITY_ALGORITHM: &str = "mcap-summary-identity-v1";

/// Inputs to the path-independent, non-cryptographic MCAP sidecar identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct McapSummaryIdentity {
    pub file_size: u64,
    pub summary_crc: u32,
    pub message_start_time: u64,
    pub message_end_time: u64,
    pub message_count: u64,
    pub schema_count: u16,
    pub channel_count: u32,
    pub chunk_count: usize,
}

impl McapSummaryIdentity {
    pub fn value(self) -> String {
        format!(
            "{}:{:08x}:{}:{}:{}:{}:{}:{}",
            self.file_size,
            self.summary_crc,
            self.message_start_time,
            self.message_end_time,
            self.message_count,
            self.schema_count,
            self.channel_count,
            self.chunk_count,
        )
    }

    pub fn revision(self) -> String {
        format!("{}:{}", MCAP_SUMMARY_IDENTITY_ALGORITHM, self.value())
    }
}

pub fn mcap_summary_fingerprint(
    identity: McapSummaryIdentity,
) -> Result<SourceFingerprint, BookmarkValidationError> {
    SourceFingerprint::new(MCAP_SUMMARY_IDENTITY_ALGORITHM, identity.value())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_canonical_and_path_independent() {
        let identity = McapSummaryIdentity {
            file_size: 596_121_452,
            summary_crc: 0x9cc5_ea08,
            message_start_time: 10,
            message_end_time: 20,
            message_count: 30,
            schema_count: 9,
            channel_count: 18,
            chunk_count: 3517,
        };
        assert_eq!(identity.value(), "596121452:9cc5ea08:10:20:30:9:18:3517");
        assert_eq!(
            identity.revision(),
            "mcap-summary-identity-v1:596121452:9cc5ea08:10:20:30:9:18:3517"
        );
    }
}
