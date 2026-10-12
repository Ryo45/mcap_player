use crate::{
    MCAP_SUMMARY_IDENTITY_ALGORITHM, McapSummaryIdentity, SourceFingerprint,
    SourceFingerprintValidationError,
};

/// Converts physical MCAP summary facts into the identity stored by derived artifacts.
pub fn mcap_summary_fingerprint(
    identity: McapSummaryIdentity,
) -> Result<SourceFingerprint, SourceFingerprintValidationError> {
    SourceFingerprint::new(MCAP_SUMMARY_IDENTITY_ALGORITHM, identity.value())
}
