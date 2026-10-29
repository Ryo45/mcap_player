//! Same payloads/indexed recording, different semantic topic names. Cameras are unchanged.
use anyhow::{Context, Result};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::Arc,
};
use viewer_core::WorkspaceBindings;
use viewer_mcap::FileMessageStream;
fn main() -> Result<()> {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("usage: workspace_bindings_fixture NEW_OUTPUT.mcap")?,
    );
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let defaults =
        WorkspaceBindings::from_json(include_str!("../../../config/workspace_bindings.json"))
            .map_err(anyhow::Error::msg)?;
    let mut custom = defaults.clone();
    custom.path_topic = format!("/robot{}", defaults.path_topic);
    custom.odometry_topic = format!("/robot{}", defaults.odometry_topic);
    custom.point_cloud_topic = format!("/robot{}", defaults.point_cloud_topic);
    custom.dynamic_tf_topic = format!("/robot{}", defaults.dynamic_tf_topic);
    custom.static_tf_topic = format!("/robot{}", defaults.static_tf_topic);
    let pairs = [
        (&defaults.path_topic, &custom.path_topic),
        (&defaults.odometry_topic, &custom.odometry_topic),
        (&defaults.point_cloud_topic, &custom.point_cloud_topic),
        (&defaults.dynamic_tf_topic, &custom.dynamic_tf_topic),
        (&defaults.static_tf_topic, &custom.static_tf_topic),
    ];
    let mut writer = mcap::Writer::new(BufWriter::new(
        File::options().write(true).create_new(true).open(&output)?,
    ))?;
    for message in
        FileMessageStream::open(&root.join("tests/fixtures/camera-jpeg/camera_7_5s.mcap"))?
    {
        let mut message = message?;
        if let Some((_, topic)) = pairs
            .iter()
            .find(|(from, _)| **from == message.channel.topic)
        {
            let mut channel = message.channel.as_ref().clone();
            channel.topic = (*topic).clone();
            message.channel = Arc::new(channel);
        }
        writer.write(&message)?;
    }
    writer.finish()?;
    let json = output.with_file_name("bindings.json");
    File::options()
        .write(true)
        .create_new(true)
        .open(&json)?
        .write_all(serde_json::to_string_pretty(&custom)?.as_bytes())?;
    println!("Created {} and {}", output.display(), json.display());
    Ok(())
}
