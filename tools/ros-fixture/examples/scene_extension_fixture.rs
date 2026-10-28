//! Add an independent, physically offset Path input to the bundled Scene fixture.
use anyhow::{Context, Result};
use std::{borrow::Cow, fs::File, io::BufWriter, path::PathBuf, sync::Arc};
use viewer_core::{decode_path, encode_path_cdr};
use viewer_mcap::FileMessageStream;

fn main() -> Result<()> {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("usage: scene_extension_fixture OUTPUT.mcap (must not exist)")?,
    );
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap");
    let mut writer = mcap::Writer::new(BufWriter::new(
        File::options().write(true).create_new(true).open(&output)?,
    ))?;
    let mut inputs = 0;
    let mut path_messages = 0;
    for message in FileMessageStream::open(&source)? {
        let message = message?;
        writer.write(&message)?;
        if message.channel.topic == "/planning/path" {
            path_messages += 1;
            if path_messages % 2 == 0 {
                continue;
            }
            let mut input = decode_path(&message.data)?;
            // Deliberately separate from the displayed trajectory: eight objects 2 m to its left.
            input.points = input
                .points
                .iter()
                .step_by(4)
                .take(8)
                .map(|[x, y, z]| [*x, y + 2., *z])
                .collect();
            let mut extra = message.clone();
            let mut channel = message.channel.as_ref().clone();
            channel.id = 50_000;
            channel.topic = "/perception/objects".into();
            extra.channel = Arc::new(channel);
            // Independent publication cadence and log time; header/measurement time is preserved.
            extra.log_time = message
                .log_time
                .checked_add(5_000_000)
                .context("log-time overflow")?;
            extra.data = Cow::Owned(encode_path_cdr(&input)?);
            writer.write(&extra)?;
            inputs += 1;
        }
    }
    writer.finish()?;
    println!(
        "Created {} with {inputs} /perception/objects Path messages; /planning/path unchanged",
        output.display()
    );
    Ok(())
}
