//! One physical Chunk being prepared for the next forward traversal.
use super::{McapSource, McapSourceError, ParsedChunkMessage};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct ChunkPrefetch {
    pub index: usize,
    cancelled: Arc<AtomicBool>,
    task: Option<std::thread::JoinHandle<Result<Arc<[ParsedChunkMessage]>, McapSourceError>>>,
}
impl ChunkPrefetch {
    pub fn start(source: McapSource, index: usize) -> Option<Self> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let task = std::thread::Builder::new()
            .name("mcap-chunk".into())
            .spawn(move || {
                source.parse_chunk_cancellable(index, &mut || signal.load(Ordering::Relaxed))
            })
            .ok()?;
        Some(Self {
            index,
            cancelled,
            task: Some(task),
        })
    }
    pub fn is_ready(&self) -> bool {
        self.task.as_ref().is_some_and(|task| task.is_finished())
    }
    pub fn finish(mut self) -> Result<Arc<[ParsedChunkMessage]>, McapSourceError> {
        self.task
            .take()
            .expect("unconsumed prefetch task")
            .join()
            .map_err(|_| McapSourceError::Io(std::io::Error::other("MCAP chunk worker panicked")))?
    }
}
impl Drop for ChunkPrefetch {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        // Do not leave a retired source's decompression running alongside its replacement.
        if let Some(task) = self.task.take() {
            let _ = task.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use mcap::{WriteOptions, Writer, records::MessageHeader};
    use std::{
        collections::BTreeMap,
        io::Cursor,
        time::{Duration, Instant},
    };
    use viewer_core::{ArrivalTime, StreamId};

    fn recording() -> McapSource {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = Writer::with_options(
                &mut bytes,
                WriteOptions::new().compression(None).chunk_size(None),
            )
            .unwrap();
            let schema = writer
                .add_schema("demo/msg/Sample", "ros2msg", b"uint8 value")
                .unwrap();
            let a = writer
                .add_channel(schema, "/a", "cdr", &BTreeMap::new())
                .unwrap();
            let b = writer
                .add_channel(schema, "/b", "cdr", &BTreeMap::new())
                .unwrap();
            for (channel, times) in [(a, [0, 10]), (b, [10, 20]), (a, [30, 40])] {
                for time in times {
                    writer
                        .write_to_known_channel(
                            &MessageHeader {
                                channel_id: channel,
                                sequence: time as u32,
                                log_time: time,
                                publish_time: time,
                            },
                            &[time as u8],
                        )
                        .unwrap();
                }
                writer.flush().unwrap();
            }
            writer.finish().unwrap();
        }
        let mut source = McapSource::from_bytes(Bytes::from(bytes.into_inner())).unwrap();
        source.select_streams([StreamId(1)]).unwrap();
        source
    }

    fn wait_prepared(source: &McapSource) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !source.prefetch.lock().unwrap().as_ref().unwrap().is_ready() {
            assert!(Instant::now() < deadline, "prefetch worker did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn selected_next_chunk_is_prepared_once_and_delivered_in_order() {
        let mut source = recording();
        assert_eq!(
            source.read_through(ArrivalTime(0)).unwrap()[0].arrival_time,
            ArrivalTime(0)
        );
        source.prefetch_next();
        wait_prepared(&source);
        assert_eq!(source.read_diagnostics().chunks_expanded, 2);
        assert_eq!(
            source.read_diagnostics().prefetch_estimated_bytes,
            source.summary.chunk_indexes[2].uncompressed_size
        );
        let messages = source.read_through(ArrivalTime(40)).unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|m| m.arrival_time.0)
                .collect::<Vec<_>>(),
            [10, 30, 40]
        );
        let facts = source.read_diagnostics();
        assert_eq!(facts.chunks_expanded, 2); // /b's Chunk was pruned; no duplicate expansion.
        assert_eq!(facts.prefetch_ready_hits, 1);
        assert_eq!(facts.sequential_cache_backing_bytes, 0);
        assert!(source.prefetch.lock().unwrap().is_none());
    }

    #[test]
    fn paused_playback_neither_starts_nor_retains_read_ahead() {
        let source = recording();
        let super::super::super::backing::Backing::Bytes(bytes) = &source.backing else {
            unreachable!()
        };
        let mut playback = crate::McapPlayback::from_bytes(bytes.clone()).unwrap();
        playback.select_streams([StreamId(1)]).unwrap();
        playback.tick(Duration::ZERO, |_, _| {}).unwrap();
        assert_eq!(
            playback
                .recording_source()
                .read_diagnostics()
                .chunks_expanded,
            1
        );
        playback.clock_mut().toggle();
        playback.tick(Duration::ZERO, |_, _| {}).unwrap();
        wait_prepared(playback.recording_source());
        assert!(
            playback
                .recording_source()
                .read_diagnostics()
                .prefetch_estimated_bytes
                > 0
        );
        playback.clock_mut().toggle();
        playback.tick(Duration::ZERO, |_, _| {}).unwrap();
        assert_eq!(
            playback
                .recording_source()
                .read_diagnostics()
                .prefetch_estimated_bytes,
            0
        );
    }

    #[test]
    fn changing_selection_discards_any_old_preparation() {
        let mut source = recording();
        source.read_through(ArrivalTime(0)).unwrap();
        source.prefetch_next();
        source.select_streams([StreamId(2)]).unwrap();
        assert!(source.prefetch.lock().unwrap().is_none());
        let messages = source.read_through(ArrivalTime(40)).unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|m| (m.stream_id.0, m.arrival_time.0))
                .collect::<Vec<_>>(),
            [(2, 10), (2, 20)]
        );
    }

    #[test]
    fn failed_preparation_does_not_consume_committed_messages() {
        let mut source = recording();
        source.read_through(ArrivalTime(0)).unwrap();
        let chunk = &source.summary.chunk_indexes[2];
        let offset = chunk.compressed_data_offset().unwrap() as usize;
        if let super::super::super::backing::Backing::Bytes(bytes) = &mut source.backing {
            let mut corrupt = bytes.to_vec();
            corrupt[offset + chunk.compressed_size as usize / 2] ^= 1;
            *bytes = Bytes::from(corrupt);
        }
        source.prefetch_next();
        wait_prepared(&source);
        let next = source.forward.next;
        assert!(source.read_through(ArrivalTime(40)).is_err());
        assert_eq!(source.forward.next, next);
        assert_eq!(
            source.read_through(ArrivalTime(10)).unwrap()[0].arrival_time,
            ArrivalTime(10)
        );
    }

    #[test]
    fn changed_file_is_rejected_before_prepared_messages_commit() {
        use std::io::Write;
        let original = recording();
        let super::super::super::backing::Backing::Bytes(bytes) = &original.backing else {
            unreachable!()
        };
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("mcap-prefetch-{}-{nonce}.mcap", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let mut source = McapSource::open_file(&path).unwrap();
        source.select_streams([StreamId(1)]).unwrap();
        source.read_through(ArrivalTime(0)).unwrap();
        source.prefetch_next();
        wait_prepared(&source);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[0])
            .unwrap();
        let position = source.forward.next;
        assert!(matches!(
            source.read_through(ArrivalTime(40)),
            Err(McapSourceError::SourceChanged)
        ));
        assert_eq!(source.forward.next, position);
        drop(source);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn preparation_accounts_for_both_compressed_and_expanded_bytes() {
        let mut source = recording();
        source.read_through(ArrivalTime(0)).unwrap();
        let chunk = &mut Arc::make_mut(&mut source.summary).chunk_indexes[2];
        chunk.compression = "zstd".into();
        chunk.compressed_size = 2 * 1024 * 1024 * 1024;
        chunk.uncompressed_size = chunk.compressed_size;
        source.prefetch_next();
        assert!(source.prefetch.lock().unwrap().is_none());
        assert_eq!(source.read_diagnostics().chunks_expanded, 1);
    }

    #[test]
    fn restore_and_forward_position_share_one_physical_expansion() {
        let source = recording();
        let reader = source.restore_reader();
        let latest = reader
            .latest_before(&[StreamId(1)], ArrivalTime(5))
            .unwrap();
        let position = reader.prepare_seek(ArrivalTime(6)).unwrap();
        assert_eq!(source.read_diagnostics().chunks_expanded, 1);
        assert_eq!(
            latest.messages[0].payload.as_ptr(),
            position.active[0].messages[0].payload.as_ptr()
        );
        assert!(source.seek_chunks.is_none());
    }
}
