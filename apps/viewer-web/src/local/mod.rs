#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_imports))]

mod catalog;
mod loader;

pub(crate) use catalog::LocalCatalog;
#[cfg(test)]
pub(crate) use loader::collect_window_from_bytes_for_test;
#[cfg(target_arch = "wasm32")]
pub(crate) use loader::{BrowserMcapRecording, BrowserMcapWindowLoader, open_browser_mcap};
