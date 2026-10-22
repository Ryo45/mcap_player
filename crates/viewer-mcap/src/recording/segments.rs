//! A recording's indexed parts share one address space and logical channel namespace.
//! Offsets are internal read addresses, not a synthesized on-disk MCAP file.
use super::backing::{Backing, FileRanges};
use crate::McapSourceError;
use bytes::Bytes;
use mcap::{Channel, Schema, Summary};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) type ChannelMap = BTreeMap<u16, u16>;

pub(super) struct SegmentedBacking {
    parts: Vec<(u64, Backing)>,
    pub(super) length: u64,
    pub(super) summary: Arc<Summary>,
    pub(super) summary_crc: u32,
    pub(super) chunk_channels: Arc<[Arc<ChannelMap>]>,
}

fn invalid(reason: &str) -> McapSourceError {
    McapSourceError::InvalidChunk(format!("split recording: {reason}"))
}

pub(super) fn directory_paths(path: &Path) -> Result<Vec<PathBuf>, McapSourceError> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(path).map_err(McapSourceError::Io)? {
        let entry = entry.map_err(McapSourceError::Io)?;
        let path = entry.path();
        // The viewer's derived preview is a sidecar, never a recording part.
        if path.file_name().is_some_and(|name| name == "preview.mcap") {
            continue;
        }
        if path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("mcap"))
        {
            if paths.len() == 1024 {
                return Err(invalid("at most 1024 parts may be opened"));
            }
            paths.push(path);
        }
    }
    paths.sort();
    if paths.is_empty() {
        return Err(invalid("directory contains no MCAP files"));
    }
    Ok(paths)
}

impl SegmentedBacking {
    pub(super) fn open(paths: &[PathBuf]) -> Result<Self, McapSourceError> {
        if paths.is_empty() || paths.len() > 1024 {
            return Err(invalid("expected 1..1024 parts"));
        }
        let mut opened = Vec::new();
        let mut seen = HashSet::new();
        for path in paths {
            let canonical = path.canonicalize().map_err(McapSourceError::Io)?;
            if !seen.insert(canonical.clone()) {
                return Err(invalid("duplicate part path"));
            }
            let backing = Backing::File(Arc::new(FileRanges::open(&canonical)?));
            let summary = Arc::unwrap_or_clone(backing.summary()?);
            if summary.chunk_indexes.is_empty() {
                return Err(McapSourceError::ChunkIndexRequired);
            }
            let start = summary
                .chunk_indexes
                .iter()
                .filter(|c| !c.message_index_offsets.is_empty())
                .map(|c| c.message_start_time)
                .min()
                .unwrap_or(0);
            opened.push((start, canonical, backing, summary));
        }
        // Filename order is only a deterministic tie breaker; log time owns the timeline.
        opened.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        let mut merged = Summary::default();
        let mut schemas = HashMap::<Schema<'static>, Arc<Schema<'static>>>::new();
        let mut topics = HashMap::<String, Arc<Channel<'static>>>::new();
        let mut chunk_channels = Vec::new();
        let mut parts = Vec::new();
        let mut length = 0_u64;
        let mut fingerprint = crc32fast::Hasher::new();
        let all_stats = opened.iter().all(|p| p.3.stats.is_some());
        let mut stats = opened.first().and_then(|p| p.3.stats.clone());
        if let Some(s) = &mut stats {
            s.message_count = 0;
            s.channel_message_counts.clear();
            s.attachment_count = 0;
            s.metadata_count = 0;
            s.message_start_time = u64::MAX;
            s.message_end_time = 0;
        }
        for (_, _, backing, summary) in opened {
            let base = length;
            length = length
                .checked_add(backing.len())
                .ok_or(McapSourceError::ReadOverflow)?;
            fingerprint.update(&backing.len().to_le_bytes());
            fingerprint.update(&backing.summary_crc()?.to_le_bytes());
            let mut channels = ChannelMap::new();
            let mut local_topics = HashSet::new();
            let mut ordered = summary.channels.values().collect::<Vec<_>>();
            ordered.sort_by_key(|c| c.id);
            for channel in ordered {
                if !local_topics.insert(&channel.topic) {
                    return Err(invalid(
                        "duplicate topic channels within a part are ambiguous",
                    ));
                }
                let schema = if let Some(schema) = &channel.schema {
                    let mut key = (**schema).clone();
                    key.id = 0;
                    let canonical = if let Some(value) = schemas.get(&key) {
                        value.clone()
                    } else {
                        let id = u16::try_from(merged.schemas.len() + 1)
                            .map_err(|_| invalid("too many schemas"))?;
                        let mut value = key.clone();
                        value.id = id;
                        let value = Arc::new(value);
                        merged.schemas.insert(id, value.clone());
                        schemas.insert(key, value.clone());
                        value
                    };
                    Some(canonical)
                } else {
                    None
                };
                let canonical = if let Some(value) = topics.get(&channel.topic) {
                    if value.schema != schema || value.message_encoding != channel.message_encoding
                    {
                        return Err(invalid(
                            "a topic changes schema or message encoding between parts",
                        ));
                    }
                    value.clone()
                } else {
                    let id = u16::try_from(merged.channels.len() + 1)
                        .map_err(|_| invalid("too many channels"))?;
                    let value = Arc::new(Channel {
                        id,
                        schema,
                        topic: channel.topic.clone(),
                        message_encoding: channel.message_encoding.clone(),
                        metadata: channel.metadata.clone(),
                    });
                    merged.channels.insert(id, value.clone());
                    topics.insert(channel.topic.clone(), value.clone());
                    value
                };
                channels.insert(channel.id, canonical.id);
            }
            if all_stats && let (Some(total), Some(part)) = (&mut stats, &summary.stats) {
                total.message_count = total
                    .message_count
                    .checked_add(part.message_count)
                    .ok_or(McapSourceError::ReadOverflow)?;
                total.attachment_count = total
                    .attachment_count
                    .checked_add(part.attachment_count)
                    .ok_or(McapSourceError::ReadOverflow)?;
                total.metadata_count = total
                    .metadata_count
                    .checked_add(part.metadata_count)
                    .ok_or(McapSourceError::ReadOverflow)?;
                // Empty parts conventionally report [0,0]; they carry no timeline facts.
                if part.message_count != 0 {
                    total.message_start_time =
                        total.message_start_time.min(part.message_start_time);
                    total.message_end_time = total.message_end_time.max(part.message_end_time);
                }
                for (local, count) in &part.channel_message_counts {
                    let global = channels
                        .get(local)
                        .ok_or_else(|| invalid("statistics reference an unknown channel"))?;
                    let count_total = total.channel_message_counts.entry(*global).or_default();
                    *count_total = count_total
                        .checked_add(*count)
                        .ok_or(McapSourceError::ReadOverflow)?;
                }
            }
            let channels = Arc::new(channels);
            for mut chunk in summary.chunk_indexes {
                // Validate physical addresses before shifting them into the recording namespace.
                let end = chunk
                    .chunk_start_offset
                    .checked_add(chunk.chunk_length)
                    .ok_or(McapSourceError::ReadOverflow)?;
                if end > backing.len() {
                    return Err(invalid("chunk lies outside its part"));
                }
                let index_start = chunk
                    .message_index_offsets
                    .values()
                    .copied()
                    .min()
                    .unwrap_or(end);
                let index_end = index_start
                    .checked_add(chunk.message_index_length)
                    .ok_or(McapSourceError::ReadOverflow)?;
                if index_start < end || index_end > backing.len() {
                    return Err(invalid("message index span lies outside its part"));
                }
                let mut offsets = BTreeMap::new();
                for (local, offset) in chunk.message_index_offsets {
                    if offset < index_start || offset >= index_end {
                        return Err(invalid("message index lies outside its part"));
                    }
                    let global = *channels
                        .get(&local)
                        .ok_or_else(|| invalid("chunk index references an unknown channel"))?;
                    offsets.insert(
                        global,
                        base.checked_add(offset)
                            .ok_or(McapSourceError::ReadOverflow)?,
                    );
                }
                chunk.chunk_start_offset = base
                    .checked_add(chunk.chunk_start_offset)
                    .ok_or(McapSourceError::ReadOverflow)?;
                chunk.message_index_offsets = offsets;
                merged.chunk_indexes.push(chunk);
                chunk_channels.push(channels.clone());
            }
            for mut index in summary.attachment_indexes {
                if index
                    .offset
                    .checked_add(index.length)
                    .is_none_or(|end| end > backing.len())
                {
                    return Err(invalid("attachment index lies outside its part"));
                }
                index.offset = index
                    .offset
                    .checked_add(base)
                    .ok_or(McapSourceError::ReadOverflow)?;
                merged.attachment_indexes.push(index);
            }
            for mut index in summary.metadata_indexes {
                if index
                    .offset
                    .checked_add(index.length)
                    .is_none_or(|end| end > backing.len())
                {
                    return Err(invalid("metadata index lies outside its part"));
                }
                index.offset = index
                    .offset
                    .checked_add(base)
                    .ok_or(McapSourceError::ReadOverflow)?;
                merged.metadata_indexes.push(index);
            }
            parts.push((base, backing));
        }
        if all_stats && let Some(mut stats) = stats {
            if stats.message_count == 0 {
                stats.message_start_time = 0;
                stats.message_end_time = 0;
            }
            stats.schema_count =
                u16::try_from(merged.schemas.len()).map_err(|_| invalid("too many schemas"))?;
            stats.channel_count =
                u32::try_from(merged.channels.len()).map_err(|_| McapSourceError::ReadOverflow)?;
            stats.chunk_count = u32::try_from(merged.chunk_indexes.len())
                .map_err(|_| McapSourceError::ReadOverflow)?;
            merged.stats = Some(stats);
        }
        let result = Self {
            parts,
            length,
            summary: Arc::new(merged),
            summary_crc: fingerprint.finalize(),
            chunk_channels: chunk_channels.into(),
        };
        result.validate()?;
        Ok(result)
    }

    pub(super) fn read(
        &self,
        offset: u64,
        length: usize,
        maximum: usize,
    ) -> Result<Bytes, McapSourceError> {
        let i = self
            .parts
            .partition_point(|(start, _)| *start <= offset)
            .checked_sub(1)
            .ok_or(McapSourceError::ReadOverflow)?;
        let (start, part) = &self.parts[i];
        part.read_with_limit(offset - *start, length, maximum)
    }
    pub(super) fn validate(&self) -> Result<(), McapSourceError> {
        for (_, part) in &self.parts {
            part.validate()?;
        }
        Ok(())
    }
    pub(super) fn read_totals(&self) -> (u64, u64) {
        self.parts
            .iter()
            .map(|(_, p)| p.file_read_totals())
            .fold((0, 0), |(a, b), (c, d)| {
                (a.saturating_add(c), b.saturating_add(d))
            })
    }
}
