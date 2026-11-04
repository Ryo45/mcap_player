//! Small indexed recording for the Native fixed-range Query panel (no ROS installation).
use std::{collections::BTreeMap, fs::OpenOptions, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: query_fixture OUTPUT.mcap")?,
    );
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut writer = mcap::Writer::with_options(
        file,
        mcap::WriteOptions::new()
            .compression(None)
            .chunk_size(Some(8192)),
    )?;
    let schema = writer.add_schema(
        "example_msgs/msg/RobotMetric",
        "ros2msg",
        b"float64 value\nstring status\n",
    )?;
    let speed = writer.add_channel(schema, "/robot/speed", "cdr", &BTreeMap::new())?;
    let temperature = writer.add_channel(schema, "/robot/temperature", "cdr", &BTreeMap::new())?;
    for i in 0..300u32 {
        for (channel, value, status) in [
            (speed, (f64::from(i) * 0.1).sin(), "nominal"),
            (temperature, 30. + f64::from(i) * 0.02, "warming"),
        ] {
            let mut data = vec![0, 1, 0, 0];
            data.extend(value.to_le_bytes());
            data.extend(((status.len() + 1) as u32).to_le_bytes());
            data.extend(status.as_bytes());
            data.push(0);
            writer.write_to_known_channel(
                &mcap::records::MessageHeader {
                    channel_id: channel,
                    sequence: i,
                    log_time: 1_700_000_000_000_000_000 + u64::from(i) * 100_000_000,
                    publish_time: u64::from(i) * 100_000_000,
                },
                &data,
            )?;
        }
    }
    writer.finish()?;
    Ok(())
}
