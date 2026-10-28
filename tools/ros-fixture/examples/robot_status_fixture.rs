//! Unknown-to-host ROS type; its definition lives only in the MCAP schema.
use anyhow::{Context, Result};
use std::{borrow::Cow, fs::File, io::BufWriter, path::PathBuf, sync::Arc};
use viewer_mcap::FileMessageStream;
const SCHEMA: &str = "float64 battery\nfloat64 temperature\nstring state\nuint64 sequence\nbool enabled\nuint8[] unused\n";
fn payload(index: u64, alternate: bool) -> Vec<u8> {
    let mut data = vec![0, 1, 0, 0];
    data.extend_from_slice(
        &(if alternate { 25. } else { 90. } - (index as f64) * 0.25).to_le_bytes(),
    );
    data.extend_from_slice(&(30. + (index as f64 * 0.2).sin() * 5.).to_le_bytes());
    let state = if alternate {
        "charging\0"
    } else {
        "navigating\0"
    };
    data.extend_from_slice(&(state.len() as u32).to_le_bytes());
    data.extend_from_slice(state.as_bytes());
    while !(data.len() - 4).is_multiple_of(8) {
        data.push(0);
    }
    data.extend_from_slice(&(9_007_199_254_740_993 + index).to_le_bytes());
    data.push(1);
    while !(data.len() - 4).is_multiple_of(4) {
        data.push(0);
    }
    data.extend_from_slice(&4096u32.to_le_bytes());
    data.resize(data.len() + 4096, 7);
    data
}
fn main() -> Result<()> {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("usage: robot_status_fixture NEW_OUTPUT.mcap [alternate|malformed]")?,
    );
    let mode = std::env::args().nth(2).unwrap_or_default();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap");
    let mut writer = mcap::Writer::new(BufWriter::new(
        File::options().write(true).create_new(true).open(&output)?,
    ))?;
    let mut index = 0;
    for message in FileMessageStream::open(&source)? {
        let message = message?;
        writer.write(&message)?;
        if message.channel.topic != "/planning/path" {
            continue;
        }
        let mut extra = message.clone();
        let mut channel = message.channel.as_ref().clone();
        channel.id = 50002;
        channel.topic = "/robot/status".into();
        channel.schema = Some(Arc::new(mcap::Schema {
            id: 50002,
            name: "my_robot_msgs/msg/RobotStatus".into(),
            encoding: "ros2msg".into(),
            data: Cow::Borrowed(SCHEMA.as_bytes()),
        }));
        extra.channel = Arc::new(channel);
        extra.data = Cow::Owned(if mode == "malformed" {
            vec![0, 1, 0, 0, 0]
        } else {
            payload(index, mode == "alternate")
        });
        extra.log_time = extra
            .log_time
            .checked_add(5_000_000)
            .context("time overflow")?;
        writer.write(&extra)?;
        index += 1;
    }
    writer.finish()?;
    println!(
        "Created {}: {index} custom messages ({mode})",
        output.display()
    );
    Ok(())
}
