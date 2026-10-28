//! Shared Scene extension contracts and guest wire SDK, with a Native VM adapter.
#![forbid(unsafe_code)]

#[cfg(not(target_arch = "wasm32"))]
mod wasm;
#[cfg(not(target_arch = "wasm32"))]
pub use wasm::WasmExtension;

use anyhow::{Result, ensure};
use serde::Deserialize;
use viewer_core::{Detection3DFrame, MeasurementTime, PathFrame, SceneOverlay};

pub const API_VERSION: u32 = 3;
pub const MAX_INPUT_POINTS: usize = 1024;
pub const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;

pub mod wire;

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
        self.input_kind()?;
        Ok(())
    }
    pub fn input_kind(&self) -> Result<ExtensionInputKind> {
        match (self.schema.as_str(), self.input.as_str()) {
            ("nav_msgs/msg/Path", "path_positions_v1") => Ok(ExtensionInputKind::PathPositions),
            ("vision_msgs/msg/Detection3DArray", "detection3d_array_v1") => {
                Ok(ExtensionInputKind::Detection3DArray)
            }
            _ => anyhow::bail!("unsupported extension schema/input projection"),
        }
    }
}

/// Native Rust implementations can use exactly the same output contract without a VM.
/// Inputs are borrowed source feature facts: partial Path positions or full Detection3DArray.
pub trait VisualizationExtension {
    fn descriptor(&self) -> &ExtensionDescriptor;
    fn update(&mut self, input: ExtensionInput<'_>) -> Result<SceneOverlay>;
    /// A fresh isolated instance starts each committed seek/source generation.
    fn reset(&mut self) -> Result<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionInputKind {
    PathPositions,
    Detection3DArray,
}

#[derive(Clone, Copy)]
pub enum ExtensionInput<'a> {
    Path(&'a PathFrame),
    Detections(&'a Detection3DFrame),
}
impl ExtensionInput<'_> {
    pub fn placement(&self) -> Result<(&str, MeasurementTime)> {
        match self {
            Self::Path(path) => Ok((&path.frame_id, path.measurement_time)),
            Self::Detections(frame) => {
                let header = &frame.message.header;
                ensure!(
                    frame
                        .message
                        .detections
                        .iter()
                        .all(|d| d.header.stamp == header.stamp
                            && d.header.frame_id.trim_start_matches('/')
                                == header.frame_id.trim_start_matches('/')),
                    "mixed Detection3D headers need per-object placement; currently require the array frame/time"
                );
                Ok((&header.frame_id, header.stamp))
            }
        }
    }
    pub fn kind(&self) -> ExtensionInputKind {
        match self {
            Self::Path(_) => ExtensionInputKind::PathPositions,
            Self::Detections(_) => ExtensionInputKind::Detection3DArray,
        }
    }
}

pub mod panel;
#[cfg(not(target_arch = "wasm32"))]
mod sandbox;
#[cfg(not(target_arch = "wasm32"))]
mod wasm_panel;
#[cfg(not(target_arch = "wasm32"))]
pub use wasm_panel::WasmPanel;
