#[cfg(any(test, target_arch = "wasm32"))]
mod data_plane;
#[cfg(any(test, target_arch = "wasm32"))]
mod inspector_query;
#[cfg(any(test, target_arch = "wasm32"))]
mod local;
#[cfg(any(test, target_arch = "wasm32"))]
mod playback;
#[cfg(any(test, target_arch = "wasm32"))]
mod remote;
#[cfg(any(test, target_arch = "wasm32"))]
mod topics;

#[cfg(any(test, target_arch = "wasm32"))]
mod webgpu;

#[cfg(target_arch = "wasm32")]
mod app;

#[cfg(target_arch = "wasm32")]
pub use app::start;

#[cfg(not(target_arch = "wasm32"))]
pub fn start() {}
