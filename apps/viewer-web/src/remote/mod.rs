mod catalog;
mod client;
mod loader;
#[cfg(target_arch = "wasm32")]
mod source_control;

pub(crate) use catalog::{RemoteCatalog, adapt_catalog};
pub(crate) use client::RemoteApiClient;
#[cfg(test)]
pub(crate) use client::RemoteBatchPage;
#[cfg(target_arch = "wasm32")]
pub(crate) use client::RequestGeneration;
pub(crate) use loader::RemoteRestoreLoader;
pub(crate) use loader::RemoteWindowLoader;
#[cfg(test)]
pub(crate) use loader::assemble_pages_for_test;
#[cfg(target_arch = "wasm32")]
pub(crate) use source_control::install;
