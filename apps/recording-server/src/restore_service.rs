use std::{
    collections::{BTreeSet, HashSet},
    sync::Arc,
    time::Instant,
};

use bytes::Bytes;
use mcap::sans_io::indexed_reader::ReadOrder;
use mcap::sans_io::{IndexedReadEvent, IndexedReader, IndexedReaderOptions};
use viewer_remote_protocol::{BatchEncoder, RemoteMessageRef};

use crate::{config::Limits, error::ServerError, metrics::RequestMetrics, recording::Recording};

#[derive(Clone, Debug)]
pub(crate) struct RestoreRequest {
    pub(crate) revision: String,
    pub(crate) latest_streams: Vec<u32>,
    pub(crate) history_streams: Vec<u32>,
    pub(crate) history_start_ns: u64,
    pub(crate) persistent_streams: Vec<u32>,
    pub(crate) target_ns: u64,
}

#[derive(Debug)]
pub(crate) struct RestoreBatch {
    pub(crate) body: Bytes,
    pub(crate) message_count: usize,
    pub(crate) metrics: RequestMetrics,
}

#[derive(Debug)]
struct OwnedMessage {
    stream_id: u32,
    sequence: u32,
    log_time_ns: u64,
    publish_time_ns: u64,
    source_order: u64,
    payload: Vec<u8>,
}

#[derive(Debug)]
struct RestoreBudget {
    messages: usize,
    encoded_bytes: usize,
}

impl RestoreBudget {
    fn new() -> Self {
        Self {
            messages: 0,
            encoded_bytes: BatchEncoder::new().encoded_len(),
        }
    }

    fn reserve(&mut self, payload_len: usize, limits: &Limits) -> Result<(), ServerError> {
        if self.messages == limits.max_messages {
            return Err(ServerError::too_large(
                "restore_too_large",
                "restore exceeds max_messages",
            ));
        }
        let frame_len = BatchEncoder::frame_len(payload_len)
            .map_err(|error| ServerError::too_large("message_too_large", error.to_string()))?;
        let encoded_bytes = self
            .encoded_bytes
            .checked_add(frame_len)
            .filter(|size| *size <= limits.max_response_bytes)
            .ok_or_else(|| {
                ServerError::too_large("restore_too_large", "restore exceeds max_response_bytes")
            })?;
        self.messages += 1;
        self.encoded_bytes = encoded_bytes;
        Ok(())
    }
}

pub(crate) fn read_restore(
    recording: Arc<Recording>,
    mut request: RestoreRequest,
    limits: Limits,
    request_id: u64,
) -> Result<RestoreBatch, ServerError> {
    if request.revision != recording.revision {
        return Err(ServerError::conflict(
            "recording revision does not match the current catalog",
        ));
    }
    normalize_streams(&recording, &mut request.latest_streams)?;
    normalize_streams(&recording, &mut request.history_streams)?;
    normalize_streams(&recording, &mut request.persistent_streams)?;
    ensure_message_indexes(&recording, &request.latest_streams)?;
    ensure_message_indexes(&recording, &request.history_streams)?;
    ensure_message_indexes(&recording, &request.persistent_streams)?;
    if request.latest_streams.is_empty()
        && request.history_streams.is_empty()
        && request.persistent_streams.is_empty()
    {
        return Err(ServerError::bad_request(
            "missing_streams",
            "restore requires at least one stream",
        ));
    }
    let recording_start = recording.catalog.time_range.start_ns.get();
    let recording_end = recording.catalog.time_range.end_ns_exclusive.get();
    if request.target_ns < recording_start || request.target_ns >= recording_end {
        return Err(ServerError::bad_request(
            "invalid_timestamp",
            "restore target is outside the recording",
        ));
    }
    if !request.history_streams.is_empty()
        && (request.history_start_ns > request.target_ns
            || request.target_ns.saturating_sub(request.history_start_ns) > limits.max_window_ns)
    {
        return Err(ServerError::bad_request(
            "window_limit_exceeded",
            "restore history exceeds the configured maximum window",
        ));
    }

    let mut metrics = RequestMetrics::new(request_id);
    let mut messages = Vec::new();
    let mut budget = RestoreBudget::new();
    if !request.latest_streams.is_empty() {
        messages.extend(read_latest(
            &recording,
            &request.latest_streams,
            request.target_ns,
            &limits,
            &mut metrics,
            &mut budget,
        )?);
    }
    if !request.history_streams.is_empty() {
        messages.extend(read_forward(
            &recording,
            &request.history_streams,
            request.history_start_ns.max(recording_start),
            request.target_ns,
            &limits,
            &mut metrics,
            &mut budget,
        )?);
    }
    if !request.persistent_streams.is_empty() {
        let persistent_end = recording_end.checked_sub(1).ok_or_else(|| {
            ServerError::unprocessable("malformed_recording", "recording has an empty time range")
        })?;
        messages.extend(read_forward(
            &recording,
            &request.persistent_streams,
            recording_start,
            persistent_end,
            &limits,
            &mut metrics,
            &mut budget,
        )?);
    }
    messages.sort_by_key(|message| (message.log_time_ns, message.stream_id, message.source_order));

    let encode_started = Instant::now();
    let mut encoder = BatchEncoder::new();
    for message in &messages {
        encoder
            .push(RemoteMessageRef {
                stream_id: message.stream_id,
                sequence: message.sequence,
                log_time_ns: message.log_time_ns,
                publish_time_ns: message.publish_time_ns,
                payload: &message.payload,
            })
            .map_err(|error| ServerError::internal("restore encoding failed", error))?;
    }
    metrics.batch_encode_ms = encode_started.elapsed().as_secs_f64() * 1_000.0;
    Ok(RestoreBatch {
        body: encoder.finish(),
        message_count: messages.len(),
        metrics,
    })
}

fn read_latest(
    recording: &Recording,
    streams: &[u32],
    target: u64,
    limits: &Limits,
    metrics: &mut RequestMetrics,
    budget: &mut RestoreBudget,
) -> Result<Vec<OwnedMessage>, ServerError> {
    let stream_set = streams.iter().copied().collect::<BTreeSet<_>>();
    let topics = recording.topics_for_streams(&stream_set)?;
    let end = target.checked_add(1).ok_or_else(|| {
        ServerError::bad_request(
            "invalid_timestamp",
            "restore target cannot be made inclusive",
        )
    })?;
    let options = IndexedReaderOptions::new()
        .with_order(ReadOrder::ReverseLogTime)
        .include_topics(topics)
        .log_time_before(end)
        .with_record_length_limit(limits.max_chunk_bytes);
    let mut reader = indexed_reader(recording, options)?;
    let mut found = HashSet::new();
    let mut output = Vec::new();
    let mut source_order = 0_u64;
    while let Some(event) = reader.next_event() {
        match event.map_err(restore_index_error)? {
            IndexedReadEvent::ReadChunkRequest { offset, length } => {
                insert_chunk(recording, &mut reader, offset, length, limits, metrics)?;
            }
            IndexedReadEvent::Message { header, data } => {
                let Some(&stream_id) = recording.channel_to_stream.get(&header.channel_id) else {
                    continue;
                };
                if stream_set.contains(&stream_id) && found.insert(stream_id) {
                    budget.reserve(data.len(), limits)?;
                    output.push(owned(stream_id, header, data, source_order));
                    source_order = source_order.saturating_add(1);
                    if found.len() == stream_set.len() {
                        break;
                    }
                }
            }
        }
    }
    Ok(output)
}

fn read_forward(
    recording: &Recording,
    streams: &[u32],
    start: u64,
    target: u64,
    limits: &Limits,
    metrics: &mut RequestMetrics,
    budget: &mut RestoreBudget,
) -> Result<Vec<OwnedMessage>, ServerError> {
    let stream_set = streams.iter().copied().collect::<BTreeSet<_>>();
    let topics = recording.topics_for_streams(&stream_set)?;
    let end = target.checked_add(1).ok_or_else(|| {
        ServerError::bad_request(
            "invalid_timestamp",
            "restore target cannot be made inclusive",
        )
    })?;
    let options = IndexedReaderOptions::new()
        .with_order(ReadOrder::LogTime)
        .include_topics(topics)
        .log_time_on_or_after(start)
        .log_time_before(end)
        .with_record_length_limit(limits.max_chunk_bytes);
    let mut reader = indexed_reader(recording, options)?;
    let mut output = Vec::new();
    let mut source_order = 0_u64;
    while let Some(event) = reader.next_event() {
        match event.map_err(restore_index_error)? {
            IndexedReadEvent::ReadChunkRequest { offset, length } => {
                insert_chunk(recording, &mut reader, offset, length, limits, metrics)?;
            }
            IndexedReadEvent::Message { header, data } => {
                let Some(&stream_id) = recording.channel_to_stream.get(&header.channel_id) else {
                    continue;
                };
                if stream_set.contains(&stream_id) {
                    budget.reserve(data.len(), limits)?;
                    output.push(owned(stream_id, header, data, source_order));
                    source_order = source_order.saturating_add(1);
                }
            }
        }
    }
    Ok(output)
}

fn indexed_reader(
    recording: &Recording,
    options: IndexedReaderOptions,
) -> Result<IndexedReader, ServerError> {
    IndexedReader::new_with_options(&recording.summary, options).map_err(restore_index_error)
}

fn insert_chunk(
    recording: &Recording,
    reader: &mut IndexedReader,
    offset: u64,
    length: usize,
    limits: &Limits,
    metrics: &mut RequestMetrics,
) -> Result<(), ServerError> {
    let compressed = recording
        .reader
        .read_exact_at(offset, length, limits.max_chunk_bytes, &mut metrics.reads)
        .map_err(|error| ServerError::internal("restore Chunk read failed", error))?;
    let started = Instant::now();
    reader
        .insert_chunk_record_data(offset, &compressed)
        .map_err(restore_index_error)?;
    metrics.chunk_decompress_ms += started.elapsed().as_secs_f64() * 1_000.0;
    metrics.chunk_count = metrics.chunk_count.saturating_add(1);
    Ok(())
}

fn normalize_streams(recording: &Recording, streams: &mut Vec<u32>) -> Result<(), ServerError> {
    streams.sort_unstable();
    streams.dedup();
    recording.topics_for_streams(&streams.iter().copied().collect())?;
    Ok(())
}

fn ensure_message_indexes(recording: &Recording, streams: &[u32]) -> Result<(), ServerError> {
    let facts = recording.indexed_chunk_facts();
    for stream in streams {
        let channel = recording.stream_to_channel.get(stream).ok_or_else(|| {
            ServerError::bad_request("unknown_stream", format!("unknown stream ID: {stream}"))
        })?;
        let message_count = recording
            .summary
            .stats
            .as_ref()
            .and_then(|stats| stats.channel_message_counts.get(channel).copied());
        viewer_core::ensure_indexed(&facts, viewer_core::StreamId(*stream), message_count)
            .map_err(|error| {
                ServerError::unprocessable(
                    "restore_index_unavailable",
                    format!("recording cannot provide indexed restore: {error}"),
                )
            })?;
    }
    Ok(())
}

fn owned(
    stream_id: u32,
    header: mcap::records::MessageHeader,
    data: &[u8],
    source_order: u64,
) -> OwnedMessage {
    OwnedMessage {
        stream_id,
        sequence: header.sequence,
        log_time_ns: header.log_time,
        publish_time_ns: header.publish_time,
        source_order,
        payload: data.to_vec(),
    }
}

fn restore_index_error(error: mcap::McapError) -> ServerError {
    ServerError::unprocessable(
        "restore_index_unavailable",
        format!("recording cannot provide indexed restore: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RecordingConfig, ServerConfig};
    use mcap::{WriteOptions, Writer, records::MessageHeader};
    use std::{
        collections::BTreeMap,
        io::Cursor,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };
    use viewer_remote_protocol::BatchDecoder;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct TemporaryRecording(PathBuf);

    impl Drop for TemporaryRecording {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn synthetic_recording(
        emit_message_indexes: bool,
    ) -> Result<(TemporaryRecording, Arc<Recording>, Limits), ServerError> {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = Writer::with_options(
                &mut output,
                WriteOptions::new()
                    .use_chunks(true)
                    .chunk_size(Some(128))
                    .emit_message_indexes(emit_message_indexes),
            )
            .unwrap();
            let schema = writer
                .add_schema(
                    "tf2_msgs/msg/TFMessage",
                    "ros2msg",
                    b"geometry_msgs/TransformStamped[] transforms\n",
                )
                .unwrap();
            let channel = writer
                .add_channel(schema, "/tf_static", "cdr", &BTreeMap::new())
                .unwrap();
            for (sequence, time, marker) in [(0, 10, 1), (1, 20, 2)] {
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence,
                            log_time: time,
                            publish_time: time,
                        },
                        &[marker],
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        let path = std::env::temp_dir().join(format!(
            "recording-server-restore-{}-{}.mcap",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, output.into_inner()).unwrap();
        let guard = TemporaryRecording(path.clone());
        let limits = Limits::default();
        let recording = Recording::open(
            &RecordingConfig {
                id: "synthetic".into(),
                display_name: "Synthetic".into(),
                path,
            },
            &limits,
        )?;
        Ok((guard, recording, limits))
    }

    fn fixture() -> (Arc<Recording>, Limits) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap")
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
        let recording = Recording::open(
            &RecordingConfig {
                id: "demo".into(),
                display_name: "Demo".into(),
                path,
            },
            &config.limits,
        )
        .unwrap();
        (recording, config.limits)
    }

    #[test]
    fn multi_camera_latest_restore_returns_one_each_from_one_shared_chunk() {
        let (recording, limits) = fixture();
        let cameras = recording
            .catalog
            .streams
            .iter()
            .filter(|stream| stream.schema_name == "sensor_msgs/msg/CompressedImage")
            .map(|stream| stream.id)
            .collect::<Vec<_>>();
        let target = recording.catalog.time_range.start_ns.get() + 500_000_000;
        let batch = read_restore(
            Arc::clone(&recording),
            RestoreRequest {
                revision: recording.revision.clone(),
                latest_streams: cameras.clone(),
                history_streams: Vec::new(),
                history_start_ns: target,
                persistent_streams: Vec::new(),
                target_ns: target,
            },
            limits,
            1,
        )
        .unwrap();
        let messages = BatchDecoder::new(&batch.body).unwrap().collect().unwrap();
        assert_eq!(messages.len(), cameras.len());
        assert_eq!(
            batch.metrics.chunk_count, 1,
            "all Camera predecessors in one compressed chunk must share one decompression"
        );
        assert!(messages.iter().all(|message| message.log_time_ns <= target));
    }

    #[test]
    fn persistent_restore_bootstraps_the_full_small_archive_once() {
        let (_guard, recording, limits) = synthetic_recording(true).unwrap();
        let stream = recording.catalog.streams[0].id;
        let batch = read_restore(
            Arc::clone(&recording),
            RestoreRequest {
                revision: recording.revision.clone(),
                latest_streams: Vec::new(),
                history_streams: Vec::new(),
                history_start_ns: 10,
                persistent_streams: vec![stream],
                target_ns: 10,
            },
            limits,
            2,
        )
        .unwrap();
        let messages = BatchDecoder::new(&batch.body).unwrap().collect().unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|message| (message.log_time_ns, message.payload[0]))
                .collect::<Vec<_>>(),
            vec![(10, 1), (20, 2)],
            "the Web session caches the complete persistent archive and filters it per seek target"
        );
    }

    #[test]
    fn restore_without_message_indexes_is_explicitly_unavailable() {
        let Err(error) = synthetic_recording(false) else {
            panic!("unindexed recording must fail while opening the catalog");
        };
        assert_eq!(error.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error.code, "restore_index_unavailable");
    }

    #[test]
    fn restore_hard_limit_is_enforced_before_retaining_another_message() {
        let (_guard, recording, mut limits) = synthetic_recording(true).unwrap();
        limits.max_messages = 1;
        let stream = recording.catalog.streams[0].id;
        let error = read_restore(
            Arc::clone(&recording),
            RestoreRequest {
                revision: recording.revision.clone(),
                latest_streams: Vec::new(),
                history_streams: Vec::new(),
                history_start_ns: 10,
                persistent_streams: vec![stream],
                target_ns: 10,
            },
            limits,
            4,
        )
        .unwrap_err();
        assert_eq!(error.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(error.code, "restore_too_large");
    }
}
