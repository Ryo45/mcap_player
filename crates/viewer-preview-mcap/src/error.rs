use std::{error::Error, fmt};

#[derive(Debug)]
pub enum PreviewMcapError {
    Mcap(mcap::McapError),
    Json(serde_json::Error),
    Invalid(String),
    MissingBuildInfo,
    DuplicateBuildInfo,
    StalePreview { expected: String, actual: String },
}

impl PreviewMcapError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

impl fmt::Display for PreviewMcapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mcap(error) => write!(f, "MCAP error: {error}"),
            Self::Json(error) => write!(f, "JSON error: {error}"),
            Self::Invalid(message) => f.write_str(message),
            Self::MissingBuildInfo => f.write_str("preview artifact has no BuildInfo message"),
            Self::DuplicateBuildInfo => {
                f.write_str("preview artifact has more than one BuildInfo message")
            }
            Self::StalePreview { expected, actual } => write!(
                f,
                "preview source fingerprint mismatch (expected {expected}, got {actual})"
            ),
        }
    }
}

impl Error for PreviewMcapError {}

impl From<mcap::McapError> for PreviewMcapError {
    fn from(value: mcap::McapError) -> Self {
        Self::Mcap(value)
    }
}

impl From<serde_json::Error> for PreviewMcapError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
