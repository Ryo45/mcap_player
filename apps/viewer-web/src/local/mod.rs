mod catalog;
mod loader;

pub(crate) use catalog::LocalCatalog;
#[cfg(test)]
pub(crate) use loader::collect_window_from_bytes_for_test;
#[cfg(target_arch = "wasm32")]
pub(crate) use loader::{
    BrowserMcapRecording, BrowserMcapRestoreLoader, BrowserMcapWindowLoader, open_browser_mcap,
};
