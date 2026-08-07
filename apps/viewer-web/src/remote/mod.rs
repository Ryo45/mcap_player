mod client;
#[cfg(target_arch = "wasm32")]
mod smoke;

#[cfg(target_arch = "wasm32")]
pub(crate) use client::{
    RemoteApiClient, RemoteBatchRequest, RemoteClientError, RequestGeneration,
};
#[cfg(target_arch = "wasm32")]
pub(crate) use smoke::install;
