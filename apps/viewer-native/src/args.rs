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
    pub(crate) inspect_topic: Option<String>,
}

impl Args {
    pub(crate) fn parse() -> Result<Self> {
        Self::parse_from(std::env::args().skip(1))
    }

    fn parse_from(values: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut mcap = PathBuf::from(DEFAULT_FIXTURE);
        let mut topic = DEFAULT_TOPIC.to_owned();
        let mut calibration = PathBuf::from(DEFAULT_CALIBRATION);
        let mut live = false;
        let mut reliable = false;
        let mut layout = WorkspaceLayout::Standard;
        let mut inspect_topic = None;
        let mut values = values.into_iter();
        while let Some(value) = values.next() {
            match value.as_str() {
                "--mcap" => mcap = PathBuf::from(values.next().context("--mcap needs a path")?),
                "--camera-topic" => {
                    topic = values.next().context("--camera-topic needs a topic")?
                }
                "--inspect-topic" => {
                    let topic = values.next().context("--inspect-topic needs a topic")?;
                    if topic.trim().is_empty() {
                        bail!("--inspect-topic needs a non-empty topic");
                    }
                    inspect_topic = Some(topic);
                }
                "--camera-calibration" => {
                    calibration =
                        PathBuf::from(values.next().context("--camera-calibration needs a path")?)
                }
                "--layout" => {
                    layout = parse_layout(
                        &values
                            .next()
                            .context("--layout needs standard, showcase, explore or monitor")?,
                    )?
                }
                "--help" | "-h" => {
                    println!(
                        "viewer-native [--mcap PATH] [--camera-topic TOPIC] [--camera-calibration JSON] [--layout standard|showcase|explore|monitor] [--inspect-topic TOPIC] [--live [--reliable]]\n\nFiles can also be dropped onto the window. --inspect-topic selects an initial recording Inspector query."
                    );
                    std::process::exit(0);
                }
                "--live" => live = true,
                "--reliable" => reliable = true,
                unknown => bail!("unknown argument: {unknown}"),
            }
        }
        if live && inspect_topic.is_some() {
            bail!("--inspect-topic requires recording input; live Inspector is unavailable");
        }
        let mode = source_mode(live, reliable)?;
        Ok(Self {
            mcap,
            topic,
            calibration,
            mode,
            layout,
            inspect_topic,
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
        "monitor" => Ok(WorkspaceLayout::Monitor),
        value => {
            bail!("unsupported layout {value:?}; expected standard, showcase, explore or monitor")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_inspector_selection_is_recording_only_and_requires_a_topic() {
        let parse = |values: &[&str]| Args::parse_from(values.iter().map(|s| (*s).to_owned()));
        let args = parse(&["--layout", "monitor", "--inspect-topic", "/scan"]).unwrap();
        assert_eq!(args.layout, WorkspaceLayout::Monitor);
        assert_eq!(args.inspect_topic.as_deref(), Some("/scan"));
        assert!(parse(&["--inspect-topic"]).is_err());
        assert!(parse(&["--inspect-topic", " "]).is_err());
        assert!(parse(&["--live", "--inspect-topic", "/scan"]).is_err());
    }

    #[test]
    fn layout_config_accepts_the_bundled_views() {
        assert_eq!(parse_layout("standard").unwrap(), WorkspaceLayout::Standard);
        assert_eq!(parse_layout("showcase").unwrap(), WorkspaceLayout::Showcase);
        assert_eq!(parse_layout("explore").unwrap(), WorkspaceLayout::Explore);
        assert_eq!(parse_layout("monitor").unwrap(), WorkspaceLayout::Monitor);
        assert!(parse_layout("dashboard").is_err());
    }

    #[test]
    fn reliability_is_not_silently_ignored_without_live_mode() {
        assert!(source_mode(false, true).is_err());
    }
}
