use std::{
    collections::{BTreeMap, BTreeSet},
    io::SeekFrom,
    sync::Arc,
};

use mcap::{
    Summary,
    sans_io::{SummaryReadEvent, SummaryReader, SummaryReaderOptions},
};
use viewer_core::{ArrivalTime, IndexedChunkFact, McapSummaryIdentity, StreamId};
use viewer_remote_protocol::{
    CatalogResponse, MessageCount, RecordingDescriptor, RemoteTimeRange, StreamDescriptor,
    TimestampNs,
};

use crate::{
    config::{Limits, RecordingConfig},
    error::ServerError,
    file_reader::{FileRangeReader, ReadMetrics},
};

const FOOTER_RECORD_AND_MAGIC_LEN: usize = 37;

#[derive(Debug)]
pub(crate) struct Recording {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) revision: String,
    pub(crate) reader: FileRangeReader,
    pub(crate) summary: Summary,
    pub(crate) catalog: CatalogResponse,
    pub(crate) channel_to_stream: BTreeMap<u16, u32>,
    pub(crate) stream_to_channel: BTreeMap<u32, u16>,
    #[cfg(test)]
    pub(crate) catalog_reads: ReadMetrics,
}

impl Recording {
    pub(crate) fn open(
        config: &RecordingConfig,
        limits: &Limits,
    ) -> Result<Arc<Self>, ServerError> {
        let reader = FileRangeReader::open(&config.path)
            .map_err(|error| ServerError::internal("could not open configured recording", error))?;
        let mut reads = ReadMetrics::default();
        let summary = read_summary(&reader, limits.max_chunk_bytes, &mut reads)?;
        if summary.chunk_indexes.is_empty() {
            return Err(ServerError::unprocessable(
                "missing_chunk_index",
                "recording summary contains no Chunk Index",
            ));
        }
        for chunk in &summary.chunk_indexes {
            if !matches!(chunk.compression.as_str(), "" | "zstd") {
                return Err(ServerError::unprocessable(
                    "unsupported_compression",
                    format!("unsupported MCAP compression: {}", chunk.compression),
                ));
            }
            if chunk.compressed_size > limits.max_chunk_bytes as u64
                || chunk.uncompressed_size > limits.max_chunk_bytes as u64
            {
                return Err(ServerError::unprocessable(
                    "chunk_too_large",
                    "recording contains a Chunk larger than max_chunk_bytes",
                ));
            }
        }
        let stats = summary.stats.as_ref().ok_or_else(|| {
            ServerError::unprocessable(
                "missing_statistics",
                "recording summary contains no Statistics record",
            )
        })?;
        let end_ns_exclusive = stats.message_end_time.checked_add(1).ok_or_else(|| {
            ServerError::unprocessable(
                "invalid_time_range",
                "recording end time cannot be represented as an exclusive bound",
            )
        })?;
        let summary_crc = read_footer_crc(&reader, &mut reads)?;
        let revision = McapSummaryIdentity {
            file_size: reader.file_size(),
            summary_crc,
            message_start_time: stats.message_start_time,
            message_end_time: stats.message_end_time,
            message_count: stats.message_count,
            schema_count: stats.schema_count,
            channel_count: stats.channel_count,
            chunk_count: summary.chunk_indexes.len(),
        }
        .revision();

        let mut cdr_channels: Vec<_> = summary
            .channels
            .values()
            .filter(|channel| channel.message_encoding == "cdr")
            .cloned()
            .collect();
        cdr_channels.sort_by_key(|channel| {
            (
                channel.topic.clone(),
                channel
                    .schema
                    .as_ref()
                    .map(|schema| schema.name.clone())
                    .unwrap_or_default(),
                channel.message_encoding.clone(),
                channel.id,
            )
        });

        let mut streams = Vec::with_capacity(cdr_channels.len());
        let mut channel_to_stream = BTreeMap::new();
        let mut stream_to_channel = BTreeMap::new();
        for (index, channel) in cdr_channels.into_iter().enumerate() {
            let id = u32::try_from(index + 1).map_err(|_| {
                ServerError::unprocessable("too_many_streams", "recording has too many streams")
            })?;
            let schema_name = channel
                .schema
                .as_ref()
                .map(|schema| schema.name.clone())
                .unwrap_or_default();
            let schema_encoding = channel
                .schema
                .as_ref()
                .map(|schema| schema.encoding.clone())
                .unwrap_or_default();
            channel_to_stream.insert(channel.id, id);
            stream_to_channel.insert(id, channel.id);
            streams.push(StreamDescriptor {
                id,
                topic: channel.topic.clone(),
                schema_name,
                schema_encoding,
                message_encoding: channel.message_encoding.clone(),
                message_count: stats
                    .channel_message_counts
                    .get(&channel.id)
                    .copied()
                    .map(MessageCount::new),
            });
        }
        let index_facts = indexed_chunk_facts(&summary, &channel_to_stream);
        for stream in &streams {
            viewer_core::ensure_indexed(
                &index_facts,
                StreamId(stream.id),
                stream.message_count.map(MessageCount::get),
            )
            .map_err(|error| {
                ServerError::unprocessable(
                    "restore_index_unavailable",
                    format!("recording cannot provide indexed restore: {error}"),
                )
            })?;
        }

        let catalog = CatalogResponse::new(
            config.id.clone(),
            revision.clone(),
            RemoteTimeRange {
                start_ns: TimestampNs::new(stats.message_start_time),
                end_ns_exclusive: TimestampNs::new(end_ns_exclusive),
            },
            streams,
        );
        tracing::info!(
            recording_id = %config.id,
            path = %config.path.display(),
            file_size = reader.file_size(),
            revision = %revision,
            start_ns = stats.message_start_time,
            end_ns_exclusive,
            stream_count = catalog.streams.len(),
            chunk_count = summary.chunk_indexes.len(),
            catalog_read_calls = reads.calls,
            catalog_read_bytes = reads.bytes,
            "recording catalog initialized"
        );
        Ok(Arc::new(Self {
            id: config.id.clone(),
            display_name: config.display_name.clone(),
            revision,
            reader,
            summary,
            catalog,
            channel_to_stream,
            stream_to_channel,
            #[cfg(test)]
            catalog_reads: reads,
        }))
    }

    pub(crate) fn descriptor(&self) -> RecordingDescriptor {
        RecordingDescriptor {
            recording_id: self.id.clone(),
            display_name: self.display_name.clone(),
            recording_revision: self.revision.clone(),
            start_ns: self.catalog.time_range.start_ns,
            end_ns_exclusive: self.catalog.time_range.end_ns_exclusive,
            stream_count: self.catalog.streams.len(),
        }
    }

    pub(crate) fn indexed_chunk_facts(&self) -> Vec<IndexedChunkFact> {
        indexed_chunk_facts(&self.summary, &self.channel_to_stream)
    }

    pub(crate) fn topics_for_streams(
        &self,
        stream_ids: &BTreeSet<u32>,
    ) -> Result<BTreeSet<String>, ServerError> {
        stream_ids
            .iter()
            .map(|stream_id| {
                let channel_id = self.stream_to_channel.get(stream_id).ok_or_else(|| {
                    ServerError::bad_request(
                        "unknown_stream",
                        format!("unknown stream ID: {stream_id}"),
                    )
                })?;
                Ok(self.summary.channels[channel_id].topic.clone())
            })
            .collect()
    }
}

fn indexed_chunk_facts(
    summary: &Summary,
    channel_to_stream: &BTreeMap<u16, u32>,
) -> Vec<IndexedChunkFact> {
    summary
        .chunk_indexes
        .iter()
        .map(|chunk| IndexedChunkFact {
            start: ArrivalTime(i64::try_from(chunk.message_start_time).unwrap_or(i64::MAX)),
            end_inclusive: ArrivalTime(i64::try_from(chunk.message_end_time).unwrap_or(i64::MAX)),
            indexed_streams: chunk
                .message_index_offsets
                .keys()
                .filter_map(|channel| channel_to_stream.get(channel).copied())
                .map(StreamId)
                .collect(),
        })
        .collect()
}

fn read_summary(
    file: &FileRangeReader,
    maximum_record_bytes: usize,
    metrics: &mut ReadMetrics,
) -> Result<Summary, ServerError> {
    let mut reader = SummaryReader::new_with_options(
        SummaryReaderOptions::default()
            .with_file_size(file.file_size())
            .with_record_length_limit(maximum_record_bytes),
    );
    let mut position = 0u64;
    while let Some(event) = reader.next_event() {
        match event.map_err(mcap_error)? {
            SummaryReadEvent::SeekRequest(target) => {
                position = resolve_seek(position, file.file_size(), target)?;
                reader.notify_seeked(position);
            }
            SummaryReadEvent::ReadRequest(length) => {
                let data = file
                    .read_exact_at(position, length, maximum_record_bytes, metrics)
                    .map_err(|error| ServerError::internal("could not read MCAP summary", error))?;
                reader.insert(length).copy_from_slice(&data);
                reader.notify_read(length);
                position = position.checked_add(length as u64).ok_or_else(|| {
                    ServerError::unprocessable("malformed_recording", "summary position overflow")
                })?;
            }
        }
    }
    reader.finish().ok_or_else(|| {
        ServerError::unprocessable("missing_summary", "recording has no MCAP Summary section")
    })
}

fn read_footer_crc(file: &FileRangeReader, metrics: &mut ReadMetrics) -> Result<u32, ServerError> {
    let offset = file
        .file_size()
        .checked_sub(FOOTER_RECORD_AND_MAGIC_LEN as u64)
        .ok_or_else(|| {
            ServerError::unprocessable("malformed_recording", "recording is too short")
        })?;
    let tail = file
        .read_exact_at(offset, FOOTER_RECORD_AND_MAGIC_LEN, 1024, metrics)
        .map_err(|error| ServerError::internal("could not read MCAP Footer", error))?;
    // `mcap::read::footer` validates both magic values. Prefixing the requested tail with the
    // leading magic creates the smallest complete slice accepted by that public parser.
    let mut minimal = Vec::with_capacity(mcap::MAGIC.len() + tail.len());
    minimal.extend_from_slice(mcap::MAGIC);
    minimal.extend_from_slice(&tail);
    mcap::read::footer(&minimal)
        .map(|footer| footer.summary_crc)
        .map_err(mcap_error)
}

fn resolve_seek(current: u64, file_size: u64, seek: SeekFrom) -> Result<u64, ServerError> {
    let resolved = match seek {
        SeekFrom::Start(position) => Some(position),
        SeekFrom::Current(delta) => checked_signed_add(current, delta),
        SeekFrom::End(delta) => checked_signed_add(file_size, delta),
    }
    .ok_or_else(|| {
        ServerError::unprocessable("malformed_recording", "MCAP seek offset overflow")
    })?;
    if resolved > file_size {
        return Err(ServerError::unprocessable(
            "malformed_recording",
            "MCAP seek is outside the recording",
        ));
    }
    Ok(resolved)
}

fn checked_signed_add(base: u64, delta: i64) -> Option<u64> {
    if delta >= 0 {
        base.checked_add(delta as u64)
    } else {
        base.checked_sub(delta.unsigned_abs())
    }
}

fn mcap_error(error: mcap::McapError) -> ServerError {
    ServerError::unprocessable(
        "malformed_recording",
        format!("invalid MCAP structure: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;
    use std::path::Path;

    fn fixture() -> (RecordingConfig, Limits) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/camera-jpeg/camera_front_3s.mcap")
            .canonicalize()
            .unwrap();
        let config = ServerConfig::from_toml(&format!(
            r#"
allowed_origins = ["http://localhost:8080"]
[[recordings]]
id = "demo"
display_name = "Demo"
path = "{}"
"#,
            path.display()
        ))
        .unwrap();
        (
            RecordingConfig {
                id: "demo".into(),
                display_name: "Demo".into(),
                path,
            },
            config.limits,
        )
    }

    #[test]
    fn catalog_uses_bounded_ranges_and_assigns_deterministic_cdr_streams() {
        let (config, limits) = fixture();
        let first = Recording::open(&config, &limits).unwrap();
        let second = Recording::open(&config, &limits).unwrap();
        assert!(first.catalog_reads.bytes < first.reader.file_size());
        assert!(first.catalog_reads.calls > 0);
        assert_eq!(first.revision, second.revision);
        assert_eq!(first.catalog.streams, second.catalog.streams);
        assert!(
            first
                .catalog
                .streams
                .iter()
                .all(|stream| stream.message_encoding == "cdr")
        );
        assert!(first.catalog.streams.iter().any(|stream| {
            stream.schema_name == "sensor_msgs/msg/CompressedImage"
                && stream.message_encoding == "cdr"
        }));
        assert!(
            first
                .catalog
                .streams
                .iter()
                .all(|stream| stream.message_count.is_some()),
            "MCAP Statistics channel counts are part of the remote catalog"
        );
        assert_eq!(
            first.catalog.time_range.end_ns_exclusive.get(),
            first.summary.stats.as_ref().unwrap().message_end_time + 1
        );
    }

    #[test]
    fn revision_matches_existing_preview_fingerprint_implementation() {
        let (config, limits) = fixture();
        let recording = Recording::open(&config, &limits).unwrap();
        let bytes = std::fs::read(&config.path).unwrap();
        let fingerprint = viewer_preview_mcap::source_fingerprint(&bytes).unwrap();
        assert_eq!(
            recording.revision,
            format!("{}:{}", fingerprint.algorithm(), fingerprint.value())
        );
    }
}
