use crate::data_plane::{DataLoadError, LoadedWindow, WindowLoadDiagnostics};
use bytes::Bytes;
use mcap::records::ChunkIndex;
#[cfg(target_arch = "wasm32")]
use mcap::records::{Record, op};
#[cfg(any(test, target_arch = "wasm32"))]
use std::collections::BTreeMap;
use std::collections::{BTreeSet, VecDeque};
#[cfg(target_arch = "wasm32")]
use viewer_core::StreamId;
use viewer_core::{ArrivalTime, DataWindowTimeRange, RawMessage, SerializedWindow};

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
    length: u64,
    compression: String,
    uncompressed_size: u64,
}

impl ChunkReadRequest {
    fn bulk_length(&self) -> Result<usize, DataLoadError> {
        usize::try_from(self.length)
            .map_err(|_| DataLoadError::invalid_record("compressed Chunk exceeds address space"))
    }

    #[cfg(any(test, target_arch = "wasm32"))]
    fn stream(&self) -> bool {
        self.uncompressed_size > 64 * 1024 * 1024
            && matches!(self.compression.as_str(), "" | "zstd")
    }
}

struct OwnedWindowCollector {
    pending_chunks: VecDeque<ChunkReadRequest>,
    selected_channels: BTreeSet<u16>,
    range: DataWindowTimeRange,
    messages: Vec<RawMessage>,
    resident_bytes: usize,
    decompressed_bytes: u64,
    copied_bytes: usize,
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
            copied_bytes: 0,
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
        if compressed.len() as u64 != request.length {
            return Err(DataLoadError::invalid_record(
                "browser MCAP chunk range is truncated",
            ));
        }
        let (backing, decompressed_bytes) = decode_chunk_backing(request, compressed)?;
        self.decompressed_bytes = self
            .decompressed_bytes
            .checked_add(decompressed_bytes as u64)
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

    // Large Chunk records borrow a small reusable decode buffer. Retain only the
    // selected window's payloads, rather than pinning the entire physical Chunk.
    #[cfg(any(test, target_arch = "wasm32"))]
    fn insert_streamed_record(&mut self, opcode: u8, data: &[u8]) -> Result<(), DataLoadError> {
        if opcode != mcap::records::op::MESSAGE {
            return Ok(());
        }
        let mcap::records::Record::Message { header, data } = mcap::parse_record(opcode, data)?
        else {
            unreachable!("MESSAGE opcode parses as Message");
        };
        let arrival_time = ArrivalTime(i64::try_from(header.log_time).map_err(|_| {
            DataLoadError::invalid_record("MCAP timestamp exceeds signed nanosecond range")
        })?);
        if !self.range.contains(arrival_time)
            || !self.selected_channels.contains(&header.channel_id)
        {
            return Ok(());
        }
        const MAX_WINDOW_PAYLOAD: usize = 1024 * 1024 * 1024;
        if data.len() > MAX_WINDOW_PAYLOAD.saturating_sub(self.resident_bytes)
            || self.messages.len() >= 200_000
        {
            return Err(DataLoadError::invalid_record(
                "browser window exceeds payload/message budget",
            ));
        }
        self.messages.push(RawMessage {
            stream_id: viewer_core::StreamId(u32::from(header.channel_id)),
            arrival_time,
            publish_time_ns: Some(header.publish_time),
            sequence: Some(header.sequence),
            payload: Bytes::copy_from_slice(&data),
        });
        self.resident_bytes += data.len();
        self.copied_bytes += data.len();
        Ok(())
    }

    fn finish(
        mut self,
        mut diagnostics: WindowLoadDiagnostics,
    ) -> Result<LoadedWindow, DataLoadError> {
        self.messages
            .sort_by_key(|message| (message.arrival_time, message.stream_id.0));
        diagnostics.decompressed_bytes = self.decompressed_bytes;
        diagnostics.per_message_copied_bytes = self.copied_bytes;
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
    // Browser support is a source policy, independent of Cargo feature unification.
    if !matches!(request.compression.as_str(), "" | "zstd" | "lz4") {
        return Err(viewer_mcap::McapChunkError::UnsupportedCompression(
            request.compression.clone(),
        )
        .into());
    }
    let backing = viewer_mcap::decode_mcap_chunk(
        &request.compression,
        compressed,
        usize::try_from(request.uncompressed_size)
            .map_err(|_| DataLoadError::invalid_record("expanded Chunk exceeds address space"))?,
    )?;
    let decompressed_bytes = if request.compression.is_empty() {
        0
    } else {
        backing.len()
    };
    Ok((backing, decompressed_bytes))
}

fn chunk_request(index: &ChunkIndex) -> Result<ChunkReadRequest, DataLoadError> {
    Ok(ChunkReadRequest {
        offset: index
            .compressed_data_offset()
            .map_err(DataLoadError::from)?,
        length: index.compressed_size,
        compression: index.compression.clone(),
        uncompressed_size: index.uncompressed_size,
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
    mut admit: impl FnMut(&RawMessage, u64) -> Result<bool, DataLoadError>,
) -> Result<ParsedChunk, DataLoadError> {
    let mut messages = Vec::new();
    #[cfg(test)]
    let mut message_offsets = Vec::new();
    viewer_mcap::visit_mcap_chunk_records(&backing, |record_offset, message| {
        if let Some(message) = message
            && range.contains(message.arrival_time)
            && (selected_channels.is_empty()
                || selected_channels.contains(&(message.stream_id.0 as u16)))
            && admit(&message, record_offset)?
        {
            messages.push(message);
            #[cfg(test)]
            message_offsets.push(record_offset);
        }
        Ok::<_, DataLoadError>(())
    })?;
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
        let range = validate_range(bytes.len() as u64, request.offset, request.bulk_length()?)?;
        let chunk = &bytes[range.offset as usize..range.end() as usize];
        source_bytes = source_bytes
            .checked_add(chunk.len())
            .ok_or_else(|| DataLoadError::invalid_record("test source byte count overflow"))?;
        source_reads = source_reads.saturating_add(1);
        collector.insert_chunk(&request, Bytes::copy_from_slice(chunk))?;
    }
    collector.finish(WindowLoadDiagnostics {
        source_reads,
        source_bytes: source_bytes as u64,
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
struct BrowserWindowSource {
    file: File,
    file_size: u64,
    summary: Rc<mcap::Summary>,
    selected_topics: Vec<String>,
    chunk_cursor: RefCell<Option<BrowserChunkCursor>>,
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct BrowserMcapWindowLoader {
    source: Rc<BrowserWindowSource>,
    generation: Rc<Cell<u64>>,
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
            source: Rc::new(BrowserWindowSource {
                file,
                file_size,
                summary,
                selected_topics,
                chunk_cursor: RefCell::new(None),
            }),
            generation: Rc::new(Cell::new(0)),
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
        self.generation.set(self.generation.get().wrapping_add(1));
        let generation = self.generation.get();
        let cancellation = self.generation.clone();
        self.state = LocalLoadState::Loading { generation, range };
        self.metrics.load_requests = self.metrics.load_requests.saturating_add(1);
        let source = self.source.clone();
        let inbox = Rc::clone(&self.inbox);
        spawn_local(async move {
            let result = load_browser_window(&source, range, &cancellation, generation).await;
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
            if result.generation != self.generation.get() || !current {
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
                        .saturating_add(loaded.diagnostics.source_bytes);
                    self.metrics.decompressed_bytes = self
                        .metrics
                        .decompressed_bytes
                        .saturating_add(loaded.diagnostics.decompressed_bytes);
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
        self.generation.set(self.generation.get().wrapping_add(1));
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
    let restore = BrowserMcapRestoreLoader::new(loader.source.file.clone(), file_size, summary);
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
    fn contains(&self, message: &RawMessage, offset: u64, target: ArrivalTime) -> bool {
        let Ok(channel) = u16::try_from(message.stream_id.0) else {
            return false;
        };
        let Ok(log_time) = u64::try_from(message.arrival_time.0) else {
            return false;
        };
        self.exact.contains(&(channel, log_time, offset))
            || self.history.get(&channel).is_some_and(|start| {
                message.arrival_time.0 >= *start && message.arrival_time <= target
            })
            || self.persistent.contains(&channel)
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
        .map(|chunk| viewer_mcap::IndexedChunkFact {
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
        let mut search = viewer_mcap::MessageIndexPredecessor::new(target);
        let count = summary
            .stats
            .as_ref()
            .and_then(|stats| stats.channel_message_counts.get(&channel).copied());
        for chunk_index in
            viewer_mcap::latest_candidate_chunks(&index_facts, *stream, count, plan.target)
                .map_err(DataLoadError::from)?
        {
            let chunk = &summary.chunk_indexes[chunk_index];
            work.check()?;
            if !search.can_improve(chunk.message_end_time, chunk_index) {
                continue;
            }
            let offset = chunk.message_index_offsets[&channel];
            let entries = read_message_index(file, file_size, channel, offset, work).await?;
            search.consider_unsorted(chunk_index, &entries);
        }
        let Some((time, position)) = search.result() else {
            continue;
        };
        needs.entry(position.chunk).or_default().exact.push((
            channel,
            time,
            position.record_offset,
        ));
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
                viewer_mcap::history_candidate_chunks(&index_facts, *stream, count, read.range)
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
        for chunk_index in viewer_mcap::persistent_candidate_chunks(&index_facts, *stream, count)
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
    if request.compression.is_empty() && request.stream() {
        let mut offsets = selection
            .exact
            .iter()
            .map(|(_, _, offset)| *offset)
            .collect::<BTreeSet<_>>();
        for channel in selection.history.keys().chain(selection.persistent.iter()) {
            let index_offset = chunk.message_index_offsets.get(channel).ok_or_else(|| {
                DataLoadError::invalid_record("uncompressed restore needs Message Index")
            })?;
            for entry in read_message_index(file, file_size, *channel, *index_offset, work).await? {
                let time = i64::try_from(entry.log_time)
                    .map_err(|_| DataLoadError::invalid_record("indexed time overflow"))?;
                if selection.persistent.contains(channel)
                    || selection
                        .history
                        .get(channel)
                        .is_some_and(|start| time >= *start && time <= target.0)
                {
                    offsets.insert(entry.offset);
                }
            }
        }
        let mut messages = Vec::new();
        for offset in offsets {
            work.check()?;
            let remaining = (256 * 1024 * 1024_usize).saturating_sub(work.chunk_bytes);
            let message =
                read_uncompressed_message(file, file_size, &request, offset, remaining).await?;
            work.reserve_chunk(message.payload.len() + 31, 0)?;
            if !selection.contains(&message, offset, target) {
                return Err(DataLoadError::invalid_record(
                    "indexed message differs from restore selection",
                ));
            }
            work.admit_message()?;
            messages.push(message);
        }
        return Ok(messages);
    }
    work.reserve_chunk(
        request.bulk_length()?,
        usize::try_from(request.uncompressed_size)
            .map_err(|_| DataLoadError::invalid_record("expanded Chunk exceeds address space"))?,
    )?;
    let range = validate_range(file_size, request.offset, request.bulk_length()?)?;
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
        |message, offset| {
            if !selection.contains(message, offset, target) {
                return Ok(false);
            }
            if selection.exact.contains(&(
                message.stream_id.0 as u16,
                message.arrival_time.0 as u64,
                offset,
            )) {
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
    source: &BrowserWindowSource,
    range: DataWindowTimeRange,
    cancellation: &Cell<u64>,
    generation: u64,
) -> Result<LoadedWindow, DataLoadError> {
    let started = Date::now();
    let file = &source.file;
    let file_size = source.file_size;
    let mut collector = OwnedWindowCollector::new(&source.summary, &source.selected_topics, range)?;
    let mut source_reads = 0_u64;
    let mut source_bytes = 0_u64;
    let mut processing_ms = 0.0;
    while let Some(request) = collector.next_read() {
        check_window_generation(cancellation, generation)?;
        if request.stream() {
            let (reads, bytes, processing) =
                stream_browser_chunk(source, &request, &mut collector, cancellation, generation)
                    .await?;
            source_reads += reads;
            source_bytes += bytes;
            processing_ms += processing;
            continue;
        }
        let range = validate_range(file_size, request.offset, request.bulk_length()?)?;
        let bytes = read_file_range(file, file_size, range).await?;
        check_window_generation(cancellation, generation)?;
        source_reads = source_reads.saturating_add(1);
        source_bytes = source_bytes
            .checked_add(bytes.len() as u64)
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
fn check_window_generation(cancellation: &Cell<u64>, expected: u64) -> Result<(), DataLoadError> {
    if cancellation.get() != expected {
        return Err(DataLoadError::invalid_record("browser window cancelled"));
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
async fn yield_browser() -> Result<(), DataLoadError> {
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        if let Err(error) = web_sys::window()
            .expect("browser Window")
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0)
        {
            let _ = reject.call1(&wasm_bindgen::JsValue::NULL, &error);
        }
    });
    JsFuture::from(promise).await.map_err(js_error)?;
    Ok(())
}

#[cfg(any(test, target_arch = "wasm32"))]
fn uncompressed_message_range(
    request: &ChunkReadRequest,
    record_offset: u64,
    header: &[u8],
    max_payload: usize,
) -> Result<ByteRange, DataLoadError> {
    if !request.compression.is_empty()
        || request.length != request.uncompressed_size
        || record_offset
            .checked_add(31)
            .is_none_or(|end| end > request.length)
        || header.len() != 9
        || header[0] != mcap::records::op::MESSAGE
    {
        return Err(DataLoadError::invalid_record(
            "invalid uncompressed Message Index position/header",
        ));
    }
    let length = u64::from_le_bytes(header[1..9].try_into().expect("eight-byte length"));
    if !(22..=256 * 1024 * 1024).contains(&length)
        || length - 22 > max_payload as u64
        || length > request.length - record_offset - 9
    {
        return Err(DataLoadError::invalid_record(
            "indexed message exceeds Chunk/payload bounds",
        ));
    }
    let offset = request
        .offset
        .checked_add(record_offset)
        .and_then(|offset| offset.checked_add(9))
        .ok_or_else(|| DataLoadError::invalid_record("indexed file position overflow"))?;
    Ok(ByteRange {
        offset,
        length: length as usize,
    })
}

#[cfg(target_arch = "wasm32")]
async fn read_uncompressed_message(
    file: &File,
    file_size: u64,
    request: &ChunkReadRequest,
    record_offset: u64,
    max_payload: usize,
) -> Result<RawMessage, DataLoadError> {
    if request.length != request.uncompressed_size
        || record_offset
            .checked_add(31)
            .is_none_or(|end| end > request.length)
    {
        return Err(DataLoadError::invalid_record(
            "invalid uncompressed indexed position",
        ));
    }
    let position = request
        .offset
        .checked_add(record_offset)
        .ok_or_else(|| DataLoadError::invalid_record("indexed file position overflow"))?;
    let header = read_file_range(file, file_size, validate_range(file_size, position, 9)?).await?;
    let range = uncompressed_message_range(request, record_offset, &header, max_payload)?;
    let backing = read_file_range(
        file,
        file_size,
        validate_range(file_size, range.offset, range.length)?,
    )
    .await?;
    let Record::Message { header, data } = mcap::parse_record(op::MESSAGE, &backing)? else {
        unreachable!("MESSAGE opcode parses as Message");
    };
    Ok(RawMessage {
        stream_id: StreamId(u32::from(header.channel_id)),
        arrival_time: ArrivalTime(
            i64::try_from(header.log_time)
                .map_err(|_| DataLoadError::invalid_record("message timestamp overflow"))?,
        ),
        publish_time_ns: Some(header.publish_time),
        sequence: Some(header.sequence),
        payload: backing.slice(backing.len() - data.len()..),
    })
}

#[cfg(any(test, target_arch = "wasm32"))]
struct BrowserChunkCursor {
    request: ChunkReadRequest,
    decoder: viewer_mcap::McapChunkStream,
    input: Bytes,
    consumed: usize,
    offset: u64,
    // Index entries keep physical offsets and exact log times; no decoded history.
    entries: Option<Vec<(u16, u64, u64)>>,
}

#[cfg(any(test, target_arch = "wasm32"))]
impl BrowserChunkCursor {
    fn rewind_before(&mut self, first: Option<u64>) -> Result<(), DataLoadError> {
        if first.is_some_and(|first| first < self.decoder.next_record_offset()) {
            // Grouped/out-of-order records or a replacement seek can require an earlier
            // physical position. Rewind, rather than treating physical order as log time.
            self.decoder = viewer_mcap::McapChunkStream::new(
                &self.request.compression,
                self.request.length,
                self.request.uncompressed_size,
            )?;
            self.input = Bytes::new();
            self.consumed = 0;
            self.offset = self.request.offset;
        }
        Ok(())
    }

    fn collect_step(
        &mut self,
        collector: &mut OwnedWindowCollector,
        selected_offsets: &Option<BTreeMap<u64, (u16, u64)>>,
        seen: &mut BTreeSet<u64>,
    ) -> Result<(), DataLoadError> {
        let last = selected_offsets
            .as_ref()
            .and_then(|offsets| offsets.last_key_value().map(|(offset, _)| *offset));
        self.consumed +=
            self.decoder
                .step(&self.input[self.consumed..], |offset, opcode, data| {
                    let before = collector.messages.len();
                    collector.insert_streamed_record(opcode, data)?;
                    if collector.messages.len() != before {
                        let message = collector.messages.last().expect("just appended message");
                        if selected_offsets.as_ref().is_some_and(|offsets| {
                            offsets.get(&offset).is_none_or(|(channel, time)| {
                                message.stream_id.0 != u32::from(*channel)
                                    || message.arrival_time.0 as u64 != *time
                            })
                        }) {
                            return Err(DataLoadError::invalid_record(
                                "message differs from Chunk Message Index",
                            ));
                        }
                        seen.insert(offset);
                    }
                    Ok(last.is_none_or(|last| offset < last))
                })?;
        Ok(())
    }
}

#[cfg(target_arch = "wasm32")]
async fn stream_browser_chunk(
    source: &BrowserWindowSource,
    request: &ChunkReadRequest,
    collector: &mut OwnedWindowCollector,
    cancellation: &Cell<u64>,
    generation: u64,
) -> Result<(u64, u64, f64), DataLoadError> {
    const READ_BYTES: u64 = 4 * 1024 * 1024;
    const MAX_CHUNK_WORK: u64 = 64 * 1024 * 1024 * 1024;
    if request.length > MAX_CHUNK_WORK || request.uncompressed_size > MAX_CHUNK_WORK {
        return Err(DataLoadError::invalid_record(
            "browser Chunk exceeds 64 GiB work limit",
        ));
    }
    let end = request
        .offset
        .checked_add(request.length)
        .ok_or_else(|| DataLoadError::invalid_record("Chunk range overflow"))?;
    if end > source.file_size || end > JS_MAX_SAFE_INTEGER {
        return Err(DataLoadError::invalid_record("Chunk exceeds file bounds"));
    }
    let cached = source.chunk_cursor.borrow_mut().take();
    let mut cursor = match cached {
        Some(cursor) if cursor.request == *request => cursor,
        _ => BrowserChunkCursor {
            request: request.clone(),
            decoder: viewer_mcap::McapChunkStream::new(
                &request.compression,
                request.length,
                request.uncompressed_size,
            )?,
            input: Bytes::new(),
            consumed: 0,
            offset: request.offset,
            entries: None,
        },
    };
    let mut reads = 0_u64;
    let mut bytes_read = 0_u64;
    if cursor.entries.is_none() {
        let chunk = source
            .summary
            .chunk_indexes
            .iter()
            .find(|chunk| chunk.compressed_data_offset().ok() == Some(request.offset))
            .ok_or_else(|| DataLoadError::invalid_record("Chunk missing from Summary"))?;
        if collector
            .selected_channels
            .iter()
            .all(|id| chunk.message_index_offsets.contains_key(id))
        {
            let token = Rc::new(Cell::new(generation));
            let mut work = RestoreWork::new(token, generation);
            let mut entries = Vec::new();
            for channel in &collector.selected_channels {
                check_window_generation(cancellation, generation)?;
                let offset = chunk.message_index_offsets[channel];
                let index =
                    read_message_index(&source.file, source.file_size, *channel, offset, &mut work)
                        .await?;
                for entry in index {
                    if entry.offset >= request.uncompressed_size
                        || i64::try_from(entry.log_time).is_err()
                    {
                        return Err(DataLoadError::invalid_record(
                            "Message Index entry exceeds Chunk/time bounds",
                        ));
                    }
                    entries.push((*channel, entry.log_time, entry.offset));
                }
            }
            reads += work.index_reads as u64;
            bytes_read += work.index_bytes as u64;
            cursor.entries = Some(entries);
        }
    }
    let selected_offsets = cursor
        .entries
        .as_ref()
        .map(|entries| {
            let mut offsets = BTreeMap::new();
            for (channel, time, offset) in entries {
                if i64::try_from(*time)
                    .ok()
                    .is_some_and(|time| collector.range.contains(ArrivalTime(time)))
                {
                    if offsets.insert(*offset, (*channel, *time)).is_some() {
                        return Err(DataLoadError::invalid_record(
                            "duplicate Message Index physical position",
                        ));
                    }
                    if offsets.len() > 200_000 {
                        return Err(DataLoadError::invalid_record(
                            "indexed window exceeds message budget",
                        ));
                    }
                }
            }
            Ok::<_, DataLoadError>(offsets)
        })
        .transpose()?;
    if selected_offsets.as_ref().is_some_and(BTreeMap::is_empty) {
        check_window_generation(cancellation, generation)?;
        *source.chunk_cursor.borrow_mut() = Some(cursor);
        return Ok((reads, bytes_read, 0.0));
    }
    if request.compression.is_empty()
        && let Some(offsets) = &selected_offsets
    {
        for (offset, (channel, time)) in offsets {
            if collector.messages.len() >= 200_000 {
                return Err(DataLoadError::invalid_record(
                    "browser window exceeds message budget",
                ));
            }
            check_window_generation(cancellation, generation)?;
            let message = read_uncompressed_message(
                &source.file,
                source.file_size,
                request,
                *offset,
                (1024 * 1024 * 1024_usize)
                    .saturating_sub(collector.resident_bytes)
                    .saturating_sub(22),
            )
            .await?;
            if message.stream_id.0 != u32::from(*channel) || message.arrival_time.0 as u64 != *time
            {
                return Err(DataLoadError::invalid_record(
                    "indexed message differs from window selection",
                ));
            }
            reads += 2;
            bytes_read += 31 + message.payload.len() as u64;
            collector.resident_bytes += message.payload.len() + 22;
            collector.messages.push(message);
        }
        check_window_generation(cancellation, generation)?;
        *source.chunk_cursor.borrow_mut() = Some(cursor);
        return Ok((reads, bytes_read, 0.0));
    }
    let first = selected_offsets
        .as_ref()
        .and_then(|offsets| offsets.first_key_value().map(|(offset, _)| *offset));
    let last = selected_offsets
        .as_ref()
        .and_then(|offsets| offsets.last_key_value().map(|(offset, _)| *offset));
    cursor.rewind_before(first)?;
    let initial_expanded = cursor.decoder.expanded_bytes();
    let mut seen = BTreeSet::new();
    let mut processing_ms = 0.0;
    let mut yield_started = Date::now();
    loop {
        check_window_generation(cancellation, generation)?;
        if cursor.consumed == cursor.input.len() && cursor.offset < end {
            let length = READ_BYTES.min(end - cursor.offset) as usize;
            cursor.input = read_file_range(
                &source.file,
                source.file_size,
                validate_range(source.file_size, cursor.offset, length)?,
            )
            .await?;
            cursor.consumed = 0;
            cursor.offset += length as u64;
            reads += 1;
            bytes_read += length as u64;
        }
        check_window_generation(cancellation, generation)?;
        let started = Date::now();
        let expanded = cursor.decoder.expanded_bytes();
        cursor.collect_step(collector, &selected_offsets, &mut seen)?;
        processing_ms += Date::now() - started;
        if Date::now() - yield_started >= 8.0 {
            yield_browser().await?;
            yield_started = Date::now();
        }
        if last.is_some_and(|last| cursor.decoder.next_record_offset() > last) {
            break;
        }
        if cursor.offset == end
            && cursor.consumed == cursor.input.len()
            && cursor.decoder.expanded_bytes() == expanded
        {
            if last.is_some() {
                return Err(DataLoadError::invalid_record(
                    "indexed message absent from Chunk",
                ));
            }
            break;
        }
    }
    if let Some(expected) = selected_offsets
        && !seen.iter().eq(expected.keys())
    {
        return Err(DataLoadError::invalid_record("incomplete indexed window"));
    }
    if !request.compression.is_empty() {
        collector.decompressed_bytes += cursor.decoder.expanded_bytes() - initial_expanded;
    }
    check_window_generation(cancellation, generation)?;
    if last.is_some() {
        *source.chunk_cursor.borrow_mut() = Some(cursor);
    } else {
        cursor.decoder.finish()?;
    }
    Ok((reads, bytes_read, processing_ms))
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
    let summary = reader.finish().ok_or_else(|| {
        DataLoadError::invalid_record(viewer_mcap::McapSourceError::SummaryRequired.to_string())
    })?;
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
    use crate::local::LocalCatalog;
    use mcap::{Compression, Summary, WriteOptions, Writer, records::MessageHeader};
    use std::{collections::BTreeMap, io::Cursor};

    #[test]
    fn browser_compression_policy_rejects_unknown_formats() {
        let request = ChunkReadRequest {
            offset: 0,
            length: 0,
            compression: "unknown".into(),
            uncompressed_size: 0,
        };
        assert!(
            matches!(decode_chunk_backing(&request, Bytes::new()), Err(DataLoadError::Chunk(error)) if matches!(error.as_ref(), viewer_mcap::McapChunkError::UnsupportedCompression(format) if format == "unknown"))
        );
    }

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
                        data: vec![0xff, 0xd8, sequence as u8, 0xff, 0xd9],
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
        let source_bytes = loaded.diagnostics.source_bytes as usize;
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
            loaded.diagnostics.decompressed_bytes as usize
        );
    }

    #[test]
    fn direct_message_ranges_keep_large_offsets_and_reject_bad_lengths_before_allocation() {
        let request = ChunkReadRequest {
            offset: 100,
            length: 20 << 30,
            uncompressed_size: 20 << 30,
            compression: String::new(),
        };
        let header = |length: u64| {
            let mut bytes = vec![mcap::records::op::MESSAGE];
            bytes.extend(length.to_le_bytes());
            bytes
        };
        let range = uncompressed_message_range(&request, 19 << 30, &header(32), 10).unwrap();
        assert_eq!(range.offset, (19 << 30) + 109);
        assert_eq!(range.length, 32);
        for (offset, length, budget) in [
            (0, 21, 10),
            (0, 32, 9),
            (0, u64::MAX, 10),
            (u64::MAX, 32, 10),
            (request.length - 31, 32, 10),
        ] {
            assert!(uncompressed_message_range(&request, offset, &header(length), budget).is_err());
        }
        assert!(uncompressed_message_range(&request, 0, &[0; 8], 10).is_err());
        let tiny = ChunkReadRequest {
            length: 5,
            uncompressed_size: 5,
            ..request.clone()
        };
        assert!(uncompressed_message_range(&tiny, 0, &header(22), 0).is_err());
        let overflowing = ChunkReadRequest {
            offset: u64::MAX,
            ..request
        };
        assert!(uncompressed_message_range(&overflowing, 0, &header(22), 0).is_err());
    }

    #[test]
    fn indexed_chunk_continuation_and_rewind_preserve_windows_and_equal_time_ties() {
        for times in [
            [1010, 1010, 1020, 1020, 1030],
            [1010, 1030, 1010, 1020, 1020],
        ] {
            let mut output = Cursor::new(Vec::new());
            {
                let mut writer = Writer::with_options(
                    &mut output,
                    WriteOptions::new()
                        .compression(Some(Compression::Zstd))
                        .chunk_size(None),
                )
                .unwrap();
                let channel = writer
                    .add_channel(0, "/camera", "cdr", &BTreeMap::new())
                    .unwrap();
                for (sequence, time) in times.into_iter().enumerate() {
                    writer
                        .write_to_known_channel(
                            &MessageHeader {
                                channel_id: channel,
                                sequence: sequence as u32,
                                log_time: time,
                                publish_time: time,
                            },
                            &[sequence as u8; 16],
                        )
                        .unwrap();
                }
                writer.finish().unwrap();
            }
            let bytes = output.into_inner();
            let summary = Summary::read(&bytes).unwrap().unwrap();
            assert_eq!(summary.chunk_indexes.len(), 1);
            let request = chunk_request(&summary.chunk_indexes[0]).unwrap();
            let encoded = Bytes::copy_from_slice(
                &bytes[request.offset as usize..(request.offset + request.length) as usize],
            );
            let (backing, _) = decode_chunk_backing(&request, encoded.clone()).unwrap();
            let mut entries = Vec::new();
            viewer_mcap::visit_mcap_chunk_records(&backing, |offset, message| {
                if let Some(message) = message {
                    entries.push((
                        message.stream_id.0 as u16,
                        message.arrival_time.0 as u64,
                        offset,
                    ));
                }
                Ok::<_, DataLoadError>(())
            })
            .unwrap();
            let mut cursor = BrowserChunkCursor {
                request: request.clone(),
                decoder: viewer_mcap::McapChunkStream::new(
                    "zstd",
                    request.length,
                    request.uncompressed_size,
                )
                .unwrap(),
                input: Bytes::new(),
                consumed: 0,
                offset: request.offset,
                entries: Some(entries),
            };
            for (start, end) in [(1000, 1020), (1020, 1040)] {
                let range = DataWindowTimeRange::new(ArrivalTime(start), ArrivalTime(end)).unwrap();
                let selected = Some(
                    cursor
                        .entries
                        .as_ref()
                        .unwrap()
                        .iter()
                        .filter(|(_, time, _)| range.contains(ArrivalTime(*time as i64)))
                        .map(|(channel, time, offset)| (*offset, (*channel, *time)))
                        .collect::<BTreeMap<_, _>>(),
                );
                let offsets = selected.as_ref().unwrap();
                cursor
                    .rewind_before(offsets.first_key_value().map(|(offset, _)| *offset))
                    .unwrap();
                if cursor.input.is_empty() {
                    assert_eq!(cursor.offset, request.offset);
                    cursor.input = encoded.clone();
                    cursor.consumed = 0;
                    cursor.offset += request.length;
                }
                let last = *offsets.last_key_value().unwrap().0;
                let mut collector =
                    OwnedWindowCollector::new(&summary, &["/camera".to_owned()], range).unwrap();
                let mut seen = BTreeSet::new();
                while cursor.decoder.next_record_offset() <= last {
                    cursor
                        .collect_step(&mut collector, &selected, &mut seen)
                        .unwrap();
                }
                assert!(seen.iter().eq(offsets.keys()));
                let actual = collector.finish(WindowLoadDiagnostics::default()).unwrap();
                let expected = collect_window_from_bytes_for_test(
                    &summary,
                    &["/camera".to_owned()],
                    range,
                    &bytes,
                )
                .unwrap();
                assert_eq!(actual.window.messages, expected.window.messages);
            }
        }
    }

    #[test]
    fn streamed_windows_match_bulk_selection_without_retaining_other_messages() {
        assert!(
            ChunkReadRequest {
                offset: 0,
                length: 20 << 30,
                compression: "zstd".into(),
                uncompressed_size: 20 << 30
            }
            .stream()
        );
        for compression in [None, Some(Compression::Zstd)] {
            let bytes = recording(compression);
            let summary = Summary::read(&bytes).unwrap().unwrap();
            let range = DataWindowTimeRange::new(ArrivalTime(1_010), ArrivalTime(1_030)).unwrap();
            let topics = ["/camera".to_owned()];
            let bulk =
                collect_window_from_bytes_for_test(&summary, &topics, range, &bytes).unwrap();
            let mut collector = OwnedWindowCollector::new(&summary, &topics, range).unwrap();
            while let Some(request) = collector.next_read() {
                let start = request.offset as usize;
                let encoded = &bytes[start..start + request.length as usize];
                let mut stream = viewer_mcap::McapChunkStream::new(
                    &request.compression,
                    request.length,
                    request.uncompressed_size,
                )
                .unwrap();
                for input in encoded.chunks(7) {
                    let mut consumed = 0;
                    loop {
                        let before = stream.expanded_bytes();
                        consumed += stream
                            .step(&input[consumed..], |_, opcode, data| {
                                collector.insert_streamed_record(opcode, data)?;
                                Ok::<_, DataLoadError>(true)
                            })
                            .unwrap();
                        if consumed == input.len() && before == stream.expanded_bytes() {
                            break;
                        }
                    }
                }
                stream.finish().unwrap();
            }
            let streamed = collector.finish(WindowLoadDiagnostics::default()).unwrap();
            assert_eq!(streamed.window.messages, bulk.window.messages);
            assert_eq!(streamed.window.messages.len(), 2);
            assert_eq!(streamed.window.resident_bytes, 32);
            assert_eq!(streamed.diagnostics.per_message_copied_bytes, 32);
        }
    }

    #[test]
    fn lz4_windows_and_exact_restore_share_one_decompressed_backing() {
        // Rust-only compression also runs when testing this package without the native workspace features.
        let bytes = recording(Some(Compression::Zstd));
        let summary = Summary::read(&bytes).unwrap().unwrap();
        let chunk = summary
            .chunk_indexes
            .iter()
            .find(|c| !c.message_index_offsets.is_empty())
            .unwrap();
        let range = DataWindowTimeRange::new(ArrivalTime(1000), ArrivalTime(1031)).unwrap();
        let start = chunk.compressed_data_offset().unwrap() as usize;
        let raw = viewer_mcap::decode_mcap_chunk(
            "zstd",
            Bytes::copy_from_slice(&bytes[start..start + chunk.compressed_size as usize]),
            chunk.uncompressed_size as usize,
        )
        .unwrap();
        use std::io::Write;
        let mut encoder = lz4_flex::frame::FrameEncoder::new(Vec::new());
        encoder.write_all(&raw).unwrap();
        let encoded = Bytes::from(encoder.finish().unwrap());
        let request = ChunkReadRequest {
            offset: 0,
            length: encoded.len() as u64,
            compression: "lz4".into(),
            uncompressed_size: raw.len() as u64,
        };
        let (backing, decompressed) = decode_chunk_backing(&request, encoded).unwrap();
        assert_eq!(decompressed, raw.len());
        let parsed = parse_chunk_backing(
            backing.clone(),
            &summary.channels.keys().copied().collect(),
            range,
            |_, _| Ok(true),
        )
        .unwrap();
        assert!(!parsed.messages.is_empty());
        assert_eq!(parsed.backing.as_ptr(), backing.as_ptr());
        assert!(
            parsed
                .messages
                .iter()
                .all(|m| m.payload.as_ptr() >= backing.as_ptr()
                    && m.payload.as_ptr() < backing.as_ptr().wrapping_add(backing.len()))
        );
        let message = &parsed.messages[0];
        let selection = RestoreChunkSelection {
            exact: vec![(
                message.stream_id.0 as u16,
                message.arrival_time.0 as u64,
                parsed.message_offsets[0],
            )],
            ..Default::default()
        };
        let mut work = RestoreWork::new(std::rc::Rc::new(std::cell::Cell::new(0)), 0);
        let restored = parse_chunk_backing(
            backing,
            &summary.channels.keys().copied().collect(),
            range,
            |message, offset| {
                if selection.contains(message, offset, ArrivalTime(1030)) {
                    work.admit_message()?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            },
        )
        .unwrap();
        assert_eq!(restored.messages.len(), 1);
        assert_eq!(restored.messages[0].arrival_time, message.arrival_time);
    }

    fn first_parsed_chunk(bytes: &[u8]) -> ParsedChunk {
        let summary = Summary::read(bytes).unwrap().unwrap();
        let range = DataWindowTimeRange::new(ArrivalTime(1_000), ArrivalTime(1_031)).unwrap();
        let mut collector =
            OwnedWindowCollector::new(&summary, &["/camera".to_owned()], range).unwrap();
        let request = collector.next_read().unwrap();
        let file_range = validate_range(
            bytes.len() as u64,
            request.offset,
            request.bulk_length().unwrap(),
        )
        .unwrap();
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
            |message, offset| {
                if !selection.contains(message, offset, ArrivalTime(1_030)) {
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
        let file_range = validate_range(
            bytes.len() as u64,
            request.offset,
            request.bulk_length().unwrap(),
        )
        .unwrap();
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
        let jpeg_start = image.data.as_ptr() as usize;

        assert!(raw_start >= parsed.backing.as_ptr() as usize);
        assert!(raw_end <= parsed.backing.as_ptr() as usize + parsed.backing.len());
        assert!(jpeg_start >= raw_start);
        assert!(jpeg_start + image.data.len() <= raw_end);
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
        decompressed_bytes: u64,
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
            let range =
                validate_range(file_size, request.offset, request.bulk_length().unwrap()).unwrap();
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
                source_bytes: range_bytes as u64,
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
