//! Prepare the bundled small demo for README screenshots without changing message bytes/times.
use anyhow::{Context, Result};
use std::{borrow::Cow, fs::File, io::BufWriter, path::PathBuf, sync::Arc};
use viewer_core::FileMessageStream;

const HEADER_DEFINITIONS: &str = "\n\
================================================================================\n\
MSG: std_msgs/msg/Header\n\
builtin_interfaces/Time stamp\n\
string frame_id\n\
================================================================================\n\
MSG: builtin_interfaces/msg/Time\n\
int32 sec\n\
uint32 nanosec\n";

fn main() -> Result<()> {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("usage: readme_fixture OUTPUT.mcap")?,
    );
    let input = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap");
    let file = File::options().write(true).create_new(true).open(&output)?;
    let mut writer = mcap::Writer::new(BufWriter::new(file))?;
    for message in FileMessageStream::open(&input)? {
        let mut message = message?;
        if let Some(schema) = &message.channel.schema
            && matches!(
                schema.name.as_str(),
                "sensor_msgs/msg/CompressedImage" | "sensor_msgs/msg/LaserScan"
            )
        {
            // This historical fixture has root-only schemas. These two roots need just Header/Time.
            let mut schema = schema.as_ref().clone();
            let mut definition = schema.data.to_vec();
            definition.extend_from_slice(HEADER_DEFINITIONS.as_bytes());
            schema.data = Cow::Owned(definition);
            let mut channel = message.channel.as_ref().clone();
            channel.schema = Some(Arc::new(schema));
            message.channel = Arc::new(channel);
        }
        writer.write(&message)?;
    }
    writer.finish()?;
    println!(
        "Prepared {} (original payloads and timestamps, complete Camera/Scan schemas)",
        output.display()
    );
    Ok(())
}
