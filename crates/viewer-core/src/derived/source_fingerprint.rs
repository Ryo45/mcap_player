//! Format-neutral identity used to bind derived artifacts to their source.
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFingerprintValidationError {
    message: String,
}

impl SourceFingerprintValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SourceFingerprintValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for SourceFingerprintValidationError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", try_from = "SourceFingerprintWire")]
pub struct SourceFingerprint {
    algorithm: String,
    value: String,
}

impl SourceFingerprint {
    pub fn new(
        algorithm: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, SourceFingerprintValidationError> {
        let fingerprint = Self {
            algorithm: algorithm.into(),
            value: value.into(),
        };
        fingerprint.validate()?;
        Ok(fingerprint)
    }

    pub fn algorithm(&self) -> &str {
        &self.algorithm
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn validate(&self) -> Result<(), SourceFingerprintValidationError> {
        if self.algorithm.trim().is_empty() {
            return Err(SourceFingerprintValidationError::new(
                "source fingerprint algorithm must not be empty",
            ));
        }
        if self.value.trim().is_empty() {
            return Err(SourceFingerprintValidationError::new(
                "source fingerprint value must not be empty",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceFingerprintWire {
    algorithm: String,
    value: String,
}

impl TryFrom<SourceFingerprintWire> for SourceFingerprint {
    type Error = SourceFingerprintValidationError;

    fn try_from(value: SourceFingerprintWire) -> Result<Self, Self::Error> {
        Self::new(value.algorithm, value.value)
    }
}
