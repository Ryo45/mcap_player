//! Repack the same ROS messages for storage-layout experiments, with bounded streaming.
use anyhow::{Result, bail, ensure};
use mcap::{Compression, WriteOptions, Writer};
use serde_json::json;
use std::{
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
};
use viewer_mcap::FileMessageStream;

pub(super) fn rewrite(
    input: &str,
    output: &str,
    codec: &str,
    chunk: u64,
    part_seconds: u64,
) -> Result<()> {
    ensure!(
        chunk == 0 || (4096..=1024 * 1024 * 1024).contains(&chunk),
        "invalid chunk bytes"
    );
    ensure!(part_seconds <= 3600, "invalid part duration");
    let compression = match codec {
        "none" => None,
        "zstd" => Some(Compression::Zstd),
        "lz4" => Some(Compression::Lz4),
        _ => bail!("invalid compression"),
    };
    ensure!(
        chunk != 0 || compression.is_none(),
        "unchunked records cannot be compressed"
    );
    let output = Path::new(output);
    if part_seconds != 0 {
        fs::create_dir(output)?;
    }
    let mut writer = None;
    let mut paths = Vec::<PathBuf>::new();
    let mut boundary = 0;
    let mut count = 0u64;
    let mut checksum = crc32fast::Hasher::new();
    for message in FileMessageStream::open(Path::new(input))? {
        let message = message?;
        if writer.is_none() || (part_seconds != 0 && message.log_time >= boundary) {
            if let Some(mut previous) = writer.take() {
                Writer::finish(&mut previous)?;
            }
            let path = if part_seconds == 0 {
                output.to_owned()
            } else {
                output.join(format!("part-{:03}.mcap", paths.len()))
            };
            writer = Some(Writer::with_options(
                BufWriter::new(File::create_new(&path)?),
                WriteOptions::new()
                    .profile("ros2")
                    .compression(compression)
                    .compression_threads(0)
                    .compression_level(3)
                    .use_chunks(chunk != 0)
                    .chunk_size(Some(chunk.max(4096))),
            )?);
            paths.push(path);
            boundary = message
                .log_time
                .saturating_add(part_seconds * 1_000_000_000);
        }
        checksum.update(message.channel.topic.as_bytes());
        checksum.update(&message.log_time.to_le_bytes());
        checksum.update(&message.publish_time.to_le_bytes());
        checksum.update(&message.sequence.to_le_bytes());
        checksum.update(&message.data);
        writer.as_mut().unwrap().write(&message)?;
        count += 1;
    }
    if let Some(mut writer) = writer {
        writer.finish()?;
    }
    let mut facts = Vec::new();
    for path in paths {
        let (s, id) = FileMessageStream::open(&path)?.read_metadata()?;
        facts.push(json!({"path":path,"bytes":id.file_size,"messages":id.message_count,"chunks":s.chunk_indexes.len(),"max_chunk_bytes":s.chunk_indexes.iter().map(|c|c.uncompressed_size).max().unwrap_or(0),"start":id.message_start_time,"end":id.message_end_time}));
    }
    println!(
        "{}",
        json!({"input":input,"codec":codec,"chunk_bytes":chunk,"part_seconds":part_seconds,"messages":count,"logical_crc32":checksum.finalize(),"files":facts})
    );
    Ok(())
}
