use crate::data_plane::{DataLoadError, LoadedWindow, WindowLoadDiagnostics};
use bytes::Bytes;
use mcap::records::{ChunkIndex, Record, op};
#[cfg(target_arch = "wasm32")]
use std::collections::BTreeMap;
use std::collections::{BTreeSet, VecDeque};
use viewer_core::{ArrivalTime, DataWindowTimeRange, RawMessage, SerializedWindow, StreamId};

#[cfg(target_arch = "wasm32")]
use {
    super::LocalCatalog,
    crate::data_plane::{WindowLoader, WindowLoaderMetrics},
    js_sys::{Date, Uint8Array},
    mcap::sans_io::{SummaryReadEvent, SummaryReader, SummaryReaderOptions},
    std::{
        cell::{Cell, RefCell},
        io::SeekFrom,
        rc::Rc,
    },
    wasm_bindgen_futures::{JsFuture, spawn_local},
    web_sys::File,
};

#[cfg(target_arch = "wasm32")]
const SUMMARY_READ_AHEAD: usize = 256 * 1024;
const JS_MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ByteRange {
    offset: u64,
    length: usize,
}

impl ByteRange {
    fn end(self) -> u64 {
        self.offset + self.length as u64
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ChunkReadRequest {
    offset: u64,
    length: usize,
    compression: String,
    uncompressed_size: usize,
}

struct OwnedWindowCollector {
    pending_chunks: VecDeque<ChunkReadRequest>,
    selected_channels: BTreeSet<u16>,
    range: DataWindowTimeRange,
    messages: Vec<RawMessage>,
    resident_bytes: usize,
    decompressed_bytes: usize,
}

impl OwnedWindowCollector {
    fn new(
        summary: &mcap::Summary,
        topics: &[String],
        range: DataWindowTimeRange,
    ) -> Result<Self, DataLoadError> {
        let start = u64::try_from(range.start.0)
            .map_err(|_| DataLoadError::invalid_record("negative browser MCAP window start"))?;
        let end = u64::try_from(range.end_exclusive.0)
            .map_err(|_| DataLoadError::invalid_record("negative browser MCAP window end"))?;
        let selected_channels = summary
            .channels
            .iter()
            .filter_map(|(id, channel)| topics.contains(&channel.topic).then_some(*id))
            .collect::<BTreeSet<_>>();
        let mut chunks = summary
            .chunk_indexes
            .iter()
            .filter(|chunk| chunk.message_end_time >= start)
            .filter(|chunk| chunk.message_start_time < end)
            .filter(|chunk| {
                !selected_channels.is_empty()
                    && (chunk.message_index_offsets.is_empty()
                        || chunk
                            .message_index_offsets
                            .keys()
                            .any(|id| selected_channels.contains(id)))
            })
            .collect::<Vec<_>>();
        chunks.sort_by_key(|chunk| (chunk.message_start_time, chunk.chunk_start_offset));
        let pending_chunks = chunks
            .into_iter()
            .map(chunk_request)
            .collect::<Result<VecDeque<_>, _>>()?;
        Ok(Self {
            pending_chunks,
            selected_channels,
            range,
            messages: Vec::new(),
            resident_bytes: 0,
            decompressed_bytes: 0,
        })
    }

    fn next_read(&mut self) -> Option<ChunkReadRequest> {
        self.pending_chunks.pop_front()
    }

    fn insert_chunk(
        &mut self,
        request: &ChunkReadRequest,
        compressed: Bytes,
    ) -> Result<(), DataLoadError> {
        if compressed.len() != request.length {
            return Err(DataLoadError::invalid_record(
                "browser MCAP chunk range is truncated",
            ));
        }
        let (backing, decompressed_bytes) = decode_chunk_backing(request, compressed)?;
        self.decompressed_bytes = self
            .decompressed_bytes
            .checked_add(decompressed_bytes)
            .ok_or_else(|| DataLoadError::invalid_record("decompressed byte count overflow"))?;
        let parsed = parse_chunk_backing(backing, &self.selected_channels, self.range, |_, _| {
            Ok(true)
        })?;
        if !parsed.messages.is_empty() {
            self.resident_bytes = self
                .resident_bytes
                .checked_add(parsed.backing.len())
                .ok_or_else(|| {
                    DataLoadError::invalid_record("local resident byte count overflow")
                })?;
        }
        self.messages.extend(parsed.messages);
        Ok(())
    }

    fn finish(
        mut self,
        mut diagnostics: WindowLoadDiagnostics,
    ) -> Result<LoadedWindow, DataLoadError> {
        self.messages
            .sort_by_key(|message| (message.arrival_time, message.stream_id.0));
        diagnostics.decompressed_bytes = self.decompressed_bytes;
        diagnostics.per_message_copied_bytes = 0;
        let window = SerializedWindow::new(self.range, self.messages, self.resident_bytes)
            .map_err(DataLoadError::from)?;
        Ok(LoadedWindow {
            window,
            diagnostics,
        })
    }
}

fn decode_chunk_backing(
    request: &ChunkReadRequest,
    compressed: Bytes,
) -> Result<(Bytes, usize), DataLoadError> {
    match request.compression.as_str() {
        "" => {
            if compressed.len() != request.uncompressed_size {
                return Err(DataLoadError::invalid_record(
                    "uncompressed MCAP chunk size mismatch",
                ));
            }
            Ok((compressed, 0))
        }
        "zstd" => {
            let decompressed = zstd::bulk::decompress(&compressed, request.uncompressed_size)
                .map_err(DataLoadError::from)?;
            if decompressed.len() != request.uncompressed_size {
                return Err(DataLoadError::invalid_record(
                    "decompressed MCAP chunk size mismatch",
                ));
            }
            let decompressed_bytes = decompressed.len();
            Ok((Bytes::from(decompressed), decompressed_bytes))
        }
        compression => Err(DataLoadError::invalid_record(format!(
            "unsupported browser MCAP compression: {compression}"
        ))),
    }
}

fn chunk_request(index: &ChunkIndex) -> Result<ChunkReadRequest, DataLoadError> {
    Ok(ChunkReadRequest {
        offset: index
            .compressed_data_offset()
            .map_err(DataLoadError::from)?,
        length: usize::try_from(index.compressed_size)
            .map_err(|_| DataLoadError::invalid_record("compressed MCAP chunk is too large"))?,
        compression: index.compression.clone(),
        uncompressed_size: usize::try_from(index.uncompressed_size)
            .map_err(|_| DataLoadError::invalid_record("uncompressed MCAP chunk is too large"))?,
    })
}

struct ParsedChunk {
    backing: Bytes,
    messages: Vec<RawMessage>,
    #[cfg(test)]
    message_offsets: Vec<u64>,
}

fn parse_chunk_backing(
    backing: Bytes,
    selected_channels: &BTreeSet<u16>,
    range: DataWindowTimeRange,
    mut admit: impl FnMut(&mcap::records::MessageHeader, u64) -> Result<bool, DataLoadError>,
) -> Result<ParsedChunk, DataLoadError> {
    let mut offset = 0_usize;
    let mut messages = Vec::new();
    #[cfg(test)]
    let mut message_offsets = Vec::new();
    while offset < backing.len() {
        let record_offset = u64::try_from(offset)
            .map_err(|_| DataLoadError::invalid_record("MCAP Chunk record offset overflow"))?;
        let header_end = offset
            .checked_add(9)
            .ok_or_else(|| DataLoadError::invalid_record("MCAP record header overflow"))?;
        if header_end > backing.len() {
            return Err(DataLoadError::invalid_record(
                "truncated MCAP record header in chunk",
            ));
        }
        let opcode = backing[offset];
        let length_bytes: [u8; 8] = backing[offset + 1..header_end]
            .try_into()
            .expect("record length slice has eight bytes");
        let body_length = usize::try_from(u64::from_le_bytes(length_bytes))
            .map_err(|_| DataLoadError::invalid_record("MCAP record body is too large"))?;
        let body_end = header_end
            .checked_add(body_length)
            .ok_or_else(|| DataLoadError::invalid_record("MCAP record body overflow"))?;
        if body_end > backing.len() {
            return Err(DataLoadError::invalid_record(
                "truncated MCAP record body in chunk",
            ));
        }
        if opcode == op::MESSAGE {
            let record = mcap::parse_record(opcode, &backing[header_end..body_end])
                .map_err(DataLoadError::from)?;
            let Record::Message { header, data } = record else {
                return Err(DataLoadError::invalid_record(
                    "MCAP message opcode parsed as another record",
                ));
            };
            let arrival = i64::try_from(header.log_time).map_err(|_| {
                DataLoadError::invalid_record("browser MCAP timestamp exceeds signed nanoseconds")
            })?;
            let arrival_time = ArrivalTime(arrival);
            if range.contains(arrival_time)
                && (selected_channels.is_empty() || selected_channels.contains(&header.channel_id))
                && admit(&header, record_offset)?
            {
                let payload_start = body_end.checked_sub(data.len()).ok_or_else(|| {
                    DataLoadError::invalid_record("MCAP message payload range underflow")
                })?;
                messages.push(RawMessage {
                    publish_time_ns: Some(header.publish_time),
                    sequence: Some(header.sequence),
                    stream_id: StreamId(u32::from(header.channel_id)),
                    arrival_time,
                    payload: backing.slice(payload_start..body_end),
                });
                #[cfg(test)]
                message_offsets.push(record_offset);
            }
        }
        offset = body_end;
    }
    Ok(ParsedChunk {
        backing,
        messages,
        #[cfg(test)]
        message_offsets,
    })
}

fn validate_range(file_size: u64, offset: u64, length: usize) -> Result<ByteRange, DataLoadError> {
    if file_size == 0 {
        return Err(DataLoadError::invalid_record("MCAP file is empty"));
    }
    if file_size > JS_MAX_SAFE_INTEGER {
        return Err(DataLoadError::invalid_record(
            "MCAP file size exceeds JavaScript's safe integer range",
        ));
    }
    let end = offset
        .checked_add(length as u64)
        .ok_or_else(|| DataLoadError::invalid_record("MCAP range end overflow"))?;
    if offset > JS_MAX_SAFE_INTEGER || end > JS_MAX_SAFE_INTEGER || end > file_size {
        return Err(DataLoadError::invalid_record(format!(
            "MCAP range {offset}..{end} exceeds file size {file_size}"
        )));
    }
    Ok(ByteRange { offset, length })
}

#[cfg(test)]
pub(crate) fn collect_window_from_bytes_for_test(
    summary: &mcap::Summary,
    topics: &[String],
    range: DataWindowTimeRange,
    bytes: &[u8],
) -> Result<LoadedWindow, DataLoadError> {
    let mut collector = OwnedWindowCollector::new(summary, topics, range)?;
    let mut source_bytes = 0_usize;
    let mut source_reads = 0_u64;
    while let Some(request) = collector.next_read() {
        let range = validate_range(bytes.len() as u64, request.offset, request.length)?;
        let chunk = &bytes[range.offset as usize..range.end() as usize];
        source_bytes = source_bytes
            .checked_add(chunk.len())
            .ok_or_else(|| DataLoadError::invalid_record("test source byte count overflow"))?;
        source_reads = source_reads.saturating_add(1);
        collector.insert_chunk(&request, Bytes::copy_from_slice(chunk))?;
    }
    collector.finish(WindowLoadDiagnostics {
        source_reads,
        source_bytes,
        decompressed_bytes: 0,
        per_message_copied_bytes: 0,
        latency_ms: 0.0,
        processing_ms: 0.0,
    })
}

#[cfg(target_arch = "wasm32")]
#[derive(Debug)]
enum LocalLoadState {
    Idle,
    Loading {
        generation: u64,
        range: DataWindowTimeRange,
    },
    Failed(DataLoadError),
}

#[cfg(target_arch = "wasm32")]
#[derive(Debug)]
struct LocalFetchResult {
    generation: u64,
    range: DataWindowTimeRange,
    result: Result<LoadedWindow, DataLoadError>,
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct BrowserMcapWindowLoader {
    file: File,
    file_size: u64,
    summary: Rc<mcap::Summary>,
    selected_topics: Vec<String>,
    generation: u64,
    state: LocalLoadState,
    inbox: Rc<RefCell<VecDeque<LocalFetchResult>>>,
    metrics: WindowLoaderMetrics,
}

#[cfg(target_arch = "wasm32")]
impl BrowserMcapWindowLoader {
    fn new(
        file: File,
        file_size: u64,
        summary: Rc<mcap::Summary>,
        selected_topics: Vec<String>,
        catalog_reads: u64,
        catalog_bytes: u64,
        catalog_latency_ms: f64,
    ) -> Self {
        Self {
            file,
            file_size,
            summary,
            selected_topics,
            generation: 0,
            state: LocalLoadState::Idle,
            inbox: Rc::new(RefCell::new(VecDeque::new())),
            metrics: WindowLoaderMetrics {
                source_reads: catalog_reads,
                source_bytes: catalog_bytes,
                request_latency_ms: catalog_latency_ms,
                ..WindowLoaderMetrics::default()
            },
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl WindowLoader for BrowserMcapWindowLoader {
    fn request(&mut self, range: DataWindowTimeRange) -> Result<(), DataLoadError> {
        if !matches!(self.state, LocalLoadState::Idle) {
            return Err(DataLoadError::invalid_record(
                "browser MCAP window loader is not idle",
            ));
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.state = LocalLoadState::Loading { generation, range };
        self.metrics.load_requests = self.metrics.load_requests.saturating_add(1);
        let file = self.file.clone();
        let file_size = self.file_size;
        let summary = Rc::clone(&self.summary);
        let selected_topics = self.selected_topics.clone();
        let inbox = Rc::clone(&self.inbox);
        spawn_local(async move {
            let result =
                load_browser_window(&file, file_size, &summary, &selected_topics, range).await;
            inbox.borrow_mut().push_back(LocalFetchResult {
                generation,
                range,
                result,
            });
        });
        Ok(())
    }

    fn poll(&mut self) -> Option<Result<LoadedWindow, DataLoadError>> {
        if let LocalLoadState::Failed(error) = &self.state {
            return Some(Err(error.clone()));
        }
        loop {
            let result = self.inbox.borrow_mut().pop_front()?;
            let current = matches!(
                self.state,
                LocalLoadState::Loading { generation, range }
                    if generation == result.generation && range == result.range
            );
            if result.generation != self.generation || !current {
                self.metrics.stale_results_discarded =
                    self.metrics.stale_results_discarded.saturating_add(1);
                continue;
            }
            match result.result {
                Ok(loaded) => {
                    self.metrics.source_reads = self
                        .metrics
                        .source_reads
                        .saturating_add(loaded.diagnostics.source_reads);
                    self.metrics.source_bytes = self
                        .metrics
                        .source_bytes
                        .saturating_add(loaded.diagnostics.source_bytes as u64);
                    self.metrics.decompressed_bytes = self
                        .metrics
                        .decompressed_bytes
                        .saturating_add(loaded.diagnostics.decompressed_bytes as u64);
                    self.metrics.per_message_copied_bytes = self
                        .metrics
                        .per_message_copied_bytes
                        .saturating_add(loaded.diagnostics.per_message_copied_bytes as u64);
                    self.metrics.messages_loaded = self
                        .metrics
                        .messages_loaded
                        .saturating_add(loaded.window.messages.len() as u64);
                    self.metrics.last_window_latency_ms = loaded.diagnostics.latency_ms;
                    self.metrics.last_processing_ms = loaded.diagnostics.processing_ms;
                    self.metrics.request_latency_ms = loaded.diagnostics.latency_ms;
                    self.state = LocalLoadState::Idle;
                    return Some(Ok(loaded));
                }
                Err(error) => {
                    self.state = LocalLoadState::Failed(error.clone());
                    return Some(Err(error));
                }
            }
        }
    }

    fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.state = LocalLoadState::Idle;
    }

    fn is_idle(&self) -> bool {
        matches!(self.state, LocalLoadState::Idle)
    }

    fn metrics(&self) -> &WindowLoaderMetrics {
        &self.metrics
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for BrowserMcapWindowLoader {
    fn drop(&mut self) {
        WindowLoader::cancel(self);
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct BrowserMcapRecording {
    pub catalog: LocalCatalog,
    pub loader: BrowserMcapWindowLoader,
    pub restore: BrowserMcapRestoreLoader,
}

#[cfg(target_arch = "wasm32")]
type RestoreInbox = Rc<RefCell<VecDeque<(u64, Result<Vec<RawMessage>, DataLoadError>)>>>;

#[cfg(target_arch = "wasm32")]
pub(crate) struct BrowserMcapRestoreLoader {
    file: File,
    file_size: u64,
    summary: Rc<mcap::Summary>,
    generation: Rc<Cell<u64>>,
    loading: bool,
    inbox: RestoreInbox,
}

#[cfg(target_arch = "wasm32")]
impl BrowserMcapRestoreLoader {
    pub(crate) fn fork(&self) -> Self {
        Self::new(self.file.clone(), self.file_size, self.summary.clone())
    }
    fn new(file: File, file_size: u64, summary: Rc<mcap::Summary>) -> Self {
        Self {
            file,
            file_size,
            summary,
            generation: Rc::new(Cell::new(0)),
            loading: false,
            inbox: Rc::new(RefCell::new(VecDeque::new())),
        }
    }

    pub(crate) fn request(&mut self, plan: viewer_core::RestorePlan) -> Result<(), DataLoadError> {
        self.cancel();
        self.loading = true;
        let generation = self.generation.get();
        let cancellation = self.generation.clone();
        let file = self.file.clone();
        let file_size = self.file_size;
        let summary = Rc::clone(&self.summary);
        let inbox = Rc::clone(&self.inbox);
        spawn_local(async move {
            let mut work = RestoreWork::new(cancellation.clone(), generation);
            let result = load_browser_restore(&file, file_size, &summary, &plan, &mut work).await;
            if cancellation.get() == generation {
                inbox.borrow_mut().push_back((generation, result));
            }
        });
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> Option<Result<Vec<RawMessage>, DataLoadError>> {
        loop {
            let (generation, result) = self.inbox.borrow_mut().pop_front()?;
            if generation != self.generation.get() || !self.loading {
                continue;
            }
            self.loading = false;
            return Some(result);
        }
    }

    pub(crate) fn cancel(&mut self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.loading = false;
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for BrowserMcapRestoreLoader {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
struct RestoreWork {
    generation: std::rc::Rc<std::cell::Cell<u64>>,
    expected: u64,
    index_bytes: usize,
    index_reads: usize,
    chunk_bytes: usize,
    messages: usize,
}

#[cfg(any(test, target_arch = "wasm32"))]
impl RestoreWork {
    fn new(generation: std::rc::Rc<std::cell::Cell<u64>>, expected: u64) -> Self {
        Self {
            generation,
            expected,
            index_bytes: 0,
            index_reads: 0,
            chunk_bytes: 0,
            messages: 0,
        }
    }
    fn check(&self) -> Result<(), DataLoadError> {
        if self.generation.get() != self.expected {
            return Err(DataLoadError::invalid_record(
                "browser indexed query cancelled",
            ));
        }
        Ok(())
    }
    fn reserve_index(&mut self, length: usize) -> Result<(), DataLoadError> {
        self.check()?;
        if self.index_reads >= 4096
            || length > (32 * 1024 * 1024_usize).saturating_sub(self.index_bytes)
        {
            return Err(DataLoadError::invalid_record(
                "browser indexed query exceeds index read/byte budget",
            ));
        }
        self.index_reads += 1;
        self.index_bytes += length;
        Ok(())
    }
    fn reserve_chunk(
        &mut self,
        compressed: usize,
        uncompressed: usize,
    ) -> Result<(), DataLoadError> {
        self.check()?;
        let bytes = compressed
            .checked_add(uncompressed)
            .ok_or_else(|| DataLoadError::invalid_record("browser indexed query byte overflow"))?;
        if bytes > (256 * 1024 * 1024_usize).saturating_sub(self.chunk_bytes) {
            return Err(DataLoadError::invalid_record(
                "browser indexed query exceeds chunk byte budget",
            ));
        }
        self.chunk_bytes += bytes;
        Ok(())
    }

    fn admit_message(&mut self) -> Result<(), DataLoadError> {
        self.check()?;
        if self.messages >= 200_000 {
            return Err(DataLoadError::invalid_record(
                "browser indexed query exceeds message budget",
            ));
        }
        self.messages += 1;
        Ok(())
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
fn index_predecessor(
    entries: &[mcap::records::MessageIndexEntry],
    target: u64,
) -> Option<(u64, u64)> {
    entries
        .iter()
        .filter(|entry| entry.log_time <= target)
        .map(|entry| (entry.log_time, entry.offset))
        .max()
}

#[cfg(any(test, target_arch = "wasm32"))]
fn improve_predecessor(
    best: &mut Option<(usize, u64, u64)>,
    chunk: usize,
    entries: &[mcap::records::MessageIndexEntry],
    target: u64,
) {
    if let Some((time, offset)) = index_predecessor(entries, target)
        && best.is_none_or(|(best_chunk, best_time, best_offset)| {
            (time, chunk, offset) > (best_time, best_chunk, best_offset)
        })
    {
        *best = Some((chunk, time, offset));
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) async fn open_browser_mcap(
    file: File,
    primary_camera_topic: &str,
) -> Result<BrowserMcapRecording, DataLoadError> {
    let file_size = browser_file_size(&file)?;
    let started = Date::now();
    let (summary, reads, bytes) = read_browser_summary(&file, file_size).await?;
    let latency_ms = Date::now() - started;
    let catalog =
        LocalCatalog::from_summary(&summary, primary_camera_topic).map_err(DataLoadError::from)?;
    let selected_topics = catalog.selected_topics.clone();
    let summary = Rc::new(summary);
    let loader = BrowserMcapWindowLoader::new(
        file,
        file_size,
        Rc::clone(&summary),
        selected_topics,
        reads,
        bytes,
        latency_ms,
    );
    let restore = BrowserMcapRestoreLoader::new(loader.file.clone(), file_size, summary);
    Ok(BrowserMcapRecording {
        catalog,
        loader,
        restore,
    })
}

#[cfg(any(test, target_arch = "wasm32"))]
#[derive(Default)]
struct RestoreChunkSelection {
    exact: Vec<(u16, u64, u64)>,
    history: std::collections::HashMap<u16, i64>,
    persistent: BTreeSet<u16>,
}

#[cfg(any(test, target_arch = "wasm32"))]
impl RestoreChunkSelection {
    fn contains(
        &self,
        header: &mcap::records::MessageHeader,
        offset: u64,
        target: ArrivalTime,
    ) -> bool {
        self.exact
            .contains(&(header.channel_id, header.log_time, offset))
            || self.history.get(&header.channel_id).is_some_and(|start| {
                i64::try_from(header.log_time).is_ok_and(|time| time >= *start && time <= target.0)
            })
            || self.persistent.contains(&header.channel_id)
    }
}

#[cfg(target_arch = "wasm32")]
async fn load_browser_restore(
    file: &File,
    file_size: u64,
    summary: &mcap::Summary,
    plan: &viewer_core::RestorePlan,
    work: &mut RestoreWork,
) -> Result<Vec<RawMessage>, DataLoadError> {
    let selections = resolve_browser_restore_chunks(file, file_size, summary, plan, work).await?;
    let mut messages = Vec::new();
    for (chunk_index, selection) in selections {
        let restored = read_browser_restore_chunk(
            file,
            file_size,
            summary,
            &summary.chunk_indexes[chunk_index],
            &selection,
            plan.target,
            work,
        )
        .await?;
        messages.extend(restored);
    }
    messages.sort_by_key(|message| (message.arrival_time, message.stream_id.0));
    work.check()?;
    Ok(messages)
}

#[cfg(target_arch = "wasm32")]
async fn resolve_browser_restore_chunks(
    file: &File,
    file_size: u64,
    summary: &mcap::Summary,
    plan: &viewer_core::RestorePlan,
    work: &mut RestoreWork,
) -> Result<BTreeMap<usize, RestoreChunkSelection>, DataLoadError> {
    let target = u64::try_from(plan.target.0)
        .map_err(|_| DataLoadError::invalid_record("negative browser restore target"))?;
    let index_facts = summary
        .chunk_indexes
        .iter()
        .map(|chunk| viewer_core::IndexedChunkFact {
            start: ArrivalTime(i64::try_from(chunk.message_start_time).unwrap_or(i64::MAX)),
            end_inclusive: ArrivalTime(i64::try_from(chunk.message_end_time).unwrap_or(i64::MAX)),
            indexed_streams: chunk
                .message_index_offsets
                .keys()
                .map(|channel| StreamId(u32::from(*channel)))
                .collect(),
        })
        .collect::<Vec<_>>();
    let mut needs = BTreeMap::<usize, RestoreChunkSelection>::new();
    for stream in &plan.latest_before {
        let channel = u16::try_from(stream.0).map_err(|_| {
            DataLoadError::invalid_record("local stream ID exceeds MCAP channel ID")
        })?;
        let mut found = None::<(usize, u64, u64)>;
        let count = summary
            .stats
            .as_ref()
            .and_then(|stats| stats.channel_message_counts.get(&channel).copied());
        for chunk_index in
            viewer_core::latest_candidate_chunks(&index_facts, *stream, count, plan.target)
                .map_err(DataLoadError::from)?
        {
            let chunk = &summary.chunk_indexes[chunk_index];
            work.check()?;
            if found.is_some_and(|(best_chunk, best_time, _)| {
                chunk.message_end_time < best_time
                    || (chunk.message_end_time == best_time && chunk_index < best_chunk)
            }) {
                continue;
            }
            let offset = chunk.message_index_offsets[&channel];
            let entries = read_message_index(file, file_size, channel, offset, work).await?;
            improve_predecessor(&mut found, chunk_index, &entries, target);
        }
        let Some((chunk, time, offset)) = found else {
            continue;
        };
        needs
            .entry(chunk)
            .or_default()
            .exact
            .push((channel, time, offset));
    }
    for read in &plan.histories {
        for stream in &read.streams {
            let channel = u16::try_from(stream.0).map_err(|_| {
                DataLoadError::invalid_record("local stream ID exceeds MCAP channel ID")
            })?;
            let count = summary
                .stats
                .as_ref()
                .and_then(|stats| stats.channel_message_counts.get(&channel).copied());
            for chunk_index in
                viewer_core::history_candidate_chunks(&index_facts, *stream, count, read.range)
                    .map_err(DataLoadError::from)?
            {
                needs
                    .entry(chunk_index)
                    .or_default()
                    .history
                    .insert(channel, read.range.start.0);
            }
        }
    }
    for stream in &plan.persistent {
        let channel = u16::try_from(stream.0).map_err(|_| {
            DataLoadError::invalid_record("local stream ID exceeds MCAP channel ID")
        })?;
        let count = summary
            .stats
            .as_ref()
            .and_then(|stats| stats.channel_message_counts.get(&channel).copied());
        for chunk_index in viewer_core::persistent_candidate_chunks(&index_facts, *stream, count)
            .map_err(DataLoadError::from)?
        {
            needs
                .entry(chunk_index)
                .or_default()
                .persistent
                .insert(channel);
        }
    }

    Ok(needs)
}

#[cfg(target_arch = "wasm32")]
async fn read_browser_restore_chunk(
    file: &File,
    file_size: u64,
    summary: &mcap::Summary,
    chunk: &ChunkIndex,
    selection: &RestoreChunkSelection,
    target: ArrivalTime,
    work: &mut RestoreWork,
) -> Result<Vec<RawMessage>, DataLoadError> {
    let request = chunk_request(chunk)?;
    work.reserve_chunk(request.length, request.uncompressed_size)?;
    let range = validate_range(file_size, request.offset, request.length)?;
    let compressed = read_file_range(file, file_size, range).await?;
    work.check()?;
    let (backing, _) = decode_chunk_backing(&request, compressed)?;
    let channels = selection
        .exact
        .iter()
        .map(|(channel, _, _)| *channel)
        .chain(selection.history.keys().copied())
        .chain(selection.persistent.iter().copied())
        .collect::<BTreeSet<_>>();
    let recording_start = summary
        .chunk_indexes
        .iter()
        .map(|chunk| chunk.message_start_time)
        .min()
        .unwrap_or(0);
    let recording_end_exclusive = summary
        .chunk_indexes
        .iter()
        .map(|chunk| chunk.message_end_time)
        .max()
        .unwrap_or(target.0 as u64)
        .checked_add(1)
        .ok_or_else(|| DataLoadError::invalid_record("browser MCAP end time overflow"))?;
    let mut exact_matches = 0;
    let parsed = parse_chunk_backing(
        backing,
        &channels,
        DataWindowTimeRange::new(
            ArrivalTime(i64::try_from(recording_start).unwrap_or(0)),
            ArrivalTime(
                i64::try_from(recording_end_exclusive)
                    .map_err(|_| DataLoadError::invalid_record("browser MCAP end time overflow"))?,
            ),
        )
        .map_err(DataLoadError::from)?,
        |header, offset| {
            if !selection.contains(header, offset, target) {
                return Ok(false);
            }
            if selection
                .exact
                .contains(&(header.channel_id, header.log_time, offset))
            {
                exact_matches += 1;
            }
            work.admit_message()?;
            Ok(true)
        },
    )?;
    if exact_matches != selection.exact.len() {
        return Err(DataLoadError::invalid_record(
            "Message Index does not identify the expected chunk records",
        ));
    }
    Ok(parsed.messages)
}

#[cfg(target_arch = "wasm32")]
async fn read_message_index(
    file: &File,
    file_size: u64,
    channel: u16,
    offset: u64,
    work: &mut RestoreWork,
) -> Result<Vec<mcap::records::MessageIndexEntry>, DataLoadError> {
    work.reserve_index(9)?;
    let header = read_file_range(file, file_size, validate_range(file_size, offset, 9)?).await?;
    work.check()?;
    if header[0] != op::MESSAGE_INDEX {
        return Err(DataLoadError::invalid_record(
            "Message Index offset points to another record",
        ));
    }
    let length = usize::try_from(u64::from_le_bytes(
        header[1..9]
            .try_into()
            .expect("record header has eight length bytes"),
    ))
    .map_err(|_| DataLoadError::invalid_record("Message Index record is too large"))?;
    work.reserve_index(length)?;
    let body = read_file_range(
        file,
        file_size,
        validate_range(file_size, offset.saturating_add(9), length)?,
    )
    .await?;
    work.check()?;
    match mcap::parse_record(op::MESSAGE_INDEX, &body).map_err(DataLoadError::from)? {
        Record::MessageIndex(index) if index.channel_id == channel => Ok(index.records),
        _ => Err(DataLoadError::invalid_record(
            "Message Index parsed as another record",
        )),
    }
}

#[cfg(target_arch = "wasm32")]
async fn load_browser_window(
    file: &File,
    file_size: u64,
    summary: &mcap::Summary,
    topics: &[String],
    range: DataWindowTimeRange,
) -> Result<LoadedWindow, DataLoadError> {
    let started = Date::now();
    let mut collector = OwnedWindowCollector::new(summary, topics, range)?;
    let mut source_reads = 0_u64;
    let mut source_bytes = 0_usize;
    let mut processing_ms = 0.0;
    while let Some(request) = collector.next_read() {
        let range = validate_range(file_size, request.offset, request.length)?;
        let bytes = read_file_range(file, file_size, range).await?;
        source_reads = source_reads.saturating_add(1);
        source_bytes = source_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| DataLoadError::invalid_record("local source byte count overflow"))?;
        let processing_started = Date::now();
        collector.insert_chunk(&request, bytes)?;
        processing_ms += Date::now() - processing_started;
    }
    let per_message_copied_bytes = collector.resident_bytes;
    collector.finish(WindowLoadDiagnostics {
        source_reads,
        source_bytes,
        decompressed_bytes: 0,
        per_message_copied_bytes,
        latency_ms: Date::now() - started,
        processing_ms,
    })
}

#[cfg(target_arch = "wasm32")]
async fn read_browser_summary(
    file: &File,
    file_size: u64,
) -> Result<(mcap::Summary, u64, u64), DataLoadError> {
    let mut reader =
        SummaryReader::new_with_options(SummaryReaderOptions::default().with_file_size(file_size));
    let mut position = 0_u64;
    let mut seek_count = 0_u32;
    let mut reads = 0_u64;
    let mut bytes_read = 0_u64;
    while let Some(event) = reader.next_event() {
        match event.map_err(DataLoadError::from)? {
            SummaryReadEvent::SeekRequest(seek) => {
                position = resolve_seek(file_size, position, seek)?;
                seek_count = seek_count.saturating_add(1);
                reader.notify_seeked(position);
            }
            SummaryReadEvent::ReadRequest(need) => {
                let remaining =
                    usize::try_from(file_size.saturating_sub(position)).unwrap_or(usize::MAX);
                let requested = if seek_count >= 2 {
                    need.max(SUMMARY_READ_AHEAD).min(remaining)
                } else {
                    need
                };
                let range = validate_range(file_size, position, requested)?;
                let bytes = read_file_range(file, file_size, range).await?;
                reader.insert(bytes.len()).copy_from_slice(&bytes);
                reader.notify_read(bytes.len());
                position = position.checked_add(bytes.len() as u64).ok_or_else(|| {
                    DataLoadError::invalid_record("SummaryReader position overflow")
                })?;
                reads = reads.saturating_add(1);
                bytes_read = bytes_read.saturating_add(bytes.len() as u64);
            }
        }
    }
    let summary = reader
        .finish()
        .ok_or_else(|| DataLoadError::invalid_record("MCAP has no Summary section"))?;
    Ok((summary, reads, bytes_read))
}

#[cfg(target_arch = "wasm32")]
async fn read_file_range(
    file: &File,
    file_size: u64,
    range: ByteRange,
) -> Result<Bytes, DataLoadError> {
    let range = validate_range(file_size, range.offset, range.length)?;
    if range.length == 0 {
        return Ok(Bytes::new());
    }
    if range.length > u32::MAX as usize {
        return Err(DataLoadError::invalid_record(
            "MCAP range exceeds Uint8Array addressable length",
        ));
    }
    let blob = file
        .slice_with_f64_and_f64(range.offset as f64, range.end() as f64)
        .map_err(js_error)?;
    let buffer = JsFuture::from(blob.array_buffer())
        .await
        .map_err(js_error)?;
    let bytes = Uint8Array::new(&buffer);
    if bytes.length() as usize != range.length {
        return Err(DataLoadError::invalid_record(format!(
            "short MCAP range read: requested {}, received {}",
            range.length,
            bytes.length()
        )));
    }
    Ok(Bytes::from(bytes.to_vec()))
}

#[cfg(target_arch = "wasm32")]
fn browser_file_size(file: &File) -> Result<u64, DataLoadError> {
    let size = file.size();
    if !size.is_finite() || size < 0.0 || size.fract() != 0.0 {
        return Err(DataLoadError::invalid_record(format!(
            "browser returned invalid File.size: {size}"
        )));
    }
    let size = size as u64;
    validate_range(size, 0, 0)?;
    Ok(size)
}

#[cfg(target_arch = "wasm32")]
fn resolve_seek(file_size: u64, current: u64, seek: SeekFrom) -> Result<u64, DataLoadError> {
    let position = match seek {
        SeekFrom::Start(position) => i128::from(position),
        SeekFrom::Current(offset) => i128::from(current) + i128::from(offset),
        SeekFrom::End(offset) => i128::from(file_size) + i128::from(offset),
    };
    if position < 0 || position > i128::from(file_size) {
        return Err(DataLoadError::invalid_record(
            "SummaryReader seek is outside the MCAP file",
        ));
    }
    u64::try_from(position)
        .map_err(|_| DataLoadError::invalid_record("SummaryReader seek overflow"))
}

#[cfg(target_arch = "wasm32")]
fn js_error(error: wasm_bindgen::JsValue) -> DataLoadError {
    DataLoadError::Browser(std::rc::Rc::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_query_work_stops_on_cancellation_and_reserves_before_io() {
        let generation = std::rc::Rc::new(std::cell::Cell::new(7));
        let mut work = RestoreWork::new(generation.clone(), 7);
        work.reserve_index(32 * 1024 * 1024).unwrap();
        assert!(work.reserve_index(1).is_err());
        work.reserve_chunk(128 * 1024 * 1024, 128 * 1024 * 1024)
            .unwrap();
        assert!(work.reserve_chunk(0, 1).is_err());
        generation.set(8);
        assert!(work.check().is_err());
        assert!(work.reserve_chunk(0, 0).is_err());
        let mut work = RestoreWork::new(generation, 8);
        for _ in 0..4096 {
            work.reserve_index(0).unwrap();
        }
        assert!(work.reserve_index(0).is_err());
        work.messages = 200_000;
        assert!(work.admit_message().is_err());
    }

    #[test]
    fn index_lookup_is_inclusive_and_handles_unsorted_entries_and_offset_ties() {
        let entries = [(100, 1), (40, 3), (90, 2), (90, 7)]
            .map(|(log_time, offset)| mcap::records::MessageIndexEntry { log_time, offset });
        assert_eq!(index_predecessor(&entries, 90), Some((90, 7)));
        assert_eq!(index_predecessor(&entries, 89), Some((40, 3)));
        assert_eq!(index_predecessor(&entries, 39), None);
    }

    #[test]
    fn overlapping_chunks_choose_the_actual_latest_with_native_chunk_and_offset_ties() {
        let mut best = None;
        for (chunk, entries) in [
            (2, vec![(65, 1)]),
            (1, vec![(55, 1)]),
            (0, vec![(1, 1), (90, 7), (90, 9), (100, 10)]),
        ] {
            let entries = entries
                .into_iter()
                .map(|(log_time, offset)| mcap::records::MessageIndexEntry { log_time, offset })
                .collect::<Vec<_>>();
            improve_predecessor(&mut best, chunk, &entries, 95);
        }
        assert_eq!(best, Some((0, 90, 9)));
        improve_predecessor(
            &mut best,
            3,
            &[mcap::records::MessageIndexEntry {
                log_time: 90,
                offset: 1,
            }],
            95,
        );
        assert_eq!(best, Some((3, 90, 1)));
    }
    use crate::local::LocalCatalog;
    use mcap::{Compression, Summary, WriteOptions, Writer, records::MessageHeader};
    use std::{collections::BTreeMap, io::Cursor};

    fn recording(compression: Option<Compression>) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        {
            let options = WriteOptions::new()
                .compression(compression)
                .chunk_size(Some(64));
            let mut writer = Writer::with_options(&mut output, options).unwrap();
            let schema = writer
                .add_schema("sensor_msgs/msg/CompressedImage", "ros2msg", b"schema")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/camera", "cdr", &BTreeMap::new())
                .unwrap();
            for sequence in 0..4 {
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence,
                            log_time: 1_000 + u64::from(sequence) * 10,
                            publish_time: 1_000 + u64::from(sequence) * 10,
                        },
                        &[sequence as u8; 16],
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        output.into_inner()
    }

    fn camera_recording(compression: Option<Compression>) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        {
            let options = WriteOptions::new()
                .compression(compression)
                .chunk_size(Some(256));
            let mut writer = Writer::with_options(&mut output, options).unwrap();
            let schema = writer
                .add_schema("sensor_msgs/msg/CompressedImage", "ros2msg", b"schema")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/camera", "cdr", &BTreeMap::new())
                .unwrap();
            for sequence in 0..4 {
                let payload =
                    viewer_core::encode_compressed_image_cdr(&viewer_core::CompressedImage {
                        measurement_time: viewer_core::MeasurementTime(1_000 + i64::from(sequence)),
                        frame_id: "camera".to_owned(),
                        format: "jpeg".to_owned(),
                        jpeg: vec![0xff, 0xd8, sequence as u8, 0xff, 0xd9],
                    })
                    .unwrap();
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence,
                            log_time: 1_000 + u64::from(sequence) * 10,
                            publish_time: 1_000 + u64::from(sequence) * 10,
                        },
                        &payload,
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        output.into_inner()
    }

    fn collect(bytes: &[u8]) -> (LoadedWindow, usize) {
        let summary = Summary::read(bytes).unwrap().unwrap();
        let range = DataWindowTimeRange::new(ArrivalTime(1_000), ArrivalTime(1_031)).unwrap();
        let loaded =
            collect_window_from_bytes_for_test(&summary, &["/camera".to_owned()], range, bytes)
                .unwrap();
        let source_bytes = loaded.diagnostics.source_bytes;
        (loaded, source_bytes)
    }

    #[test]
    fn no_continuous_topics_means_no_chunk_reads_instead_of_whole_source_selection() {
        let bytes = recording(None);
        let summary = Summary::read(&bytes).unwrap().unwrap();
        let loaded = collect_window_from_bytes_for_test(
            &summary,
            &[],
            DataWindowTimeRange::new(ArrivalTime(1_000), ArrivalTime(1_031)).unwrap(),
            &bytes,
        )
        .unwrap();
        assert!(loaded.window.messages.is_empty());
        assert_eq!(loaded.diagnostics.source_reads, 0);
        assert_eq!(loaded.window.resident_bytes, 0);
    }

    #[test]
    fn camera_selection_falls_back_to_an_available_topic() {
        let bytes = camera_recording(None);
        let summary = Summary::read(&bytes).unwrap().unwrap();
        let catalog = crate::local::LocalCatalog::from_summary(&summary, "/absent-camera").unwrap();
        assert_eq!(catalog.plan.primary_camera_topic(), Some("/camera"));
    }

    #[test]
    fn uncompressed_windows_retain_shared_chunk_backings_without_message_copies() {
        let bytes = recording(None);
        let (loaded, source_bytes) = collect(&bytes);
        assert_eq!(loaded.window.messages.len(), 4);
        assert!(source_bytes < bytes.len());
        assert_eq!(loaded.window.range.start, ArrivalTime(1_000));
        assert_eq!(loaded.window.range.end_exclusive, ArrivalTime(1_031));
        assert_eq!(loaded.diagnostics.per_message_copied_bytes, 0);
        assert_eq!(loaded.window.logical_payload_bytes, 64);
        assert_eq!(loaded.window.resident_bytes, source_bytes);
        assert!(loaded.window.resident_bytes >= loaded.window.logical_payload_bytes);
    }

    #[test]
    fn zstd_windows_retain_decompressed_backings_without_message_copies() {
        let bytes = recording(Some(Compression::Zstd));
        let (loaded, source_bytes) = collect(&bytes);
        assert_eq!(loaded.window.messages.len(), 4);
        assert!(source_bytes < bytes.len());
        assert_eq!(loaded.window.messages[0].payload.as_ref(), &[0; 16]);
        assert_eq!(loaded.diagnostics.per_message_copied_bytes, 0);
        assert!(loaded.diagnostics.decompressed_bytes > 0);
        assert_eq!(
            loaded.window.resident_bytes,
            loaded.diagnostics.decompressed_bytes
        );
    }

    fn first_parsed_chunk(bytes: &[u8]) -> ParsedChunk {
        let summary = Summary::read(bytes).unwrap().unwrap();
        let range = DataWindowTimeRange::new(ArrivalTime(1_000), ArrivalTime(1_031)).unwrap();
        let mut collector =
            OwnedWindowCollector::new(&summary, &["/camera".to_owned()], range).unwrap();
        let request = collector.next_read().unwrap();
        let file_range =
            validate_range(bytes.len() as u64, request.offset, request.length).unwrap();
        let compressed =
            Bytes::copy_from_slice(&bytes[file_range.offset as usize..file_range.end() as usize]);
        let (backing, _) = decode_chunk_backing(&request, compressed).unwrap();
        parse_chunk_backing(backing, &collector.selected_channels, range, |_, _| {
            Ok(true)
        })
        .unwrap()
    }

    #[test]
    fn exact_restore_admits_only_the_indexed_record_and_shares_its_chunk_backing() {
        let parsed = first_parsed_chunk(&recording(None));
        let raw = &parsed.messages[0];
        let channel = raw.stream_id.0 as u16;
        let selection = RestoreChunkSelection {
            exact: vec![(
                channel,
                raw.arrival_time.0 as u64,
                parsed.message_offsets[0],
            )],
            ..Default::default()
        };
        let mut work = RestoreWork::new(std::rc::Rc::new(std::cell::Cell::new(0)), 0);
        let backing = parsed.backing.clone();
        let filtered = parse_chunk_backing(
            backing.clone(),
            &BTreeSet::from([channel]),
            DataWindowTimeRange::new(ArrivalTime(1_000), ArrivalTime(1_031)).unwrap(),
            |header, offset| {
                if !selection.contains(header, offset, ArrivalTime(1_030)) {
                    return Ok(false);
                }
                work.admit_message()?;
                Ok(true)
            },
        )
        .unwrap();
        assert_eq!(filtered.messages.len(), 1);
        assert_eq!(work.messages, 1);
        assert_eq!(filtered.messages[0].arrival_time, raw.arrival_time);
        assert_eq!(filtered.messages[0].payload.as_ptr(), raw.payload.as_ptr());
    }

    fn assert_messages_share_backing(parsed: ParsedChunk) {
        assert!(!parsed.messages.is_empty());
        let backing_start = parsed.backing.as_ptr() as usize;
        let backing_end = backing_start + parsed.backing.len();
        for message in &parsed.messages {
            let payload_start = message.payload.as_ptr() as usize;
            assert!(payload_start >= backing_start);
            assert!(payload_start + message.payload.len() <= backing_end);
        }
        assert!(!parsed.backing.is_unique());
        drop(parsed.messages);
        assert!(parsed.backing.is_unique());
    }

    #[test]
    fn parsed_chunk_offsets_match_public_mcap_message_indexes() {
        let bytes = recording(None);
        let summary = Summary::read(&bytes).unwrap().unwrap();
        let chunk = summary
            .chunk_indexes
            .iter()
            .find(|chunk| !chunk.message_index_offsets.is_empty())
            .unwrap();
        let indexes = summary.read_message_indexes(&bytes, chunk).unwrap();
        let parsed = first_parsed_chunk(&bytes);

        for (message, offset) in parsed.messages.iter().zip(&parsed.message_offsets) {
            let channel = u16::try_from(message.stream_id.0).unwrap();
            let entries = indexes
                .iter()
                .find(|(descriptor, _)| descriptor.id == channel)
                .map(|(_, entries)| entries)
                .unwrap();
            assert!(entries.iter().any(|entry| {
                entry.log_time == message.arrival_time.0 as u64 && entry.offset == *offset
            }));
        }
    }

    #[test]
    fn uncompressed_message_slices_share_the_file_range_allocation() {
        assert_messages_share_backing(first_parsed_chunk(&recording(None)));
    }

    #[test]
    fn zstd_message_slices_share_the_decompressed_allocation() {
        assert_messages_share_backing(first_parsed_chunk(&recording(Some(Compression::Zstd))));
    }

    #[test]
    fn compressed_and_uncompressed_windows_reduce_to_the_same_camera_state() {
        fn reduce(
            bytes: &[u8],
        ) -> (
            Vec<viewer_core::CameraFrame>,
            viewer_core::ProcessingCounters,
        ) {
            let summary = Summary::read(bytes).unwrap().unwrap();
            let catalog = LocalCatalog::from_summary(&summary, "/camera").unwrap();
            let range = DataWindowTimeRange::new(ArrivalTime(1_000), ArrivalTime(1_031)).unwrap();
            let loaded = collect_window_from_bytes_for_test(
                &summary,
                &catalog.selected_topics,
                range,
                bytes,
            )
            .unwrap();
            let mut cameras = viewer_core::CameraController::new(&catalog.plan);
            for message in &loaded.window.messages {
                cameras.admit(message);
            }
            cameras.advance(std::time::Duration::from_secs(1));
            let frames = cameras
                .state()
                .frames()
                .map(|(_, frame)| frame.clone())
                .collect();
            (frames, cameras.counters())
        }

        assert_eq!(
            reduce(&camera_recording(None)),
            reduce(&camera_recording(Some(Compression::Zstd)))
        );
    }

    #[test]
    fn local_camera_jpeg_shares_the_chunk_and_cdr_allocation() {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap"),
        )
        .unwrap();
        let summary = Summary::read(&bytes).unwrap().unwrap();
        let range = DataWindowTimeRange::new(
            ArrivalTime(summary.stats.as_ref().unwrap().message_start_time as i64),
            ArrivalTime(summary.stats.as_ref().unwrap().message_end_time as i64 + 1),
        )
        .unwrap();
        let mut collector = OwnedWindowCollector::new(
            &summary,
            &["/camera/front/image/compressed".to_owned()],
            range,
        )
        .unwrap();
        let request = collector.next_read().unwrap();
        let file_range =
            validate_range(bytes.len() as u64, request.offset, request.length).unwrap();
        let compressed =
            Bytes::copy_from_slice(&bytes[file_range.offset as usize..file_range.end() as usize]);
        let (backing, _) = decode_chunk_backing(&request, compressed).unwrap();
        let parsed = parse_chunk_backing(backing, &collector.selected_channels, range, |_, _| {
            Ok(true)
        })
        .unwrap();
        let raw = parsed.messages.into_iter().next().unwrap().payload;
        let raw_start = raw.as_ptr() as usize;
        let raw_end = raw_start + raw.len();
        let image = viewer_core::decode_compressed_image_bytes(raw).unwrap();
        let jpeg_start = image.jpeg.as_ptr() as usize;

        assert!(raw_start >= parsed.backing.as_ptr() as usize);
        assert!(raw_end <= parsed.backing.as_ptr() as usize + parsed.backing.len());
        assert!(jpeg_start >= raw_start);
        assert!(jpeg_start + image.jpeg.len() <= raw_end);
    }

    #[test]
    fn range_validation_rejects_whole_file_api_edge_cases() {
        assert!(validate_range(0, 0, 0).is_err());
        assert!(validate_range(10, 9, 2).is_err());
        assert!(validate_range(10, u64::MAX, 2).is_err());
        assert!(validate_range(JS_MAX_SAFE_INTEGER + 1, 0, 0).is_err());
    }

    struct FileWindowMetrics {
        messages: usize,
        reads: usize,
        source_bytes: usize,
        decompressed_bytes: usize,
        logical_payload_bytes: usize,
        resident_bytes: usize,
        copied_bytes: usize,
        load_ms: f64,
        processing_ms: f64,
    }

    fn collect_file_window(path: &std::path::Path) -> FileWindowMetrics {
        use mcap::sans_io::{SummaryReadEvent, SummaryReader};
        use std::io::{Read, Seek};

        let load_started = std::time::Instant::now();
        let mut file = std::fs::File::open(path).unwrap();
        let file_size = file.metadata().unwrap().len();
        let mut summary_reader = SummaryReader::new();
        while let Some(event) = summary_reader.next_event() {
            match event.unwrap() {
                SummaryReadEvent::ReadRequest(need) => {
                    let read = file.read(summary_reader.insert(need)).unwrap();
                    summary_reader.notify_read(read);
                }
                SummaryReadEvent::SeekRequest(seek) => {
                    let position = file.seek(seek).unwrap();
                    summary_reader.notify_seeked(position);
                }
            }
        }
        let summary = summary_reader.finish().unwrap();
        let catalog =
            LocalCatalog::from_summary(&summary, "/camera/front/image/compressed").unwrap();
        let recording = catalog.core.time_range.unwrap();
        let end = ArrivalTime(
            recording
                .start
                .0
                .saturating_add(1_000_000_000)
                .min(recording.end_exclusive.0),
        );
        let range = DataWindowTimeRange::new(recording.start, end).unwrap();
        let mut collector =
            OwnedWindowCollector::new(&summary, &catalog.selected_topics, range).unwrap();
        let mut range_bytes = 0_usize;
        let mut range_reads = 0_usize;
        let mut processing = std::time::Duration::ZERO;
        while let Some(request) = collector.next_read() {
            let range = validate_range(file_size, request.offset, request.length).unwrap();
            let mut bytes = vec![0; range.length];
            file.seek(std::io::SeekFrom::Start(range.offset)).unwrap();
            file.read_exact(&mut bytes).unwrap();
            range_bytes += bytes.len();
            range_reads += 1;
            let processing_started = std::time::Instant::now();
            collector
                .insert_chunk(&request, Bytes::from(bytes))
                .unwrap();
            processing += processing_started.elapsed();
        }
        let per_message_copied_bytes = 0;
        let loaded = collector
            .finish(WindowLoadDiagnostics {
                source_reads: range_reads as u64,
                source_bytes: range_bytes,
                decompressed_bytes: 0,
                per_message_copied_bytes,
                latency_ms: 0.0,
                processing_ms: processing.as_secs_f64() * 1_000.0,
            })
            .unwrap();
        assert!(range_bytes < file_size as usize);
        FileWindowMetrics {
            messages: loaded.window.messages.len(),
            reads: range_reads,
            source_bytes: range_bytes,
            decompressed_bytes: loaded.diagnostics.decompressed_bytes,
            logical_payload_bytes: loaded.window.logical_payload_bytes,
            resident_bytes: loaded.window.resident_bytes,
            copied_bytes: loaded.diagnostics.per_message_copied_bytes,
            load_ms: load_started.elapsed().as_secs_f64() * 1_000.0,
            processing_ms: loaded.diagnostics.processing_ms,
        }
    }

    #[test]
    #[ignore = "manual lazy-read verification for the 596 MB Zstd recording"]
    fn reads_requested_window_from_actual_zstd_recording() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mcap/turtlebot3_7cam_fhd/turtlebot3_7cam_fhd_0.mcap");
        let metrics = collect_file_window(&path);
        eprintln!(
            "zstd window: {} messages, {} reads, {} source, {} decompressed, {} logical, {} resident, {} copied bytes, {:.2} ms load/{:.2} ms processing",
            metrics.messages,
            metrics.reads,
            metrics.source_bytes,
            metrics.decompressed_bytes,
            metrics.logical_payload_bytes,
            metrics.resident_bytes,
            metrics.copied_bytes,
            metrics.load_ms,
            metrics.processing_ms,
        );
        assert!(metrics.messages > 0);
        assert_eq!(metrics.copied_bytes, 0);
    }

    #[test]
    #[ignore = "manual lazy-read verification for the 2.86 GB uncompressed recording"]
    fn reads_requested_window_from_actual_uncompressed_recording() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mcap/turtlebot3_7cam_fhd/turtlebot3_7cam_fhd_0_uncompressed.mcap");
        let metrics = collect_file_window(&path);
        eprintln!(
            "uncompressed window: {} messages, {} reads, {} source, {} decompressed, {} logical, {} resident, {} copied bytes, {:.2} ms load/{:.2} ms processing",
            metrics.messages,
            metrics.reads,
            metrics.source_bytes,
            metrics.decompressed_bytes,
            metrics.logical_payload_bytes,
            metrics.resident_bytes,
            metrics.copied_bytes,
            metrics.load_ms,
            metrics.processing_ms,
        );
        assert!(metrics.messages > 0);
        assert_eq!(metrics.copied_bytes, 0);
    }
}
