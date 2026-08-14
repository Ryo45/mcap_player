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
                            .context("--layout needs standard or showcase")?,
                    )?
                }
                "--help" | "-h" => {
                    println!(
                        "viewer-native [--mcap PATH] [--camera-topic TOPIC] [--camera-calibration JSON] [--layout standard|showcase] [--live [--reliable]]\n\nFiles can also be dropped onto the window."
                    );
                    std::process::exit(0);
                }
                "--live" => live = true,
                "--reliable" => reliable = true,
                unknown => bail!("unknown argument: {unknown}"),
            }
        }
        #[cfg(feature = "ros2-live")]
        let mode = if live {
            SourceMode::Ros { reliable }
        } else {
            SourceMode::Mcap
        };
        #[cfg(not(feature = "ros2-live"))]
        let mode = {
            if live || reliable {
                bail!(
                    "--live requires `cargo run -p viewer-native --features ros2-live -- --live`"
                );
            }
            SourceMode::Mcap
        };
        Ok(Self {
            mcap,
            topic,
            calibration,
            mode,
            layout,
        })
    }
}

fn parse_layout(value: &str) -> Result<WorkspaceLayout> {
    match value {
        "standard" => Ok(WorkspaceLayout::Standard),
        "showcase" => Ok(WorkspaceLayout::Showcase),
        value => bail!("unsupported layout {value:?}; expected standard or showcase"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_config_accepts_only_the_two_bundled_views() {
        assert_eq!(parse_layout("standard").unwrap(), WorkspaceLayout::Standard);
        assert_eq!(parse_layout("showcase").unwrap(), WorkspaceLayout::Showcase);
        assert!(parse_layout("dashboard").is_err());
    }
}
