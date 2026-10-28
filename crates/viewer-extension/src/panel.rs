//! Portable declarative Panel protocol. No egui, MCAP, VM or GPU types.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub const PANEL_API_VERSION: u32 = 1;
pub const MAX_UPDATE_BYTES: usize = 128 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PanelDefinition {
    pub api_version: u32,
    pub id: String,
    pub title: String,
    pub topic: String,
    pub fields: Vec<String>,
    pub plots: Vec<String>,
    pub history_seconds: u32,
    pub max_points: usize,
}
impl PanelDefinition {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.api_version == PANEL_API_VERSION,
            "unsupported panel API"
        );
        ensure!(
            !self.id.is_empty()
                && self.id.len() <= 128
                && self.title.len() <= 256
                && !self.topic.is_empty()
                && self.topic.len() <= 1024,
            "invalid panel identity/topic"
        );
        ensure!(
            self.fields.len() <= 16
                && self.plots.len() <= 4
                && self.history_seconds >= 1
                && self.history_seconds <= 60
                && (1..=256).contains(&self.max_points),
            "panel demand exceeds limits"
        );
        for paths in [&self.fields, &self.plots] {
            let mut seen = BTreeSet::new();
            for p in paths {
                ensure!(
                    !p.is_empty() && p.len() <= 128 && seen.insert(p),
                    "invalid/duplicate selected field"
                );
            }
        }
        Ok(())
    }
}
/// Integer values are decimal strings: JS/Python need not round through a double.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum PanelScalar {
    Bool(bool),
    Signed(String),
    Unsigned(String),
    Number(f64),
    Text(String),
}
impl PanelScalar {
    pub fn display(&self) -> String {
        match self {
            Self::Bool(v) => v.to_string(),
            Self::Signed(v) | Self::Unsigned(v) | Self::Text(v) => v.clone(),
            Self::Number(v) => format!("{v:.2}"),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputStatus {
    Ready,
    MissingTopic,
    NoPredecessor,
    DecodeError,
    Unavailable,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PanelEvent {
    Click { id: String },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PanelUpdate {
    pub generation: u64,
    pub revision: u64,
    pub cursor_ns: String,
    pub source_start_ns: String,
    pub status: InputStatus,
    pub values: BTreeMap<String, PanelScalar>,
    /// Bounded min/max-reduced samples; x is seconds relative to source_start_ns.
    pub histories: BTreeMap<String, Vec<[f64; 2]>>,
    pub errors: Vec<String>,
    pub events: Vec<PanelEvent>,
    pub settings: serde_json::Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum UiNode {
    Text {
        text: String,
    },
    Value {
        label: String,
        value: String,
        unit: String,
    },
    Row {
        children: Vec<UiNode>,
    },
    Column {
        children: Vec<UiNode>,
    },
    Plot {
        title: String,
        series: Vec<String>,
    },
    Button {
        id: String,
        label: String,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum HostCommand {
    TogglePlayback,
    Seek { time_ns: String },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelOutput {
    pub root: UiNode,
    pub commands: Vec<HostCommand>,
}
impl PanelOutput {
    pub fn validate(&self, definition: &PanelDefinition, user_event: bool) -> Result<()> {
        ensure!(
            self.commands.len() <= 1 && (self.commands.is_empty() || user_event),
            "Host commands require a user event"
        );
        for c in &self.commands {
            if let HostCommand::Seek { time_ns } = c {
                time_ns.parse::<i64>()?;
            }
        }
        fn visit(
            node: &UiNode,
            depth: usize,
            budget: &mut usize,
            definition: &PanelDefinition,
            ids: &mut BTreeSet<String>,
        ) -> Result<()> {
            ensure!(depth <= 8 && *budget > 0, "panel UI tree exceeds limits");
            *budget -= 1;
            let check = |s: &str| -> Result<()> {
                ensure!(s.len() <= 2048, "panel UI text exceeds limit");
                Ok(())
            };
            match node {
                UiNode::Text { text } => check(text)?,
                UiNode::Value { label, value, unit } => {
                    check(label)?;
                    check(value)?;
                    check(unit)?;
                }
                UiNode::Row { children } | UiNode::Column { children } => {
                    ensure!(children.len() <= 16, "too many children");
                    for c in children {
                        visit(c, depth + 1, budget, definition, ids)?;
                    }
                }
                UiNode::Plot { title, series } => {
                    check(title)?;
                    ensure!(
                        series.len() <= 4 && series.iter().all(|id| definition.plots.contains(id)),
                        "plot references undeclared series"
                    );
                }
                UiNode::Button { id, label } => {
                    check(label)?;
                    ensure!(
                        !id.is_empty() && id.len() <= 128 && ids.insert(id.clone()),
                        "invalid/duplicate UI event id"
                    );
                }
            }
            Ok(())
        }
        visit(&self.root, 0, &mut 128, definition, &mut BTreeSet::new())
    }
}
