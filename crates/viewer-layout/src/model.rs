use crate::{LayoutLoadError, LayoutSaveError, LayoutValidationError, PanelIdError};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const CURRENT_LAYOUT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutDocument {
    pub schema_version: u32,
    pub root: LayoutNode,
}

impl LayoutDocument {
    pub fn from_json(json: &str) -> Result<Self, LayoutLoadError> {
        serde_json::from_str(json).map_err(LayoutLoadError::new)
    }

    pub fn to_json_pretty(&self) -> Result<String, LayoutSaveError> {
        serde_json::to_string_pretty(self).map_err(LayoutSaveError::new)
    }

    pub fn validate(&self) -> Result<(), LayoutValidationError> {
        crate::validation::validate(self)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum LayoutNode {
    Split {
        direction: SplitDirection,
        children: Vec<SplitChild>,
    },
    Panel(PanelNode),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SplitDirection {
    Row,
    Column,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitChild {
    pub weight: f32,
    pub node: LayoutNode,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PanelId(String);

impl PanelId {
    pub fn new(value: impl Into<String>) -> Result<Self, PanelIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(PanelIdError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PanelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelNode {
    pub id: PanelId,
    pub panel_type: String,
    pub config_version: u32,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub config: serde_json::Value,
}
