#[cfg(target_arch = "wasm32")]
use super::client::RemoteBatchRequest;
use super::client::{RemoteApiClient, RemoteBatchPage};
use crate::data_plane::{
    DataLoadError, LoadedWindow, WindowLoadDiagnostics, WindowLoader, WindowLoaderMetrics,
};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use viewer_core::{DataWindowTimeRange, RawMessage, SerializedWindow, StreamId};
use viewer_remote_protocol::BatchDecoder;

#[cfg(target_arch = "wasm32")]
use {js_sys::Date, wasm_bindgen_futures::spawn_local, web_sys::AbortController};

#[derive(Debug)]
pub(crate) enum WindowLoadState {
    Idle,
    Loading {
        generation: u64,
        range: DataWindowTimeRange,
    },
    Ready(LoadedWindow),
    Failed(DataLoadError),
}

#[derive(Debug)]
struct FetchResult {
    generation: u64,
    range: DataWindowTimeRange,
    result: Result<LoadedWindow, DataLoadError>,
}

pub(crate) struct RemoteWindowLoader {
    client: RemoteApiClient,
    recording_id: String,
    revision: String,
    selected_streams: Vec<u32>,
    generation: u64,
    state: WindowLoadState,
    inbox: Rc<RefCell<VecDeque<FetchResult>>>,
    metrics: WindowLoaderMetrics,
    #[cfg(target_arch = "wasm32")]
    abort: Option<AbortController>,
}

impl RemoteWindowLoader {
    pub(crate) fn new(
        client: RemoteApiClient,
        recording_id: String,
        revision: String,
        selected_streams: Vec<u32>,
    ) -> Result<Self, DataLoadError> {
        if selected_streams.is_empty() {
            return Err(DataLoadError::new("remote stream selection is empty"));
        }
        Ok(Self {
            client,
            recording_id,
            revision,
            selected_streams,
            generation: 0,
            state: WindowLoadState::Idle,
            inbox: Rc::new(RefCell::new(VecDeque::new())),
            metrics: WindowLoaderMetrics::default(),
            #[cfg(target_arch = "wasm32")]
            abort: None,
        })
    }

    pub(crate) fn start(&mut self, range: DataWindowTimeRange) -> Result<(), DataLoadError> {
        if !matches!(self.state, WindowLoadState::Idle) {
            return Err(DataLoadError::new("remote window loader is not idle"));
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.state = WindowLoadState::Loading { generation, range };
        self.metrics.load_requests = self.metrics.load_requests.saturating_add(1);

        #[cfg(target_arch = "wasm32")]
        {
            let controller = AbortController::new().map_err(|error| {
                let error =
                    DataLoadError::new(format!("could not create AbortController: {error:?}"));
                self.state = WindowLoadState::Failed(error.clone());
                error
            })?;
            let signal = controller.signal();
            self.abort = Some(controller);
            let client = self.client.clone();
            let recording_id = self.recording_id.clone();
            let revision = self.revision.clone();
            let selected_streams = self.selected_streams.clone();
            let inbox = Rc::clone(&self.inbox);
            spawn_local(async move {
                let result = fetch_complete_window(
                    &client,
                    &recording_id,
                    &revision,
                    selected_streams,
                    range,
                    &signal,
                )
                .await;
                inbox.borrow_mut().push_back(FetchResult {
                    generation,
                    range,
                    result,
                });
            });
        }
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> Result<bool, DataLoadError> {
        if let WindowLoadState::Failed(error) = &self.state {
            return Err(error.clone());
        }
        let mut changed = false;
        loop {
            let result = self.inbox.borrow_mut().pop_front();
            let Some(result) = result else {
                break;
            };
            let current = matches!(
                self.state,
                WindowLoadState::Loading { generation, range }
                    if generation == result.generation && range == result.range
            );
            if result.generation != self.generation || !current {
                self.metrics.stale_results_discarded =
                    self.metrics.stale_results_discarded.saturating_add(1);
                continue;
            }
            #[cfg(target_arch = "wasm32")]
            {
                self.abort = None;
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
                    self.metrics.messages_loaded = self
                        .metrics
                        .messages_loaded
                        .saturating_add(loaded.window.messages.len() as u64);
                    self.metrics.last_window_latency_ms = loaded.diagnostics.latency_ms;
                    self.metrics.last_processing_ms = loaded.diagnostics.processing_ms;
                    self.metrics.request_latency_ms = loaded.diagnostics.latency_ms;
                    self.state = WindowLoadState::Ready(loaded);
                    changed = true;
                }
                Err(error) => {
                    self.state = WindowLoadState::Failed(error.clone());
                    return Err(error);
                }
            }
        }
        Ok(changed)
    }

    pub(crate) fn take_ready(&mut self) -> Option<LoadedWindow> {
        let state = std::mem::replace(&mut self.state, WindowLoadState::Idle);
        match state {
            WindowLoadState::Ready(loaded) => Some(loaded),
            other => {
                self.state = other;
                None
            }
        }
    }

    pub(crate) fn is_idle(&self) -> bool {
        matches!(self.state, WindowLoadState::Idle)
    }

    pub(crate) fn metrics(&self) -> &WindowLoaderMetrics {
        &self.metrics
    }

    #[cfg(test)]
    pub(crate) fn inject_loaded(&mut self, loaded: LoadedWindow) -> Result<(), DataLoadError> {
        let WindowLoadState::Loading { generation, range } = self.state else {
            return Err(DataLoadError::new("test loader is not loading"));
        };
        self.inbox.borrow_mut().push_back(FetchResult {
            generation,
            range,
            result: Ok(loaded),
        });
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn inject_stale(&mut self, loaded: LoadedWindow) -> Result<(), DataLoadError> {
        let WindowLoadState::Loading { generation, range } = self.state else {
            return Err(DataLoadError::new("test loader is not loading"));
        };
        self.inbox.borrow_mut().push_back(FetchResult {
            generation: generation.wrapping_sub(1),
            range,
            result: Ok(loaded),
        });
        Ok(())
    }
}

impl Drop for RemoteWindowLoader {
    fn drop(&mut self) {
        WindowLoader::cancel(self);
    }
}

impl WindowLoader for RemoteWindowLoader {
    fn request(&mut self, range: DataWindowTimeRange) -> Result<(), DataLoadError> {
        self.start(range)
    }

    fn poll(&mut self) -> Option<Result<LoadedWindow, DataLoadError>> {
        match RemoteWindowLoader::poll(self) {
            Ok(_) => self.take_ready().map(Ok),
            Err(error) => Some(Err(error)),
        }
    }

    fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        #[cfg(target_arch = "wasm32")]
        if let Some(controller) = self.abort.take() {
            controller.abort();
        }
        self.state = WindowLoadState::Idle;
    }

    fn is_idle(&self) -> bool {
        RemoteWindowLoader::is_idle(self)
    }

    fn metrics(&self) -> &WindowLoaderMetrics {
        RemoteWindowLoader::metrics(self)
    }
}

struct WindowAssembler {
    range: DataWindowTimeRange,
    messages: Vec<RawMessage>,
    pages: u64,
    source_bytes: usize,
    resident_bytes: usize,
    previous_key: Option<(i64, u32)>,
    complete: bool,
}

#[cfg(test)]
pub(crate) fn assemble_pages_for_test(
    range: DataWindowTimeRange,
    pages: impl IntoIterator<Item = RemoteBatchPage>,
) -> Result<LoadedWindow, DataLoadError> {
    let mut assembler = WindowAssembler::new(range);
    for page in pages {
        assembler.push_page(page)?;
    }
    assembler.finish(0.0)
}

impl WindowAssembler {
    fn new(range: DataWindowTimeRange) -> Self {
        Self {
            range,
            messages: Vec::new(),
            pages: 0,
            source_bytes: 0,
            resident_bytes: 0,
            previous_key: None,
            complete: false,
        }
    }

    fn push_page(&mut self, page: RemoteBatchPage) -> Result<bool, DataLoadError> {
        if self.complete {
            return Err(DataLoadError::new(
                "remote window received a page after completion",
            ));
        }
        let decoded = BatchDecoder::new(&page.body)
            .and_then(BatchDecoder::collect)
            .map_err(|error| DataLoadError::new(error.to_string()))?;
        let retained_page_bytes = if decoded.is_empty() {
            0
        } else {
            page.body.len()
        };
        for message in decoded {
            let arrival = i64::try_from(message.log_time_ns)
                .map_err(|_| DataLoadError::new("remote timestamp exceeds signed nanoseconds"))?;
            let key = (arrival, message.stream_id);
            if self.previous_key.is_some_and(|previous| key < previous) {
                return Err(DataLoadError::new(
                    "remote window messages are not time ordered",
                ));
            }
            let payload_range = message
                .payload_range_in(&page.body)
                .ok_or_else(|| DataLoadError::new("decoded payload is outside its batch body"))?;
            self.previous_key = Some(key);
            self.messages.push(RawMessage {
                stream_id: StreamId(message.stream_id),
                arrival_time: viewer_core::ArrivalTime(arrival),
                payload: page.body.slice(payload_range),
            });
        }
        self.pages = self.pages.saturating_add(1);
        self.source_bytes = self
            .source_bytes
            .checked_add(page.body.len())
            .ok_or_else(|| DataLoadError::new("remote source byte count overflow"))?;
        self.resident_bytes = self
            .resident_bytes
            .checked_add(retained_page_bytes)
            .ok_or_else(|| DataLoadError::new("remote resident byte count overflow"))?;
        self.complete = page.complete;
        Ok(self.complete)
    }

    fn finish(self, latency_ms: f64) -> Result<LoadedWindow, DataLoadError> {
        if !self.complete {
            return Err(DataLoadError::new(
                "remote window cannot be published before continuation completes",
            ));
        }
        let window = SerializedWindow::new(self.range, self.messages, self.resident_bytes)
            .map_err(|error| DataLoadError::new(error.to_string()))?;
        Ok(LoadedWindow {
            window,
            diagnostics: WindowLoadDiagnostics {
                source_reads: self.pages,
                source_bytes: self.source_bytes,
                latency_ms,
                processing_ms: 0.0,
            },
        })
    }
}

#[cfg(target_arch = "wasm32")]
async fn fetch_complete_window(
    client: &RemoteApiClient,
    recording_id: &str,
    revision: &str,
    stream_ids: Vec<u32>,
    range: DataWindowTimeRange,
    signal: &web_sys::AbortSignal,
) -> Result<LoadedWindow, DataLoadError> {
    let start_ns = u64::try_from(range.start.0)
        .map_err(|_| DataLoadError::new("negative remote window start"))?;
    let end_ns = u64::try_from(range.end_exclusive.0)
        .map_err(|_| DataLoadError::new("negative remote window end"))?;
    let started = Date::now();
    let mut assembler = WindowAssembler::new(range);
    let mut cursor = None;
    loop {
        let page = client
            .fetch_batch_page(
                &RemoteBatchRequest {
                    recording_id: recording_id.to_owned(),
                    revision: revision.to_owned(),
                    stream_ids: stream_ids.clone(),
                    start_ns,
                    end_ns,
                    max_bytes: None,
                    max_messages: None,
                    cursor,
                },
                Some(signal),
            )
            .await
            .map_err(|error| DataLoadError::new(error.to_string()))?;
        cursor = page.next_cursor.clone();
        if assembler.push_page(page)? {
            return assembler.finish(Date::now() - started);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::ArrivalTime;
    use viewer_remote_protocol::{BatchEncoder, RemoteMessageRef};

    fn range() -> DataWindowTimeRange {
        DataWindowTimeRange::new(ArrivalTime(10), ArrivalTime(20)).unwrap()
    }

    fn page(messages: &[(u32, u64, &[u8])], complete: bool) -> RemoteBatchPage {
        let mut encoder = BatchEncoder::new();
        for (sequence, (stream_id, time, payload)) in messages.iter().enumerate() {
            encoder
                .push(RemoteMessageRef {
                    stream_id: *stream_id,
                    sequence: sequence as u32,
                    log_time_ns: *time,
                    publish_time_ns: *time,
                    payload,
                })
                .unwrap();
        }
        RemoteBatchPage {
            body: encoder.finish(),
            complete,
            next_cursor: (!complete).then(|| "next".into()),
            message_count: messages.len(),
            recording_revision: "revision".into(),
        }
    }

    #[test]
    fn continuation_pages_only_finish_as_one_serialized_window() {
        let mut partial = WindowAssembler::new(range());
        assert!(!partial.push_page(page(&[(1, 10, b"a")], false)).unwrap());
        assert!(partial.finish(1.0).is_err());

        let mut assembler = WindowAssembler::new(range());
        let first = page(&[(1, 10, b"a")], false);
        let first_body = first.body.clone();
        assert!(!assembler.push_page(first).unwrap());
        assert!(assembler.push_page(page(&[(1, 11, b"b")], true)).unwrap());

        let loaded = assembler.finish(5.0).unwrap();
        assert_eq!(loaded.window.range, range());
        assert_eq!(loaded.window.messages.len(), 2);
        assert_eq!(loaded.diagnostics.source_reads, 2);
        assert_eq!(
            loaded.window.messages[0].payload.as_ptr(),
            first_body[44..].as_ptr(),
            "RawMessage must retain a Bytes slice rather than copying payload bytes"
        );
    }

    #[test]
    fn rejects_timestamp_overflow_disorder_and_out_of_window_messages() {
        let mut overflow = WindowAssembler::new(range());
        assert!(
            overflow
                .push_page(page(&[(1, i64::MAX as u64 + 1, b"x")], true))
                .is_err()
        );

        let mut unordered = WindowAssembler::new(range());
        unordered.push_page(page(&[(2, 19, b"a")], false)).unwrap();
        assert!(unordered.push_page(page(&[(1, 18, b"b")], true)).is_err());

        let mut outside = WindowAssembler::new(range());
        assert!(outside.push_page(page(&[(1, 20, b"x")], true)).unwrap());
        assert!(outside.finish(1.0).is_err());
    }

    #[test]
    fn empty_complete_batch_advances_without_retaining_its_body() {
        let mut assembler = WindowAssembler::new(range());
        let empty = page(&[], true);
        let source_bytes = empty.body.len();
        assert!(assembler.push_page(empty).unwrap());

        let loaded = assembler.finish(1.0).unwrap();
        assert!(loaded.window.messages.is_empty());
        assert_eq!(loaded.window.resident_bytes, 0);
        assert_eq!(loaded.diagnostics.source_bytes, source_bytes);
    }
}
