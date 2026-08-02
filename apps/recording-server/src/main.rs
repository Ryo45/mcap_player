mod api;
mod batch_service;
mod config;
mod cursor;
mod error;
mod file_reader;
mod metrics;
mod recording;

use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use api::AppState;
use config::ServerConfig;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let (config_path, validate_only) = parse_args()?;
    let source = fs::read_to_string(&config_path)
        .with_context(|| format!("could not read config {}", config_path.display()))?;
    let config = ServerConfig::from_toml(&source).map_err(anyhow::Error::msg)?;
    let state = AppState::initialize(&config).map_err(anyhow::Error::new)?;
    if validate_only {
        println!(
            "configuration valid: {} recording(s)",
            config.recordings.len()
        );
        return Ok(());
    }
    let app = api::router(&config, state).map_err(anyhow::Error::msg)?;
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("could not bind {}", config.bind))?;
    tracing::info!(bind = %config.bind, recording_count = config.recordings.len(), "recording server ready");
    axum::serve(listener, app)
        .await
        .context("HTTP server failed")?;
    Ok(())
}

fn parse_args() -> Result<(PathBuf, bool)> {
    let mut arguments = env::args_os().skip(1);
    let mut config = None;
    let mut validate_only = false;
    while let Some(argument) = arguments.next() {
        if argument == "--config" {
            let value = arguments
                .next()
                .ok_or_else(|| anyhow::anyhow!("--config requires a path"))?;
            config = Some(PathBuf::from(value));
        } else if argument == "--validate-only" {
            validate_only = true;
        } else {
            bail!("unknown argument: {}", argument.to_string_lossy());
        }
    }
    let config = config.ok_or_else(|| {
        anyhow::anyhow!("usage: recording-server --config PATH [--validate-only]")
    })?;
    Ok((config, validate_only))
}
