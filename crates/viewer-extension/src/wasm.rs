use crate::{API_VERSION, ExtensionDescriptor, MAX_INPUT_POINTS, VisualizationExtension};
use anyhow::{Context, Result, ensure};
use std::{fs::File, io::Read, path::Path};
use viewer_core::{MAX_OVERLAY_LINES, PathFrame, SceneLine, SceneOverlay};
use wasmi::{
    Config, EnforcedLimits, Engine, Instance, Linker, Module, Store, StoreLimits,
    StoreLimitsBuilder,
};

const MAX_MODULE_BYTES: usize = 2 * 1024 * 1024;
const MAX_MEMORY_BYTES: usize = 8 * 1024 * 1024;
const FUEL: u64 = 1_000_000;
const LINE_BYTES: usize = 40;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WasmManifest {
    wasm: String,
    extension: ExtensionDescriptor,
}

pub struct WasmExtension {
    descriptor: ExtensionDescriptor,
    engine: Engine,
    module: Module,
    store: Store<StoreLimits>,
    instance: Instance,
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)
        .with_context(|| format!("open {}", path.display()))?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "{} exceeds {limit} bytes",
        path.display()
    );
    Ok(bytes)
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
            MAX_MODULE_BYTES,
        )?;
        Self::from_bytes(manifest_data.extension, &bytes)
    }

    pub fn from_bytes(descriptor: ExtensionDescriptor, bytes: &[u8]) -> Result<Self> {
        descriptor.validate()?;
        ensure!(bytes.len() <= MAX_MODULE_BYTES, "module exceeds size limit");
        let mut config = Config::default();
        config.consume_fuel(true);
        config.allow_start_fn(false);
        config.enforced_limits(EnforcedLimits::strict());
        config.set_min_stack_height(1024);
        config.set_max_stack_height(256 * 1024);
        config.set_max_recursion_depth(128);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes)?;
        ensure!(
            module.imports().next().is_none(),
            "extension imports are forbidden (including WASI)"
        );
        let (store, instance) = instantiate(&engine, &module)?;
        Ok(Self {
            descriptor,
            engine,
            module,
            store,
            instance,
        })
    }

    fn call_u32(&mut self, name: &str) -> Result<u32> {
        Ok(self
            .instance
            .get_typed_func::<(), u32>(&self.store, name)?
            .call(&mut self.store, ())?)
    }
}

fn instantiate(engine: &Engine, module: &Module) -> Result<(Store<StoreLimits>, Instance)> {
    let limits = StoreLimitsBuilder::new()
        .memory_size(MAX_MEMORY_BYTES)
        .instances(1)
        .memories(1)
        .tables(1)
        .table_elements(1024)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(engine, limits);
    store.limiter(|limits| limits);
    store.set_fuel(FUEL)?;
    // No unmetered user initializer; init itself shares the same instruction budget.
    let instance = Linker::<StoreLimits>::new(engine).instantiate_and_start(&mut store, module)?;
    let version = instance
        .get_typed_func::<(), u32>(&store, "api_version")?
        .call(&mut store, ())?;
    ensure!(version == API_VERSION, "Wasm ABI version mismatch");
    instance
        .get_typed_func::<(), ()>(&store, "init")?
        .call(&mut store, ())?;
    ensure!(
        instance.get_memory(&store, "memory").is_some(),
        "missing exported memory"
    );
    Ok((store, instance))
}

impl VisualizationExtension for WasmExtension {
    fn descriptor(&self) -> &ExtensionDescriptor {
        &self.descriptor
    }

    fn update(&mut self, path: &PathFrame) -> Result<SceneOverlay> {
        ensure!(
            path.points.len() <= MAX_INPUT_POINTS,
            "Path exceeds extension input limit ({MAX_INPUT_POINTS} points)"
        );
        ensure!(
            path.points.iter().flatten().all(|p| p.is_finite()),
            "non-finite input position"
        );
        self.store.set_fuel(FUEL)?;
        let input_ptr = self.call_u32("input_ptr")? as usize;
        let memory = self
            .instance
            .get_memory(&self.store, "memory")
            .context("missing memory")?;
        // Explicit little-endian wire encoding; no shared Rust layout or guest pointers.
        let mut input = Vec::with_capacity(16 + path.points.len() * 24);
        input.extend_from_slice(&path.arrival_time.0.to_le_bytes());
        input.extend_from_slice(&path.measurement_time.0.to_le_bytes());
        for position in &path.points {
            for value in position {
                input.extend_from_slice(&value.to_le_bytes());
            }
        }
        memory
            .write(&mut self.store, input_ptr, &input)
            .map_err(|e| anyhow::anyhow!("input memory: {e}"))?;
        let count = self
            .instance
            .get_typed_func::<u32, u32>(&self.store, "update")?
            .call(&mut self.store, path.points.len() as u32)? as usize;
        ensure!(
            count <= MAX_OVERLAY_LINES,
            "extension output exceeds line limit"
        );
        let output_ptr = self.call_u32("output_ptr")? as usize;
        let mut output = vec![0; count * LINE_BYTES];
        memory
            .read(&self.store, output_ptr, &mut output)
            .map_err(|e| anyhow::anyhow!("output memory: {e}"))?;
        let lines = output
            .chunks_exact(LINE_BYTES)
            .map(|bytes| {
                let id = u32::from_le_bytes(bytes[..4].try_into().expect("fixed line record"));
                let mut values = [0f32; 9];
                for (value, bytes) in values.iter_mut().zip(bytes[4..].chunks_exact(4)) {
                    *value = f32::from_le_bytes(bytes.try_into().expect("fixed f32 field"));
                }
                SceneLine {
                    id,
                    start: [values[0], values[1], values[2]],
                    end: [values[3], values[4], values[5]],
                    color: [values[6], values[7], values[8]],
                }
            })
            .collect();
        Ok(SceneOverlay::new(lines)?)
    }

    fn reset(&mut self) -> Result<()> {
        let (store, instance) = instantiate(&self.engine, &self.module)?;
        self.store = store;
        self.instance = instance;
        Ok(())
    }
}
