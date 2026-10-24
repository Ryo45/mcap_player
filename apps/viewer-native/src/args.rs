use crate::workspace::WorkspaceLayout;
use anyhow::{Context, Result, bail};
use std::{collections::BTreeSet, path::PathBuf};

const DEFAULT_FIXTURE: &str = "tests/fixtures/camera-jpeg/camera_7_5s.mcap";
const DEFAULT_TOPIC: &str = "/camera/front/image/compressed";
const DEFAULT_CALIBRATION: &str = "config/camera_calibration.json";

#[derive(Debug)]
pub(crate) enum SourceMode {
    Mcap,
    #[cfg(feature = "ros2-live")]
    Ros {
        reliable: bool,
        raw_camera_topics: BTreeSet<String>,
    },
}

pub(crate) struct Args {
    pub(crate) mcap: PathBuf,
    pub(crate) topic: String,
    pub(crate) calibration: PathBuf,
    pub(crate) platform_geometry: Option<PathBuf>,
    pub(crate) ground_frame: Option<String>,
    pub(crate) fixed_frame: String,
    pub(crate) mode: SourceMode,
    pub(crate) layout: WorkspaceLayout,
    pub(crate) inspect_topic: Option<String>,
    pub(crate) point_cloud_topic: Option<String>,
}

impl Args {
    pub(crate) fn parse() -> Result<Self> {
        Self::parse_from(std::env::args().skip(1))
    }

    fn parse_from(values: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut mcap = PathBuf::from(DEFAULT_FIXTURE);
        let mut topic = DEFAULT_TOPIC.to_owned();
        let mut explicit_camera_topic = false;
        let mut raw_camera_topics = BTreeSet::new();
        let mut calibration = PathBuf::from(DEFAULT_CALIBRATION);
        let mut platform_geometry = Some(PathBuf::from(
            "config/platform_geometry/turtlebot3_waffle_pi.json",
        ));
        let mut ground_frame = None;
        let mut fixed_frame = "odom".to_owned();
        let mut live = false;
        let mut reliable = false;
        let mut layout = WorkspaceLayout::Standard;
        let mut inspect_topic = None;
        let mut point_cloud_topic = None;
        let mut values = values.into_iter();
        while let Some(value) = values.next() {
            match value.as_str() {
                "--mcap" => mcap = PathBuf::from(values.next().context("--mcap needs a path")?),
                "--camera-topic" => {
                    topic = values.next().context("--camera-topic needs a topic")?;
                    explicit_camera_topic = true;
                }
                "--live-raw-camera-topic" => {
                    let topic = values
                        .next()
                        .context("--live-raw-camera-topic needs a topic")?;
                    if topic.trim().is_empty() || topic.len() > 1024 {
                        bail!("--live-raw-camera-topic must be nonempty and at most 1024 bytes");
                    }
                    raw_camera_topics.insert(topic);
                    if raw_camera_topics.len() > 32 {
                        bail!("at most 32 Raw Live camera topics are supported");
                    }
                }
                "--point-cloud-topic" => {
                    let topic = values.next().context("--point-cloud-topic needs a topic")?;
                    if topic.trim().is_empty() || topic.len() > 1024 {
                        bail!("--point-cloud-topic must be nonempty and at most 1024 bytes");
                    }
                    point_cloud_topic = Some(topic);
                }
                "--inspect-topic" => {
                    let topic = values.next().context("--inspect-topic needs a topic")?;
                    if topic.trim().is_empty() {
                        bail!("--inspect-topic needs a non-empty topic");
                    }
                    inspect_topic = Some(topic);
                }
                "--platform-geometry" => {
                    platform_geometry = Some(PathBuf::from(
                        values
                            .next()
                            .context("--platform-geometry needs a JSON path")?,
                    ))
                }
                "--no-platform-geometry" => platform_geometry = None,
                "--fixed-frame" => {
                    fixed_frame = values.next().context("--fixed-frame needs a TF frame")?
                }
                "--ground-frame" => {
                    ground_frame = Some(values.next().context("--ground-frame needs a TF frame")?)
                }
                "--camera-calibration" => {
                    calibration =
                        PathBuf::from(values.next().context("--camera-calibration needs a path")?)
                }
                "--layout" => {
                    layout = parse_layout(&values.next().context(
                        "--layout needs standard, showcase, explore, monitor, health, scene, cameras, navigation or inspection",
                    )?)?
                }
                "--help" | "-h" => {
                    println!(
                        "viewer-native [--mcap FILE_OR_DIRECTORY] [--camera-topic TOPIC] [--camera-calibration JSON] [--platform-geometry JSON | --no-platform-geometry] [--fixed-frame FRAME] [--ground-frame FRAME] [--layout standard|showcase|explore|monitor|health|scene|cameras|navigation|inspection] [--inspect-topic TOPIC] [--point-cloud-topic TOPIC] [--live [--reliable] [--live-raw-camera-topic TOPIC]...]\n\nFiles or directories containing indexed MCAP parts can also be dropped onto the window. --inspect-topic selects an initial recording Inspector query."
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
        if fixed_frame.trim().is_empty()
            || fixed_frame.len() > 256
            || ground_frame
                .as_ref()
                .is_some_and(|f| f.trim().is_empty() || f.len() > 256)
        {
            bail!("world/ground frame names must be nonempty and at most 256 bytes");
        }
        if !explicit_camera_topic && let Some(first) = raw_camera_topics.first() {
            topic = first.clone();
        }
        let mode = source_mode(live, reliable, raw_camera_topics)?;
        Ok(Self {
            mcap,
            topic,
            calibration,
            platform_geometry,
            ground_frame,
            fixed_frame,
            mode,
            layout,
            inspect_topic,
            point_cloud_topic,
        })
    }
}

fn source_mode(
    live: bool,
    reliable: bool,
    raw_camera_topics: BTreeSet<String>,
) -> Result<SourceMode> {
    if !live && !raw_camera_topics.is_empty() {
        bail!("--live-raw-camera-topic requires --live");
    }
    if reliable && !live {
        bail!("--reliable requires --live");
    }
    #[cfg(feature = "ros2-live")]
    {
        Ok(if live {
            SourceMode::Ros {
                reliable,
                raw_camera_topics,
            }
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
        "health" => Ok(WorkspaceLayout::Health),
        "scene" => Ok(WorkspaceLayout::Scene),
        "cameras" => Ok(WorkspaceLayout::Cameras),
        "navigation" => Ok(WorkspaceLayout::Navigation),
        "inspection" => Ok(WorkspaceLayout::Inspection),
        value => {
            bail!(
                "unsupported layout {value:?}; expected standard, showcase, explore, monitor, health, scene, cameras, navigation or inspection"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_live_topics_are_explicit_bounded_and_live_only() {
        let parse = |values: &[&str]| Args::parse_from(values.iter().map(|s| (*s).to_owned()));
        for args in [
            vec!["--live-raw-camera-topic"],
            vec!["--live-raw-camera-topic", " "],
            vec!["--live-raw-camera-topic", "/raw"],
        ] {
            assert!(parse(&args).is_err());
        }
        assert!(
            Args::parse_from([
                "--live".into(),
                "--live-raw-camera-topic".into(),
                "x".repeat(1025)
            ])
            .is_err()
        );
        #[cfg(feature = "ros2-live")]
        {
            let args = parse(&[
                "--live",
                "--live-raw-camera-topic",
                "/raw_b",
                "--live-raw-camera-topic",
                "/raw_a",
                "--live-raw-camera-topic",
                "/raw_a",
            ])
            .unwrap();
            assert_eq!(args.topic, "/raw_a");
            let SourceMode::Ros {
                raw_camera_topics, ..
            } = args.mode
            else {
                panic!("expected ROS mode")
            };
            assert_eq!(raw_camera_topics.len(), 2);
            let explicit = parse(&[
                "--live-raw-camera-topic",
                "/raw",
                "--camera-topic",
                "/compressed",
                "--live",
            ])
            .unwrap();
            assert_eq!(explicit.topic, "/compressed");
        }
    }

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
    fn point_cloud_selection_requires_a_nonempty_bounded_topic() {
        let parse = |values: &[&str]| Args::parse_from(values.iter().map(|v| (*v).to_owned()));
        assert_eq!(
            parse(&["--layout", "scene", "--point-cloud-topic", "/points"])
                .unwrap()
                .point_cloud_topic
                .as_deref(),
            Some("/points")
        );
        assert!(parse(&["--point-cloud-topic"]).is_err());
        assert!(parse(&["--point-cloud-topic", " "]).is_err());
        assert!(Args::parse_from(["--point-cloud-topic".to_owned(), "x".repeat(1025)]).is_err());
    }

    #[test]
    fn layout_config_accepts_the_bundled_views() {
        assert_eq!(parse_layout("standard").unwrap(), WorkspaceLayout::Standard);
        assert_eq!(parse_layout("showcase").unwrap(), WorkspaceLayout::Showcase);
        assert_eq!(parse_layout("explore").unwrap(), WorkspaceLayout::Explore);
        assert_eq!(parse_layout("monitor").unwrap(), WorkspaceLayout::Monitor);
        assert_eq!(parse_layout("health").unwrap(), WorkspaceLayout::Health);
        assert_eq!(parse_layout("scene").unwrap(), WorkspaceLayout::Scene);
        assert!(parse_layout("dashboard").is_err());
    }

    #[test]
    fn reliability_is_not_silently_ignored_without_live_mode() {
        assert!(source_mode(false, true, BTreeSet::new()).is_err());
    }
}
