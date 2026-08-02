use std::{collections::BTreeSet, net::SocketAddr, path::PathBuf};

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServerConfig {
    #[serde(default = "default_bind")]
    pub(crate) bind: SocketAddr,
    pub(crate) allowed_origins: Vec<String>,
    #[serde(default = "default_max_in_flight")]
    pub(crate) max_in_flight_requests: usize,
    #[serde(default)]
    pub(crate) limits: Limits,
    pub(crate) recordings: Vec<RecordingConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecordingConfig {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Limits {
    pub(crate) default_window_ns: u64,
    pub(crate) max_window_ns: u64,
    pub(crate) default_response_bytes: usize,
    pub(crate) max_response_bytes: usize,
    pub(crate) default_max_messages: usize,
    pub(crate) max_messages: usize,
    pub(crate) max_chunk_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            default_window_ns: 2_000_000_000,
            max_window_ns: 5_000_000_000,
            default_response_bytes: 8_388_608,
            max_response_bytes: 67_108_864,
            default_max_messages: 50_000,
            max_messages: 200_000,
            max_chunk_bytes: 134_217_728,
        }
    }
}

fn default_bind() -> SocketAddr {
    "127.0.0.1:8081".parse().unwrap()
}

fn default_max_in_flight() -> usize {
    4
}

impl ServerConfig {
    pub(crate) fn from_toml(source: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(source).map_err(|error| error.to_string())?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        if self.allowed_origins.is_empty() {
            return Err("allowed_origins must contain at least one origin".into());
        }
        for origin in &self.allowed_origins {
            if origin == "*" || !(origin.starts_with("http://") || origin.starts_with("https://")) {
                return Err(format!("invalid allowed origin: {origin}"));
            }
        }
        if self.max_in_flight_requests == 0 {
            return Err("max_in_flight_requests must be positive".into());
        }
        self.limits.validate()?;
        if self.recordings.is_empty() {
            return Err("recordings must contain at least one entry".into());
        }
        let mut ids = BTreeSet::new();
        for recording in &self.recordings {
            if !valid_recording_id(&recording.id) {
                return Err(format!("invalid recording id: {}", recording.id));
            }
            if !ids.insert(recording.id.as_str()) {
                return Err(format!("duplicate recording id: {}", recording.id));
            }
            if recording.display_name.trim().is_empty() {
                return Err(format!(
                    "recording {} has an empty display_name",
                    recording.id
                ));
            }
            if !recording.path.is_absolute() {
                return Err(format!("recording {} path must be absolute", recording.id));
            }
        }
        Ok(())
    }
}

impl Limits {
    fn validate(&self) -> Result<(), String> {
        let values = [
            ("default_window_ns", self.default_window_ns),
            ("max_window_ns", self.max_window_ns),
            ("default_response_bytes", self.default_response_bytes as u64),
            ("max_response_bytes", self.max_response_bytes as u64),
            ("default_max_messages", self.default_max_messages as u64),
            ("max_messages", self.max_messages as u64),
            ("max_chunk_bytes", self.max_chunk_bytes as u64),
        ];
        if let Some((name, _)) = values.into_iter().find(|(_, value)| *value == 0) {
            return Err(format!("{name} must be positive"));
        }
        if self.default_window_ns > self.max_window_ns
            || self.default_response_bytes > self.max_response_bytes
            || self.default_max_messages > self.max_messages
        {
            return Err("default limits must not exceed hard limits".into());
        }
        if self.max_response_bytes < 16 {
            return Err("max_response_bytes must fit the 16-byte batch header".into());
        }
        Ok(())
    }
}

pub(crate) fn valid_recording_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=128).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && id != ".."
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(path: &str) -> String {
        format!(
            r#"
allowed_origins = ["http://localhost:8080"]
[[recordings]]
id = "demo-1"
display_name = "Demo"
path = "{path}"
"#
        )
    }

    #[test]
    fn valid_config_uses_documented_defaults() {
        let parsed = ServerConfig::from_toml(&config("/tmp/demo.mcap")).unwrap();
        assert_eq!(parsed.bind, default_bind());
        assert_eq!(parsed.max_in_flight_requests, 4);
        assert_eq!(parsed.limits.max_window_ns, 5_000_000_000);
    }

    #[test]
    fn rejects_relative_duplicate_and_invalid_ids() {
        assert!(ServerConfig::from_toml(&config("relative.mcap")).is_err());
        assert!(!valid_recording_id(""));
        assert!(!valid_recording_id("../demo"));
        assert!(!valid_recording_id("demo/path"));
        assert!(!valid_recording_id(".."));

        let duplicate = format!(
            "{}\n{}",
            config("/tmp/a.mcap"),
            &config("/tmp/b.mcap")[48..]
        );
        assert!(ServerConfig::from_toml(&duplicate).is_err());
    }

    #[test]
    fn rejects_zero_limits_and_wildcard_origin() {
        let source = format!(
            "{}\n[limits]\nmax_window_ns = 0\n",
            config("/tmp/demo.mcap")
        );
        assert!(ServerConfig::from_toml(&source).is_err());
        assert!(
            ServerConfig::from_toml(
                &config("/tmp/demo.mcap").replace("http://localhost:8080", "*")
            )
            .is_err()
        );
    }
}
