#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_imports))]

mod catalog;
mod client;
mod loader;
#[cfg(target_arch = "wasm32")]
mod smoke;

pub(crate) use catalog::{RemoteCatalog, adapt_catalog};
pub(crate) use client::RemoteApiClient;
#[cfg(test)]
pub(crate) use client::RemoteBatchPage;
#[cfg(target_arch = "wasm32")]
pub(crate) use client::{RemoteBatchRequest, RemoteClientError, RequestGeneration};
pub(crate) use loader::RemoteRestoreLoader;
pub(crate) use loader::RemoteWindowLoader;
#[cfg(test)]
pub(crate) use loader::assemble_pages_for_test;
#[cfg(target_arch = "wasm32")]
pub(crate) use smoke::install;
