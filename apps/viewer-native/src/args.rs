use crate::workspace::WorkspaceLayout;
use anyhow::{Context, Result, bail};
use std::path::PathBuf;

const DEFAULT_FIXTURE: &str = "tests/fixtures/camera-jpeg/camera_7_5s.mcap";
const DEFAULT_TOPIC: &str = "/camera/front/image/compressed";
const DEFAULT_CALIBRATION: &str = "config/camera_calibration.json";

#[derive(Debug)]
pub(crate) enum SourceMode {
    Mcap,
    #[cfg(feature = "ros2-live")]
    Ros {
        reliable: bool,
    },
}

pub(crate) struct Args {
    pub(crate) mcap: PathBuf,
    pub(crate) topic: String,
    pub(crate) calibration: PathBuf,
    pub(crate) mode: SourceMode,
    pub(crate) layout: WorkspaceLayout,
}

impl Args {
    pub(crate) fn parse() -> Result<Self> {
        let mut mcap = PathBuf::from(DEFAULT_FIXTURE);
        let mut topic = DEFAULT_TOPIC.to_owned();
        let mut calibration = PathBuf::from(DEFAULT_CALIBRATION);
        let mut live = false;
        let mut reliable = false;
        let mut layout = WorkspaceLayout::Standard;
        let mut values = std::env::args().skip(1);
        while let Some(value) = values.next() {
            match value.as_str() {
                "--mcap" => mcap = PathBuf::from(values.next().context("--mcap needs a path")?),
                "--camera-topic" => {
                    topic = values.next().context("--camera-topic needs a topic")?
                }
                "--camera-calibration" => {
                    calibration =
                        PathBuf::from(values.next().context("--camera-calibration needs a path")?)
                }
                "--layout" => {
                    layout = parse_layout(
                        &values
                            .next()
                            .context("--layout needs standard, showcase or explore")?,
                    )?
                }
                "--help" | "-h" => {
                    println!(
                        "viewer-native [--mcap PATH] [--camera-topic TOPIC] [--camera-calibration JSON] [--layout standard|showcase|explore] [--live [--reliable]]\n\nFiles can also be dropped onto the window."
                    );
                    std::process::exit(0);
                }
                "--live" => live = true,
                "--reliable" => reliable = true,
                unknown => bail!("unknown argument: {unknown}"),
            }
        }
        let mode = source_mode(live, reliable)?;
        Ok(Self {
            mcap,
            topic,
            calibration,
            mode,
            layout,
        })
    }
}

fn source_mode(live: bool, reliable: bool) -> Result<SourceMode> {
    if reliable && !live {
        bail!("--reliable requires --live");
    }
    #[cfg(feature = "ros2-live")]
    {
        Ok(if live {
            SourceMode::Ros { reliable }
        } else {
            SourceMode::Mcap
        })
    }
    #[cfg(not(feature = "ros2-live"))]
    {
        if live {
            bail!("--live requires `cargo run -p viewer-native --features ros2-live -- --live`");
        }
        Ok(SourceMode::Mcap)
    }
}

fn parse_layout(value: &str) -> Result<WorkspaceLayout> {
    match value {
        "standard" => Ok(WorkspaceLayout::Standard),
        "showcase" => Ok(WorkspaceLayout::Showcase),
        "explore" => Ok(WorkspaceLayout::Explore),
        value => bail!("unsupported layout {value:?}; expected standard, showcase or explore"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_config_accepts_the_bundled_views() {
        assert_eq!(parse_layout("standard").unwrap(), WorkspaceLayout::Standard);
        assert_eq!(parse_layout("showcase").unwrap(), WorkspaceLayout::Showcase);
        assert_eq!(parse_layout("explore").unwrap(), WorkspaceLayout::Explore);
        assert!(parse_layout("dashboard").is_err());
    }

    #[test]
    fn reliability_is_not_silently_ignored_without_live_mode() {
        assert!(source_mode(false, true).is_err());
    }
}
