mod state;

#[cfg(target_arch = "wasm32")]
mod host;
#[cfg(target_arch = "wasm32")]
mod presenter;

#[cfg(target_arch = "wasm32")]
pub(crate) use host::WebGpuHost;
#[cfg(target_arch = "wasm32")]
pub(crate) use state::{PhysicalSize, physical_size};
