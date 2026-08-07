#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_imports))]

mod catalog;
mod client;
mod playback;
#[cfg(target_arch = "wasm32")]
mod smoke;
mod source;

pub(crate) use catalog::{RemoteCatalog, adapt_catalog};
pub(crate) use client::RemoteApiClient;
#[cfg(target_arch = "wasm32")]
pub(crate) use client::{RemoteBatchRequest, RemoteClientError, RequestGeneration};
pub(crate) use playback::{RemotePlayback, WebPlayback};
#[cfg(target_arch = "wasm32")]
pub(crate) use smoke::install;
pub(crate) use source::{RemoteBatchSource, RemoteSourceMetrics};
