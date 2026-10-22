//! Byte-bounded physical Chunk reuse for repeated playback restoration.
use super::ParsedChunkMessage;
use std::{collections::VecDeque, sync::Arc};

struct SeekChunk {
    index: usize,
    bytes: u64,
    messages: Arc<[ParsedChunkMessage]>,
}

pub(super) struct SeekChunks {
    entries: VecDeque<SeekChunk>,
    bytes: u64,
    maximum: u64,
}

impl SeekChunks {
    pub fn new(maximum: u64) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            maximum,
        }
    }

    pub fn get(&mut self, index: usize) -> Option<Arc<[ParsedChunkMessage]>> {
        let slot = self.entries.iter().position(|entry| entry.index == index)?;
        let entry = self.entries.remove(slot).expect("located Chunk");
        let messages = entry.messages.clone();
        self.entries.push_back(entry);
        Some(messages)
    }

    pub fn insert(&mut self, index: usize, bytes: u64, messages: Arc<[ParsedChunkMessage]>) {
        if bytes > self.maximum || self.entries.iter().any(|entry| entry.index == index) {
            return;
        }
        while self.bytes.saturating_add(bytes) > self.maximum || self.entries.len() >= 16 {
            self.evict_oldest();
        }
        self.bytes += bytes;
        self.entries.push_back(SeekChunk {
            index,
            bytes,
            messages,
        });
    }

    pub fn resize(&mut self, maximum: u64) {
        self.maximum = maximum;
        while self.bytes > maximum {
            self.evict_oldest();
        }
    }

    fn evict_oldest(&mut self) {
        let entry = self.entries.pop_front().expect("nonempty bounded cache");
        self.bytes -= entry.bytes;
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn make_room_for_preparation(&mut self, active: &[usize], maximum_extra: u64) -> u64 {
        let mut extra: u64 = self
            .entries
            .iter()
            .filter(|entry| !active.contains(&entry.index))
            .map(|entry| entry.bytes)
            .sum();
        while extra > maximum_extra {
            let slot = self
                .entries
                .iter()
                .position(|entry| !active.contains(&entry.index))
                .expect("extra backing belongs to an inactive Chunk");
            let entry = self.entries.remove(slot).expect("located inactive Chunk");
            self.bytes -= entry.bytes;
            extra -= entry.bytes;
        }
        extra
    }

    #[cfg(any(not(target_arch = "wasm32"), test))]
    pub fn contains(&self, index: usize) -> bool {
        self.entries.iter().any(|entry| entry.index == index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playback() -> crate::McapPlayback {
        use mcap::{WriteOptions, Writer, records::MessageHeader};
        use std::{collections::BTreeMap, io::Cursor};
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = Writer::with_options(
                &mut bytes,
                WriteOptions::new()
                    .compression(Some(mcap::Compression::Zstd))
                    .chunk_size(None),
            )
            .unwrap();
            let schema = writer
                .add_schema("demo/msg/Value", "ros2msg", b"uint8 value")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/value", "cdr", &BTreeMap::new())
                .unwrap();
            for times in [[0, 10, 20], [100, 110, 120]] {
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
        let mut playback =
            crate::McapPlayback::from_bytes(bytes::Bytes::from(bytes.into_inner())).unwrap();
        playback.select_streams([viewer_core::StreamId(1)]).unwrap();
        playback
    }

    fn restore(target: i64) -> viewer_core::RestorePlan {
        viewer_core::RestorePlan {
            target: viewer_core::ArrivalTime(target),
            latest_before: vec![viewer_core::StreamId(1)],
            histories: vec![],
            persistent: vec![],
        }
    }

    #[test]
    fn alternating_seeks_reuse_validated_chunks_and_preserve_forward_delivery() {
        let mut playback = playback();
        for time in [10, 110, 10, 110, 10] {
            playback
                .seek_with(&restore(time), |target, messages| {
                    assert_eq!(messages.len(), 1);
                    assert_eq!(messages[0].arrival_time, target);
                    assert_eq!(messages[0].payload.as_ref(), &[time as u8]);
                    Ok::<_, std::convert::Infallible>(())
                })
                .unwrap();
        }
        let source = playback.recording_source();
        assert_eq!(source.read_diagnostics().chunks_expanded, 2);
        assert!(source.read_diagnostics().seek_cache_hits > 0);
        assert!(source.read_diagnostics().seek_cache_backing_bytes > 0);
        // Ordinary queries do not pin playback's resident Chunk set.
        assert!(playback.query_source().seek_chunks.is_none());
        playback.clock_mut().toggle();
        playback
            .tick(std::time::Duration::from_nanos(10), |_, messages| {
                assert_eq!(
                    messages
                        .iter()
                        .map(|m| m.arrival_time.0)
                        .collect::<Vec<_>>(),
                    [20]
                );
            })
            .unwrap();
    }

    #[test]
    fn rejected_restore_keeps_the_old_clock_and_forward_position() {
        let mut playback = playback();
        playback
            .seek_with(&restore(10), |_, _| Ok::<_, &'static str>(()))
            .unwrap();
        let before = playback.clock().view();
        assert!(matches!(
            playback.seek_with(&restore(110), |_, _| Err("reject")),
            Err(crate::McapSeekError::Restore("reject"))
        ));
        assert_eq!(playback.clock().view(), before);
        playback.clock_mut().toggle();
        playback
            .tick(std::time::Duration::from_nanos(10), |_, messages| {
                assert_eq!(
                    messages
                        .iter()
                        .map(|m| m.arrival_time.0)
                        .collect::<Vec<_>>(),
                    [20]
                );
            })
            .unwrap();
    }

    #[test]
    fn sequential_playback_does_not_populate_the_seek_cache() {
        let mut playback = playback();
        playback.clock_mut().toggle();
        playback
            .tick(std::time::Duration::from_nanos(200), |_, _| {})
            .unwrap();
        assert_eq!(
            playback
                .recording_source()
                .read_diagnostics()
                .seek_cache_backing_bytes,
            0
        );
    }

    #[test]
    fn forward_preparation_reclaims_only_extra_backings_without_double_counting_active() {
        let mut cache = SeekChunks::new(20);
        for index in 0..3 {
            cache.insert(index, 4, Arc::from([]));
        }
        assert_eq!(cache.make_room_for_preparation(&[0], 4), 4);
        assert!(cache.contains(0));
        assert!(!cache.contains(1));
        assert!(cache.contains(2));
        assert_eq!(cache.bytes(), 8);
    }

    #[test]
    fn byte_budget_and_recency_evict_the_least_recent_chunk() {
        let mut cache = SeekChunks::new(10);
        cache.insert(0, 4, Arc::from([]));
        cache.insert(1, 4, Arc::from([]));
        cache.get(0).unwrap();
        cache.insert(2, 4, Arc::from([]));
        assert!(cache.contains(0));
        assert!(!cache.contains(1));
        assert!(cache.contains(2));
        assert_eq!(cache.bytes(), 8);
        cache.insert(3, 11, Arc::from([]));
        assert_eq!(cache.bytes(), 8);
        cache.resize(4);
        assert!(!cache.contains(0));
        assert!(cache.contains(2));
        cache.resize(0);
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn tiny_chunks_cannot_accumulate_unbounded_entries() {
        let mut cache = SeekChunks::new(1000);
        for index in 0..100 {
            cache.insert(index, 1, Arc::from([]));
        }
        assert_eq!(cache.entries.len(), 16);
        assert_eq!(cache.bytes(), 16);
    }
}
