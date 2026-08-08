#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_imports))]

mod catalog;
mod loader;

pub(crate) use catalog::LocalCatalog;
#[cfg(target_arch = "wasm32")]
pub(crate) use loader::{BrowserMcapRecording, BrowserMcapWindowLoader, open_browser_mcap};
