//! Native VM mechanics shared by Scene and Panel protocols; no application/query semantics.
use anyhow::{Context, Result, ensure};
use std::{fs::File, io::Read, path::Path};
use wasmi::{
    Config, EnforcedLimits, Engine, Instance, Linker, Module, Store, StoreLimits,
    StoreLimitsBuilder,
};
const MAX_MODULE_BYTES: usize = 2 * 1024 * 1024;

pub(crate) fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
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
pub(crate) struct Sandbox {
    engine: Engine,
    module: Module,
    store: Store<StoreLimits>,
    instance: Instance,
    version_export: &'static str,
    version: u32,
    fuel: u64,
    memory_bytes: usize,
}
impl Sandbox {
    pub(crate) fn new(
        bytes: &[u8],
        version_export: &'static str,
        version: u32,
        fuel: u64,
        memory_bytes: usize,
    ) -> Result<Self> {
        ensure!(bytes.len() <= MAX_MODULE_BYTES, "module exceeds size limit");
        let mut config = Config::default();
        config
            .consume_fuel(true)
            .allow_start_fn(false)
            .enforced_limits(EnforcedLimits::strict())
            .set_min_stack_height(1024)
            .set_max_stack_height(256 * 1024)
            .set_max_recursion_depth(128);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes)?;
        let imports = module
            .imports()
            .take(8)
            .map(|i| format!("{}::{}", i.module(), i.name()))
            .collect::<Vec<_>>();
        ensure!(
            imports.is_empty(),
            "extension imports are forbidden (including WASI): {}",
            imports.join(", ")
        );
        let (store, instance) = instantiate(
            &engine,
            &module,
            version_export,
            version,
            fuel,
            memory_bytes,
        )?;
        Ok(Self {
            engine,
            module,
            store,
            instance,
            version_export,
            version,
            fuel,
            memory_bytes,
        })
    }
    pub(crate) fn reset(&mut self) -> Result<()> {
        let (store, instance) = instantiate(
            &self.engine,
            &self.module,
            self.version_export,
            self.version,
            self.fuel,
            self.memory_bytes,
        )?;
        self.store = store;
        self.instance = instance;
        Ok(())
    }
    fn scalar(&mut self, name: &str) -> Result<u32> {
        Ok(self
            .instance
            .get_typed_func::<(), u32>(&self.store, name)?
            .call(&mut self.store, ())?)
    }
    pub(crate) fn begin(&mut self, input: &[u8]) -> Result<()> {
        self.store.set_fuel(self.fuel)?;
        let ptr = self.scalar("input_ptr")? as usize;
        self.instance
            .get_memory(&self.store, "memory")
            .context("missing memory")?
            .write(&mut self.store, ptr, input)
            .map_err(|e| anyhow::anyhow!("input memory: {e}"))?;
        Ok(())
    }
    pub(crate) fn invoke(&mut self, name: &str, argument: u32) -> Result<u32> {
        Ok(self
            .instance
            .get_typed_func::<u32, u32>(&self.store, name)?
            .call(&mut self.store, argument)?)
    }
    pub(crate) fn output(&mut self, length: usize, limit: usize) -> Result<Vec<u8>> {
        ensure!(length <= limit, "extension output exceeds byte limit");
        let ptr = self.scalar("output_ptr")? as usize;
        let mut bytes = vec![0; length];
        self.instance
            .get_memory(&self.store, "memory")
            .context("missing memory")?
            .read(&self.store, ptr, &mut bytes)
            .map_err(|e| anyhow::anyhow!("output memory: {e}"))?;
        Ok(bytes)
    }
}
fn instantiate(
    engine: &Engine,
    module: &Module,
    version_export: &str,
    version: u32,
    fuel: u64,
    memory_bytes: usize,
) -> Result<(Store<StoreLimits>, Instance)> {
    let limits = StoreLimitsBuilder::new()
        .memory_size(memory_bytes)
        .instances(1)
        .memories(1)
        .tables(1)
        .table_elements(1024)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(engine, limits);
    store.limiter(|limits| limits);
    store.set_fuel(fuel)?;
    let instance = Linker::<StoreLimits>::new(engine).instantiate_and_start(&mut store, module)?;
    let found = instance
        .get_typed_func::<(), u32>(&store, version_export)?
        .call(&mut store, ())?;
    ensure!(found == version, "Wasm ABI version mismatch");
    instance
        .get_typed_func::<(), ()>(&store, "init")?
        .call(&mut store, ())?;
    ensure!(
        instance.get_memory(&store, "memory").is_some(),
        "missing exported memory"
    );
    Ok((store, instance))
}
