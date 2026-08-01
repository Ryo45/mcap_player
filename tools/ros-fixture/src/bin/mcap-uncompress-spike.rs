//! One-off preprocessing tool for the Web range-read spike.
//!
//! This is deliberately a concrete file-to-file utility, not a production storage abstraction.

use anyhow::{Context, Result, bail};
use mcap::{MessageStream, Summary, WriteOptions, Writer, read};
use memmap2::Mmap;
use std::{
    env,
    fs::{self, OpenOptions},
    io::BufWriter,
    path::{Path, PathBuf},
};

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1).map(PathBuf::from);
    let input = args.next().context("missing input MCAP path")?;
    let output = args.next().context("missing output MCAP path")?;
    if args.next().is_some() {
        bail!("usage: mcap-uncompress-spike INPUT.mcap OUTPUT.mcap");
    }
    if input == output {
        bail!("input and output paths must differ");
    }
    if output.exists() {
        bail!("refusing to overwrite {}", output.display());
    }

    let input_file =
        fs::File::open(&input).with_context(|| format!("open input {}", input.display()))?;
    // SAFETY: the input file remains open and is never modified for the lifetime of this mapping.
    let mapped = unsafe { Mmap::map(&input_file) }
        .with_context(|| format!("map input {}", input.display()))?;
    let source_summary = Summary::read(&mapped)?
        .with_context(|| format!("{} has no Summary section", input.display()))?;
    let source_stats = source_summary
        .stats
        .as_ref()
        .context("source Summary has no Statistics record")?;

    let output_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .with_context(|| format!("create output {}", output.display()))?;
    let options = WriteOptions::new()
        .profile("ros2")
        .library("mcap-player range-read spike uncompressor")
        .compression(None);
    let mut writer = Writer::with_options(BufWriter::new(output_file), options)?;
    let mut messages = 0_u64;
    for message in MessageStream::new(&mapped)? {
        writer.write(&message?)?;
        messages += 1;
        if messages.is_multiple_of(10_000) {
            eprintln!("rewrote {messages}/{} messages", source_stats.message_count);
        }
    }
    for index in &source_summary.attachment_indexes {
        writer.attach(&read::attachment(&mapped, index)?)?;
    }
    for index in &source_summary.metadata_indexes {
        writer.write_metadata(&read::metadata(&mapped, index)?)?;
    }
    writer.finish()?;
    drop(mapped);
    drop(input_file);

    validate_output(&output, &source_summary, messages)?;
    println!(
        "wrote {} messages to {} ({} bytes)",
        messages,
        output.display(),
        output.metadata()?.len()
    );
    Ok(())
}

fn validate_output(path: &Path, source: &Summary, rewritten_messages: u64) -> Result<()> {
    let file = fs::File::open(path).with_context(|| format!("open output {}", path.display()))?;
    // SAFETY: the completed output file remains open and is not modified while this mapping lives.
    let mapped =
        unsafe { Mmap::map(&file) }.with_context(|| format!("map output {}", path.display()))?;
    let output = Summary::read(&mapped)?
        .with_context(|| format!("{} has no Summary section", path.display()))?;
    let source_stats = source.stats.as_ref().context("source Statistics missing")?;
    let output_stats = output.stats.as_ref().context("output Statistics missing")?;
    if rewritten_messages != source_stats.message_count
        || output_stats.message_count != source_stats.message_count
        || output_stats.schema_count != source_stats.schema_count
        || output_stats.channel_count != source_stats.channel_count
        || output.metadata_indexes.len() != source.metadata_indexes.len()
        || output.attachment_indexes.len() != source.attachment_indexes.len()
    {
        bail!(
            "output validation failed: source messages/schemas/channels/metadata/attachments={}/{}/{}/{}/{}, output={}/{}/{}/{}/{}",
            source_stats.message_count,
            source_stats.schema_count,
            source_stats.channel_count,
            source.metadata_indexes.len(),
            source.attachment_indexes.len(),
            output_stats.message_count,
            output_stats.schema_count,
            output_stats.channel_count,
            output.metadata_indexes.len(),
            output.attachment_indexes.len(),
        );
    }
    if output
        .chunk_indexes
        .iter()
        .any(|chunk| !chunk.compression.is_empty())
    {
        bail!("output validation failed: at least one Chunk is still compressed");
    }
    Ok(())
}
