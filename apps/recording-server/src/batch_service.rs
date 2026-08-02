use std::{collections::BTreeSet, sync::Arc, time::Instant};

use bytes::Bytes;
use mcap::sans_io::indexed_reader::ReadOrder;
use mcap::sans_io::{IndexedReadEvent, IndexedReader, IndexedReaderOptions};
use viewer_remote_protocol::{BatchEncoder, RemoteMessageRef};

use crate::{
    config::Limits, cursor::ContinuationCursor, error::ServerError, metrics::RequestMetrics,
    recording::Recording,
};

#[derive(Clone, Debug)]
pub(crate) struct BatchRequest {
    pub(crate) revision: String,
    pub(crate) stream_ids: Vec<u32>,
    pub(crate) start_ns: u64,
    pub(crate) end_ns: u64,
    pub(crate) max_bytes: usize,
    pub(crate) max_messages: usize,
    pub(crate) cursor: Option<String>,
}

#[derive(Debug)]
pub(crate) struct BatchPage {
    pub(crate) body: Bytes,
    pub(crate) complete: bool,
    pub(crate) next_cursor: Option<String>,
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

pub(crate) fn read_batch(
    recording: Arc<Recording>,
    mut request: BatchRequest,
    limits: Limits,
    request_id: u64,
) -> Result<BatchPage, ServerError> {
    validate_request(&recording, &mut request, &limits)?;
    let stream_set: BTreeSet<_> = request.stream_ids.iter().copied().collect();
    let topics = recording.topics_for_streams(&stream_set)?;
    let resume_ordinal = validate_cursor(&recording, &request)?;
    let mut metrics = RequestMetrics::new(request_id);

    let recording_start = recording.catalog.time_range.start_ns.get();
    let recording_end = recording.catalog.time_range.end_ns_exclusive.get();
    let start_ns = request.start_ns.max(recording_start);
    let end_ns = request.end_ns.min(recording_end);
    if start_ns >= end_ns {
        return Ok(BatchPage {
            body: BatchEncoder::new().finish(),
            complete: true,
            next_cursor: None,
            message_count: 0,
            metrics,
        });
    }

    let options = IndexedReaderOptions::new()
        .with_order(ReadOrder::LogTime)
        .include_topics(topics)
        .log_time_on_or_after(start_ns)
        .log_time_before(end_ns)
        .with_record_length_limit(limits.max_chunk_bytes);
    let mut reader =
        IndexedReader::new_with_options(&recording.summary, options).map_err(|error| {
            ServerError::internal("could not initialize indexed MCAP reader", error)
        })?;
    let mut pager = PageBuilder::new(
        resume_ordinal,
        request.max_bytes,
        request.max_messages,
        limits.max_response_bytes,
    );
    let filter_started = Instant::now();
    let mut group = Vec::new();
    let mut group_time = None;
    let mut source_order = 0u64;

    while let Some(event) = reader.next_event() {
        match event
            .map_err(|error| ServerError::internal("MCAP message iteration failed", error))?
        {
            IndexedReadEvent::ReadChunkRequest { offset, length } => {
                let compressed = recording
                    .reader
                    .read_exact_at(offset, length, limits.max_chunk_bytes, &mut metrics.reads)
                    .map_err(|error| ServerError::internal("MCAP Chunk read failed", error))?;
                let decompress_started = Instant::now();
                reader
                    .insert_chunk_record_data(offset, &compressed)
                    .map_err(|error| ServerError::internal("MCAP Chunk decode failed", error))?;
                metrics.chunk_decompress_ms += decompress_started.elapsed().as_secs_f64() * 1000.0;
                metrics.chunk_count += 1;
            }
            IndexedReadEvent::Message { header, data } => {
                let Some(&stream_id) = recording.channel_to_stream.get(&header.channel_id) else {
                    continue;
                };
                if !stream_set.contains(&stream_id) {
                    continue;
                }
                let message = OwnedMessage {
                    stream_id,
                    sequence: header.sequence,
                    log_time_ns: header.log_time,
                    publish_time_ns: header.publish_time,
                    source_order,
                    payload: data.to_vec(),
                };
                source_order = source_order.saturating_add(1);
                if group_time.is_some_and(|time| time != header.log_time)
                    && !pager.push_group(&mut group)?
                {
                    metrics.message_filter_ms = filter_started.elapsed().as_secs_f64() * 1000.0;
                    return pager.finish(recording.as_ref(), &request, metrics);
                }
                group_time = Some(header.log_time);
                group.push(message);
            }
        }
    }
    if !group.is_empty() && !pager.push_group(&mut group)? {
        metrics.message_filter_ms = filter_started.elapsed().as_secs_f64() * 1000.0;
        return pager.finish(recording.as_ref(), &request, metrics);
    }
    metrics.message_filter_ms = filter_started.elapsed().as_secs_f64() * 1000.0;
    if pager.ordinal < resume_ordinal {
        return Err(ServerError::bad_request(
            "invalid_cursor",
            "continuation cursor points beyond the query result",
        ));
    }
    pager.complete = true;
    pager.finish(recording.as_ref(), &request, metrics)
}

fn validate_request(
    recording: &Recording,
    request: &mut BatchRequest,
    limits: &Limits,
) -> Result<(), ServerError> {
    if request.revision != recording.revision {
        return Err(ServerError::conflict(
            "recording revision does not match the current catalog",
        ));
    }
    if request.start_ns >= request.end_ns {
        return Err(ServerError::bad_request(
            "invalid_time_range",
            "start_ns must be less than end_ns",
        ));
    }
    if request.end_ns - request.start_ns > limits.max_window_ns {
        return Err(ServerError::bad_request(
            "window_limit_exceeded",
            "requested time window exceeds max_window_ns",
        ));
    }
    if request.stream_ids.is_empty() {
        return Err(ServerError::bad_request(
            "missing_streams",
            "at least one stream ID is required",
        ));
    }
    request.stream_ids.sort_unstable();
    request.stream_ids.dedup();
    if request.max_bytes == 0 || request.max_messages == 0 {
        return Err(ServerError::bad_request(
            "invalid_limit",
            "max_bytes and max_messages must be positive",
        ));
    }
    if request.max_bytes > limits.max_response_bytes || request.max_messages > limits.max_messages {
        return Err(ServerError::bad_request(
            "limit_exceeded",
            "requested response limit exceeds the configured hard limit",
        ));
    }
    recording.topics_for_streams(&request.stream_ids.iter().copied().collect())?;
    Ok(())
}

fn validate_cursor(recording: &Recording, request: &BatchRequest) -> Result<u64, ServerError> {
    let Some(encoded) = &request.cursor else {
        return Ok(0);
    };
    let cursor = ContinuationCursor::decode(encoded).map_err(|error| {
        ServerError::bad_request(
            "invalid_cursor",
            format!("invalid continuation cursor: {error}"),
        )
    })?;
    if cursor.recording_revision != recording.revision {
        return Err(ServerError::conflict(
            "cursor recording revision does not match the current catalog",
        ));
    }
    if cursor.recording_id != recording.id
        || cursor.recording_revision != request.revision
        || cursor.start_ns != request.start_ns
        || cursor.end_ns != request.end_ns
        || cursor.stream_ids != request.stream_ids
    {
        return Err(ServerError::bad_request(
            "cursor_query_mismatch",
            "continuation cursor does not match this query",
        ));
    }
    Ok(cursor.next_ordinal)
}

struct PageBuilder {
    encoder: BatchEncoder,
    resume_ordinal: u64,
    ordinal: u64,
    returned: usize,
    max_bytes: usize,
    max_messages: usize,
    hard_max_bytes: usize,
    complete: bool,
}

impl PageBuilder {
    fn new(
        resume_ordinal: u64,
        max_bytes: usize,
        max_messages: usize,
        hard_max_bytes: usize,
    ) -> Self {
        Self {
            encoder: BatchEncoder::new(),
            resume_ordinal,
            ordinal: 0,
            returned: 0,
            max_bytes,
            max_messages,
            hard_max_bytes,
            complete: false,
        }
    }

    fn push_group(&mut self, group: &mut Vec<OwnedMessage>) -> Result<bool, ServerError> {
        group.sort_by_key(|message| (message.stream_id, message.source_order));
        for message in group.drain(..) {
            if self.ordinal < self.resume_ordinal {
                self.ordinal += 1;
                continue;
            }
            let frame_len = BatchEncoder::frame_len(message.payload.len())
                .map_err(|error| ServerError::too_large("message_too_large", error.to_string()))?;
            let hard_size = self
                .encoder
                .encoded_len()
                .checked_add(frame_len)
                .ok_or_else(|| {
                    ServerError::too_large("response_too_large", "batch size overflow")
                })?;
            if frame_len
                .checked_add(16)
                .is_none_or(|size| size > self.hard_max_bytes)
            {
                return Err(ServerError::too_large(
                    "message_too_large",
                    "a single message exceeds max_response_bytes",
                ));
            }
            if self.returned > 0
                && (self.returned >= self.max_messages || hard_size > self.max_bytes)
            {
                return Ok(false);
            }
            self.encoder
                .push(RemoteMessageRef {
                    stream_id: message.stream_id,
                    sequence: message.sequence,
                    log_time_ns: message.log_time_ns,
                    publish_time_ns: message.publish_time_ns,
                    payload: &message.payload,
                })
                .map_err(|error| ServerError::internal("batch encoding failed", error))?;
            self.returned += 1;
            self.ordinal += 1;
        }
        Ok(true)
    }

    fn finish(
        self,
        recording: &Recording,
        request: &BatchRequest,
        mut metrics: RequestMetrics,
    ) -> Result<BatchPage, ServerError> {
        let encode_started = Instant::now();
        let next_cursor = if self.complete {
            None
        } else {
            Some(
                ContinuationCursor::new(
                    recording.id.clone(),
                    recording.revision.clone(),
                    request.start_ns,
                    request.end_ns,
                    request.stream_ids.clone(),
                    self.ordinal,
                )
                .encode()
                .map_err(|error| ServerError::internal("cursor encoding failed", error))?,
            )
        };
        let body = self.encoder.finish();
        metrics.batch_encode_ms += encode_started.elapsed().as_secs_f64() * 1000.0;
        Ok(BatchPage {
            body,
            complete: self.complete,
            next_cursor,
            message_count: self.returned,
            metrics,
        })
    }
}

#[cfg(test)]
#[path = "batch_service_tests.rs"]
mod tests;
