#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_imports))]

mod catalog;
mod client;
mod data_plane;
mod loader;
mod playback;
#[cfg(target_arch = "wasm32")]
mod smoke;

pub(crate) use catalog::{RemoteCatalog, adapt_catalog};
pub(crate) use client::RemoteApiClient;
#[cfg(target_arch = "wasm32")]
pub(crate) use client::{RemoteBatchRequest, RemoteClientError, RequestGeneration};
pub(crate) use data_plane::{RecordingDataPlane, RecordingDataPlaneDiagnostics};
pub(crate) use loader::RemoteWindowLoader;
#[cfg(test)]
pub(crate) use loader::{LoadedWindow, WindowLoadDiagnostics};
pub(crate) use playback::{RemotePlayback, WebPlayback};
#[cfg(target_arch = "wasm32")]
pub(crate) use smoke::install;
