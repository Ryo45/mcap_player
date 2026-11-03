use crate::{
    panel::{
        MAX_OUTPUT_BYTES, MAX_UPDATE_BYTES, PANEL_API_VERSION, PanelDefinition, PanelOutput,
        PanelUpdate,
    },
    sandbox::{Sandbox, read_bounded},
};
use anyhow::{Result, ensure};
use std::path::Path;
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    wasm: String,
    #[serde(default)]
    settings: serde_json::Value,
}
pub struct WasmPanel {
    vm: Sandbox,
    definition: PanelDefinition,
    settings: serde_json::Value,
}
impl WasmPanel {
    pub fn load(path: &Path) -> Result<Self> {
        let manifest: Manifest = serde_json::from_slice(&read_bounded(path, 8192)?)?;
        ensure!(
            !manifest.wasm.is_empty() && manifest.wasm.len() <= 1024,
            "invalid panel module path"
        );
        Self::from_bytes(
            &read_bounded(
                &path.parent().unwrap_or(Path::new(".")).join(&manifest.wasm),
                2 * 1024 * 1024,
            )?,
            manifest.settings,
        )
    }
    pub fn from_bytes(bytes: &[u8], settings: serde_json::Value) -> Result<Self> {
        let mut vm = Sandbox::new(
            bytes,
            "panel_api_version",
            PANEL_API_VERSION,
            5_000_000,
            16 * 1024 * 1024,
        )?;
        let input = serde_json::to_vec(&settings)?;
        ensure!(input.len() <= 8192, "panel settings exceed limit");
        vm.begin(&input)?;
        let len = vm.invoke("define", input.len() as u32)? as usize;
        let definition: PanelDefinition = serde_json::from_slice(&vm.output(len, 8192)?)?;
        definition.validate()?;
        Ok(Self {
            vm,
            definition,
            settings,
        })
    }
    pub fn definition(&self) -> &PanelDefinition {
        &self.definition
    }
    pub fn settings(&self) -> &serde_json::Value {
        &self.settings
    }
    pub fn reset(&mut self) -> Result<()> {
        self.vm.reset()?;
        let input = serde_json::to_vec(&self.settings)?;
        self.vm.begin(&input)?;
        let len = self.vm.invoke("define", input.len() as u32)? as usize;
        let definition: PanelDefinition = serde_json::from_slice(&self.vm.output(len, 8192)?)?;
        ensure!(
            definition == self.definition,
            "panel definition changed during reset"
        );
        Ok(())
    }
    pub fn update(&mut self, update: &PanelUpdate) -> Result<PanelOutput> {
        ensure!(update.events.len() <= 1, "too many panel events");
        let input = serde_json::to_vec(update)?;
        ensure!(input.len() <= MAX_UPDATE_BYTES, "panel input exceeds limit");
        self.vm.begin(&input)?;
        let len = self.vm.invoke("update", input.len() as u32)? as usize;
        let output: PanelOutput = serde_json::from_slice(&self.vm.output(len, MAX_OUTPUT_BYTES)?)?;
        output.validate(&self.definition, !update.events.is_empty())?;
        Ok(output)
    }
}
