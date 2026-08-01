use std::{error::Error, fmt};

#[derive(Debug)]
pub struct LayoutLoadError {
    source: serde_json::Error,
}

impl LayoutLoadError {
    pub(crate) fn new(source: serde_json::Error) -> Self {
        Self { source }
    }
}

impl fmt::Display for LayoutLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "deserialize layout: {}", self.source)
    }
}

impl Error for LayoutLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Debug)]
pub struct LayoutSaveError {
    source: serde_json::Error,
}

impl LayoutSaveError {
    pub(crate) fn new(source: serde_json::Error) -> Self {
        Self { source }
    }
}

impl fmt::Display for LayoutSaveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "serialize layout: {}", self.source)
    }
}

impl Error for LayoutSaveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutValidationError {
    message: String,
}

impl LayoutValidationError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for LayoutValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for LayoutValidationError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanelIdError;

impl fmt::Display for PanelIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("panel id must not be empty or whitespace")
    }
}

impl Error for PanelIdError {}
