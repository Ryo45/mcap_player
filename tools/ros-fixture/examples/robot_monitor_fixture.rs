//! Adds asynchronous custom variable-array messages to the RobotStatus fixture.
use anyhow::{Context, Result};
use std::{borrow::Cow, fs::File, io::BufWriter, path::PathBuf, sync::Arc};
use viewer_mcap::FileMessageStream;
const SCHEMA: &str = "uint32 frame\nmy_robot_msgs/TrackedObject[] objects\nfloat32[] timings_ms\n================================================================================\nMSG: my_robot_msgs/TrackedObject\nuint64 id\nstring label\nfloat64 confidence\nfloat64 distance\n";
fn align(data: &mut Vec<u8>, size: usize) {
    while !(data.len() - 4).is_multiple_of(size) {
        data.push(0);
    }
}
fn payload(index: u32, mode: &str) -> Vec<u8> {
    let mut data = vec![0, 1, 0, 0];
    data.extend_from_slice(&index.to_le_bytes());
    let count = match mode {
        "oversized" => 129,
        "alternate" => 1,
        _ => index % 4,
    };
    data.extend_from_slice(&count.to_le_bytes());
    for i in 0..count {
        align(&mut data, 8);
        data.extend_from_slice(&(9_007_199_254_740_993u64 + u64::from(i)).to_le_bytes());
        let label = if mode == "alternate" {
            "alternate"
        } else {
            ["person", "car", "forklift"][(i % 3) as usize]
        };
        align(&mut data, 4);
        data.extend_from_slice(&(label.len() as u32 + 1).to_le_bytes());
        data.extend_from_slice(label.as_bytes());
        data.push(0);
        align(&mut data, 8);
        data.extend_from_slice(&(0.95 - f64::from(i) * 0.05).to_le_bytes());
        data.extend_from_slice(&(2. + f64::from(i) + f64::from(index) * 0.1).to_le_bytes());
    }
    align(&mut data, 4);
    let stages = index % 3 + 1;
    data.extend_from_slice(&stages.to_le_bytes());
    for stage in 0..stages {
        data.extend_from_slice(&(1.5 + stage as f32 * 0.7).to_le_bytes());
    }
    data
}
fn main() -> Result<()> {
    let output=PathBuf::from(std::env::args().nth(1).context("usage: robot_monitor_fixture NEW_OUTPUT.mcap [alternate|malformed|oversized|missing-status|missing-tracks]")?);
    let mode = std::env::args().nth(2).unwrap_or_default();
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/plugin-panel/recording.mcap");
    let mut writer = mcap::Writer::new(BufWriter::new(
        File::options().write(true).create_new(true).open(&output)?,
    ))?;
    let mut path_index = 0;
    let mut frames = 0;
    for message in FileMessageStream::open(&source)? {
        let message = message?;
        if mode == "missing-status" && message.channel.topic == "/robot/status" {
            continue;
        }
        writer.write(&message)?;
        if message.channel.topic != "/planning/path" || mode == "missing-tracks" {
            continue;
        }
        path_index += 1;
        if path_index % 3 != 1 {
            continue;
        }
        let mut extra = message.clone();
        let mut channel = message.channel.as_ref().clone();
        channel.id = 50003;
        channel.topic = "/perception/tracks".into();
        channel.schema = Some(Arc::new(mcap::Schema {
            id: 50003,
            name: "my_robot_msgs/msg/TrackedObjects".into(),
            encoding: "ros2msg".into(),
            data: Cow::Borrowed(SCHEMA.as_bytes()),
        }));
        extra.channel = Arc::new(channel);
        extra.data = Cow::Owned(if mode == "malformed" {
            vec![0, 1, 0, 0, 0]
        } else {
            payload(frames, &mode)
        });
        extra.log_time = extra
            .log_time
            .checked_add(15_000_000)
            .context("time overflow")?;
        writer.write(&extra)?;
        frames += 1;
    }
    writer.finish()?;
    println!(
        "Created {} with {frames} custom array messages ({mode})",
        output.display()
    );
    Ok(())
}
