//! A single, genuinely large uncompressed Chunk without consuming its full disk
//! size. This is a format/addressing/streaming test, not cold-storage throughput.
use anyhow::{Result, ensure};
use mcap::{WriteOptions, Writer, records::MessageHeader};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Seek, SeekFrom, Write},
    os::unix::fs::MetadataExt,
    path::Path,
};

struct SparseFile(File);
impl Write for SparseFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for part in bytes.chunks(64 * 1024) {
            if part.iter().all(|value| *value == 0)
                && self.0.stream_position()? >= self.0.metadata()?.len()
            {
                self.0.seek(SeekFrom::Current(part.len() as i64))?;
            } else {
                self.0.write_all(part)?;
            }
        }
        // Seek alone does not extend EOF. MCAP backpatches its Chunk header and
        // seeks to End afterwards, so the trailing hole must already count.
        let position = self.0.stream_position()?;
        if position > self.0.metadata()?.len() {
            self.0.set_len(position)?;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
impl Seek for SparseFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.seek(position)
    }
}

pub(super) fn generate(path: &str, gib: u64) -> Result<()> {
    ensure!(
        (1..=32).contains(&gib),
        "sparse Chunk size must be 1..32 GiB"
    );
    let target = gib * (1 << 30);
    let file = File::create_new(path)?;
    let mut writer = Writer::with_options(
        SparseFile(file),
        WriteOptions::new()
            .profile("ros2")
            .compression(None)
            .chunk_size(None)
            .calculate_chunk_crcs(false)
            .calculate_data_section_crc(false),
    )?;
    let schema = writer.add_schema("sensor_msgs/msg/Image", "ros2msg", super::RAW_IMAGE_SCHEMA)?;
    let channels = (0..4)
        .map(|camera| {
            writer.add_channel(
                schema,
                &format!("/camera/{camera}"),
                "cdr",
                &BTreeMap::new(),
            )
        })
        .collect::<mcap::McapResult<Vec<_>>>()?;
    let spec = super::CameraFixture {
        width: 1920,
        height: 1080,
        hz: 10,
        count: 4,
        ..Default::default()
    };
    let mut pixels = vec![0; (spec.width * spec.height * 3) as usize];
    let mut count = 0_u32;
    let mut logical = 0_u64;
    while logical < target {
        // Moving, changing central patch. Other pixel bytes remain zero/sparse.
        for y in 450..630 {
            for x in 820..1100 {
                let offset = (y * spec.width + x) as usize * 3;
                pixels[offset..offset + 3].fill(32 + ((count / 4) % 192) as u8);
            }
        }
        let time = super::BASE + u64::from(count / 4) * 100_000_000;
        let payload = super::raw_image(time, &pixels, spec);
        writer.write_to_known_channel(
            &MessageHeader {
                channel_id: channels[count as usize % 4],
                sequence: count / 4,
                log_time: time,
                publish_time: time,
            },
            &payload,
        )?;
        logical += payload.len() as u64 + 31;
        count += 1;
    }
    writer.finish()?;
    drop(writer);
    let metadata = std::fs::metadata(path)?;
    let (summary, identity) =
        viewer_mcap::FileMessageStream::open(Path::new(path))?.read_metadata()?;
    ensure!(
        summary.chunk_indexes.len() == 1 && identity.message_count == u64::from(count),
        "single Chunk fixture mismatch"
    );
    let chunk = &summary.chunk_indexes[0];
    ensure!(
        chunk.compressed_size == chunk.uncompressed_size
            && chunk
                .compressed_data_offset()?
                .checked_add(chunk.compressed_size)
                .is_some_and(|end| end <= metadata.len()),
        "sparse Chunk extends beyond file EOF"
    );
    println!(
        "{}",
        json!({"path": path, "file_bytes": metadata.len(), "allocated_bytes": metadata.blocks() * 512,
        "chunks": 1, "uncompressed_chunk_bytes": summary.chunk_indexes[0].uncompressed_size,
        "messages": count, "span_seconds": (identity.message_end_time - identity.message_start_time) as f64 / 1e9, "camera_count": 4, "camera_hz": 10, "source_size": [1920,1080],
        "note": "sparse, mostly zero synthetic rgb8; addressing/memory test, not disk throughput"})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trailing_holes_extend_eof_and_backpatches_preserve_the_end() {
        let path =
            std::env::temp_dir().join(format!("mcap-sparse-writer-{}.tmp", std::process::id()));
        let mut file = SparseFile(File::create_new(&path).unwrap());
        file.write_all(&[1; 32]).unwrap();
        file.write_all(&vec![0; 2 * 1024 * 1024]).unwrap();
        let end = file.stream_position().unwrap();
        assert_eq!(file.0.metadata().unwrap().len(), end);
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(&[0; 32]).unwrap();
        assert_eq!(file.seek(SeekFrom::End(0)).unwrap(), end);
        drop(file);
        assert!(
            std::fs::read(&path)
                .unwrap()
                .iter()
                .all(|value| *value == 0)
        );
        std::fs::remove_file(path).unwrap();
    }
}
