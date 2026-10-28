use crate::sandbox::{Sandbox, read_bounded};
use crate::{API_VERSION, ExtensionDescriptor, ExtensionInput, VisualizationExtension, wire};
use anyhow::{Result, ensure};
use std::path::Path;
use viewer_core::{MAX_OVERLAY_BOXES, SceneBox, SceneObjectId, SceneOverlay};
const BOX_BYTES: usize = 60;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WasmManifest {
    wasm: String,
    extension: ExtensionDescriptor,
}

pub struct WasmExtension {
    descriptor: ExtensionDescriptor,
    vm: Sandbox,
}

impl WasmExtension {
    pub fn load(manifest: &Path) -> Result<Self> {
        let manifest_data: WasmManifest = serde_json::from_slice(&read_bounded(manifest, 8192)?)?;
        manifest_data.extension.validate()?;
        ensure!(
            !manifest_data.wasm.is_empty() && manifest_data.wasm.len() <= 1024,
            "invalid module path"
        );
        let bytes = read_bounded(
            &manifest
                .parent()
                .unwrap_or(Path::new("."))
                .join(&manifest_data.wasm),
            2 * 1024 * 1024,
        )?;
        Self::from_bytes(manifest_data.extension, &bytes)
    }

    pub fn from_bytes(descriptor: ExtensionDescriptor, bytes: &[u8]) -> Result<Self> {
        descriptor.validate()?;
        let vm = Sandbox::new(
            bytes,
            "api_version",
            API_VERSION,
            1_000_000,
            8 * 1024 * 1024,
        )?;
        Ok(Self { descriptor, vm })
    }
}

impl VisualizationExtension for WasmExtension {
    fn descriptor(&self) -> &ExtensionDescriptor {
        &self.descriptor
    }

    fn update(&mut self, input: ExtensionInput<'_>) -> Result<SceneOverlay> {
        ensure!(
            input.kind() == self.descriptor.input_kind()?,
            "extension input kind mismatch"
        );
        input.placement()?;
        let input = wire::encode_input(input)?;
        self.vm.begin(&input)?;
        let count = self.vm.invoke("update", input.len() as u32)? as usize;
        ensure!(
            count <= MAX_OVERLAY_BOXES,
            "extension output exceeds box limit"
        );
        let output = self
            .vm
            .output(count * BOX_BYTES, MAX_OVERLAY_BOXES * BOX_BYTES)?;
        let boxes = output
            .chunks_exact(BOX_BYTES)
            .map(|bytes| {
                let id = SceneObjectId(u64::from_le_bytes(
                    bytes[..8].try_into().expect("fixed box record"),
                ));
                let mut values = [0f32; 13];
                for (value, bytes) in values.iter_mut().zip(bytes[8..].chunks_exact(4)) {
                    *value = f32::from_le_bytes(bytes.try_into().expect("fixed f32 field"));
                }
                SceneBox {
                    id,
                    center: [values[0], values[1], values[2]],
                    orientation: [values[3], values[4], values[5], values[6]],
                    size: [values[7], values[8], values[9]],
                    color: [values[10], values[11], values[12]],
                }
            })
            .collect();
        Ok(SceneOverlay::new(vec![], boxes)?)
    }

    fn reset(&mut self) -> Result<()> {
        self.vm.reset()
    }
}
