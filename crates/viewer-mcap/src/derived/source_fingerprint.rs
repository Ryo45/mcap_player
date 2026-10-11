use crate::{
    BookmarkValidationError, MCAP_SUMMARY_IDENTITY_ALGORITHM, McapSummaryIdentity,
    SourceFingerprint,
};

/// Converts physical MCAP summary facts into the identity stored by derived artifacts.
pub fn mcap_summary_fingerprint(
    identity: McapSummaryIdentity,
) -> Result<SourceFingerprint, BookmarkValidationError> {
    SourceFingerprint::new(MCAP_SUMMARY_IDENTITY_ALGORITHM, identity.value())
}
