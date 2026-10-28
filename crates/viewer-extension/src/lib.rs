//! Native execution adapter for the bounded Scene extension ABI.
#![forbid(unsafe_code)]

#[cfg(not(target_arch = "wasm32"))]
mod wasm;
#[cfg(not(target_arch = "wasm32"))]
pub use wasm::WasmExtension;

use anyhow::{Result, ensure};
use serde::Deserialize;
use viewer_core::{PathFrame, SceneOverlay};

pub const API_VERSION: u32 = 1;
pub const MAX_INPUT_POINTS: usize = 1024;

/// Deployment facts, resolved once at startup. Not a subscription interface.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionDescriptor {
    pub id: String,
    pub api_version: u32,
    pub topic: String,
    pub schema: String,
    pub input: String,
}

impl ExtensionDescriptor {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.id.is_empty() && self.id.len() <= 128,
            "invalid extension id"
        );
        ensure!(
            self.api_version == API_VERSION,
            "unsupported extension API version"
        );
        ensure!(
            !self.topic.is_empty() && self.topic.len() <= 1024,
            "invalid extension topic"
        );
        ensure!(
            self.schema == "nav_msgs/msg/Path" && self.input == "path_positions_v1",
            "only Path positions are implemented; this is not a full ROS Path model"
        );
        Ok(())
    }
}

/// Native Rust implementations can use exactly the same output contract without a VM.
/// Input is the existing partial Path feature (positions, frame and times), not full PoseStamped.
pub trait VisualizationExtension {
    fn descriptor(&self) -> &ExtensionDescriptor;
    fn update(&mut self, path: &PathFrame) -> Result<SceneOverlay>;
    /// A fresh isolated instance starts each committed seek/source generation.
    fn reset(&mut self) -> Result<()>;
}
