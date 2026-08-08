#[cfg(target_arch = "wasm32")]
use super::client::RemoteBatchRequest;
use super::client::{RemoteApiClient, RemoteBatchPage};
use std::{cell::RefCell, collections::VecDeque, error::Error, fmt, rc::Rc, time::Duration};
use viewer_core::{ArrivalTime, RawMessage, StreamId};
use viewer_remote_protocol::BatchDecoder;

#[cfg(target_arch = "wasm32")]
use {js_sys::Date, wasm_bindgen_futures::spawn_local, web_sys::AbortController};

const DEFAULT_WINDOW: Duration = Duration::from_secs(1);
const DEFAULT_PREFETCH: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RemoteSourceStatus {
    Idle,
    Fetching {
        start: ArrivalTime,
        end: ArrivalTime,
    },
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RemoteSourceMetrics {
    pub requests: u64,
    pub pages: u64,
    pub bytes_received: u64,
    pub messages_received: u64,
    pub request_latency_ms: f64,
    pub last_window_latency_ms: f64,
    pub buffering_count: u64,
    pub stale_results_discarded: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteSourceError(String);

impl RemoteSourceError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for RemoteSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for RemoteSourceError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FetchWindow {
    generation: u64,
    start: ArrivalTime,
    end: ArrivalTime,
}

#[derive(Debug)]
struct CompletedWindow {
    messages: Vec<RawMessage>,
    pages: u64,
    bytes: u64,
    latency_ms: f64,
}

#[derive(Debug)]
struct FetchResult {
    window: FetchWindow,
    result: Result<CompletedWindow, RemoteSourceError>,
}

pub(crate) struct RemoteBatchSource {
    client: RemoteApiClient,
    recording_id: String,
    revision: String,
    selected_streams: Vec<u32>,
    ready_messages: VecDeque<RawMessage>,
    complete_until: ArrivalTime,
    recording_end_exclusive: ArrivalTime,
    in_flight: Option<FetchWindow>,
    generation: u64,
    inbox: Rc<RefCell<VecDeque<FetchResult>>>,
    window_size: Duration,
    prefetch_duration: Duration,
    status: RemoteSourceStatus,
    metrics: RemoteSourceMetrics,
    #[cfg(target_arch = "wasm32")]
    abort: Option<AbortController>,
}

impl RemoteBatchSource {
    pub(crate) fn new(
        client: RemoteApiClient,
        recording_id: String,
        revision: String,
        selected_streams: Vec<u32>,
        start: ArrivalTime,
        recording_end_exclusive: ArrivalTime,
    ) -> Result<Self, RemoteSourceError> {
        if selected_streams.is_empty() {
            return Err(RemoteSourceError::new("remote stream selection is empty"));
        }
        if start >= recording_end_exclusive {
            return Err(RemoteSourceError::new("remote source range is empty"));
        }
        Ok(Self {
            client,
            recording_id,
            revision,
            selected_streams,
            ready_messages: VecDeque::new(),
            complete_until: start,
            recording_end_exclusive,
            in_flight: None,
            generation: 0,
            inbox: Rc::new(RefCell::new(VecDeque::new())),
            window_size: DEFAULT_WINDOW,
            prefetch_duration: DEFAULT_PREFETCH,
            status: RemoteSourceStatus::Idle,
            metrics: RemoteSourceMetrics::default(),
            #[cfg(target_arch = "wasm32")]
            abort: None,
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn ensure_available_through(&mut self, target: ArrivalTime) {
        if self.in_flight.is_some()
            || matches!(self.status, RemoteSourceStatus::Failed(_))
            || target < self.complete_until
            || self.complete_until >= self.recording_end_exclusive
        {
            return;
        }
        let start = self.complete_until;
        let end = ArrivalTime(
            start
                .0
                .saturating_add(duration_ns(self.window_size))
                .min(self.recording_end_exclusive.0),
        );
        self.generation = self.generation.wrapping_add(1);
        let window = FetchWindow {
            generation: self.generation,
            start,
            end,
        };
        let controller = match AbortController::new() {
            Ok(controller) => controller,
            Err(error) => {
                self.status = RemoteSourceStatus::Failed(format!(
                    "could not create AbortController: {error:?}"
                ));
                return;
            }
        };
        let signal = controller.signal();
        self.abort = Some(controller);
        self.in_flight = Some(window);
        self.status = RemoteSourceStatus::Fetching { start, end };
        self.metrics.requests = self.metrics.requests.saturating_add(1);

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
                window,
                &signal,
            )
            .await;
            inbox.borrow_mut().push_back(FetchResult { window, result });
        });
    }

    pub(crate) fn poll_completed_fetch(&mut self) -> Result<bool, RemoteSourceError> {
        let mut changed = false;
        loop {
            let result = self.inbox.borrow_mut().pop_front();
            let Some(result) = result else {
                break;
            };
            if result.window.generation != self.generation || self.in_flight != Some(result.window)
            {
                self.metrics.stale_results_discarded =
                    self.metrics.stale_results_discarded.saturating_add(1);
                continue;
            }
            self.in_flight = None;
            #[cfg(target_arch = "wasm32")]
            {
                self.abort = None;
            }
            match result.result {
                Ok(completed) => {
                    if result.window.start != self.complete_until {
                        let error = RemoteSourceError::new(
                            "completed remote window does not follow committed data",
                        );
                        self.status = RemoteSourceStatus::Failed(error.to_string());
                        return Err(error);
                    }
                    let message_count = completed.messages.len() as u64;
                    self.ready_messages.extend(completed.messages);
                    self.complete_until = result.window.end;
                    self.metrics.pages = self.metrics.pages.saturating_add(completed.pages);
                    self.metrics.bytes_received =
                        self.metrics.bytes_received.saturating_add(completed.bytes);
                    self.metrics.messages_received =
                        self.metrics.messages_received.saturating_add(message_count);
                    self.metrics.last_window_latency_ms = completed.latency_ms;
                    self.metrics.request_latency_ms = completed.latency_ms;
                    self.status = RemoteSourceStatus::Ready;
                    changed = true;
                }
                Err(error) => {
                    self.status = RemoteSourceStatus::Failed(error.to_string());
                    return Err(error);
                }
            }
        }
        Ok(changed)
    }

    pub(crate) fn is_complete_through(&self, target: ArrivalTime) -> bool {
        target < self.complete_until
            || (target == ArrivalTime(self.recording_end_exclusive.0 - 1)
                && self.complete_until == self.recording_end_exclusive)
    }

    pub(crate) fn drain_through(&mut self, target: ArrivalTime) -> Vec<RawMessage> {
        let count = self
            .ready_messages
            .partition_point(|message| message.arrival_time <= target);
        self.ready_messages.drain(..count).collect()
    }

    pub(crate) fn prefetch_duration(&self) -> Duration {
        self.prefetch_duration
    }

    pub(crate) fn metrics(&self) -> &RemoteSourceMetrics {
        &self.metrics
    }

    pub(crate) fn note_buffering(&mut self) {
        self.metrics.buffering_count = self.metrics.buffering_count.saturating_add(1);
    }

    pub(crate) fn complete_until(&self) -> ArrivalTime {
        self.complete_until
    }

    #[cfg(test)]
    pub(crate) fn inject_completed_window(&mut self, end: ArrivalTime, messages: Vec<RawMessage>) {
        self.generation = self.generation.wrapping_add(1);
        let window = FetchWindow {
            generation: self.generation,
            start: self.complete_until,
            end,
        };
        self.in_flight = Some(window);
        self.inbox.borrow_mut().push_back(FetchResult {
            window,
            result: Ok(CompletedWindow {
                pages: 1,
                bytes: messages
                    .iter()
                    .map(|message| message.payload.len() as u64)
                    .sum(),
                messages,
                latency_ms: 1.0,
            }),
        });
    }
}

impl Drop for RemoteBatchSource {
    fn drop(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        #[cfg(target_arch = "wasm32")]
        if let Some(controller) = self.abort.take() {
            controller.abort();
        }
    }
}

struct WindowAssembler {
    messages: Vec<RawMessage>,
    pages: u64,
    bytes: u64,
    previous_key: Option<(i64, u32)>,
}

impl WindowAssembler {
    fn new() -> Self {
        Self {
            messages: Vec::new(),
            pages: 0,
            bytes: 0,
            previous_key: None,
        }
    }

    fn push_page(&mut self, page: RemoteBatchPage) -> Result<bool, RemoteSourceError> {
        let decoded = BatchDecoder::new(&page.body)
            .and_then(BatchDecoder::collect)
            .map_err(|error| RemoteSourceError::new(error.to_string()))?;
        for message in decoded {
            let arrival = i64::try_from(message.log_time_ns).map_err(|_| {
                RemoteSourceError::new("remote timestamp exceeds signed nanoseconds")
            })?;
            let key = (arrival, message.stream_id);
            if self.previous_key.is_some_and(|previous| key < previous) {
                return Err(RemoteSourceError::new(
                    "remote window messages are not time ordered",
                ));
            }
            let payload_range = message.payload_range_in(&page.body).ok_or_else(|| {
                RemoteSourceError::new("decoded payload is outside its batch body")
            })?;
            self.previous_key = Some(key);
            self.messages.push(RawMessage {
                stream_id: StreamId(message.stream_id),
                arrival_time: ArrivalTime(arrival),
                payload: page.body.slice(payload_range),
            });
        }
        self.pages = self.pages.saturating_add(1);
        self.bytes = self.bytes.saturating_add(page.body.len() as u64);
        Ok(page.complete)
    }

    fn finish(self, latency_ms: f64) -> CompletedWindow {
        CompletedWindow {
            messages: self.messages,
            pages: self.pages,
            bytes: self.bytes,
            latency_ms,
        }
    }
}

#[cfg(target_arch = "wasm32")]
async fn fetch_complete_window(
    client: &RemoteApiClient,
    recording_id: &str,
    revision: &str,
    stream_ids: Vec<u32>,
    window: FetchWindow,
    signal: &web_sys::AbortSignal,
) -> Result<CompletedWindow, RemoteSourceError> {
    let start_ns = u64::try_from(window.start.0)
        .map_err(|_| RemoteSourceError::new("negative remote window start"))?;
    let end_ns = u64::try_from(window.end.0)
        .map_err(|_| RemoteSourceError::new("negative remote window end"))?;
    let started = Date::now();
    let mut assembler = WindowAssembler::new();
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
            .map_err(|error| RemoteSourceError::new(error.to_string()))?;
        cursor = page.next_cursor.clone();
        if assembler.push_page(page)? {
            return Ok(assembler.finish(Date::now() - started));
        }
    }
}

fn duration_ns(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_remote_protocol::{BatchEncoder, RemoteMessageRef};

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
    fn continuation_pages_remain_staged_until_complete() {
        let mut assembler = WindowAssembler::new();
        let first = page(&[(1, 10, b"a")], false);
        let first_body = first.body.clone();
        assert!(!assembler.push_page(first).unwrap());
        assert_eq!(assembler.messages.len(), 1);
        assert_eq!(
            assembler.messages[0].payload.as_ptr(),
            first_body[44..].as_ptr(),
            "RawMessage must retain a Bytes slice rather than copying payload bytes"
        );
        assert!(assembler.push_page(page(&[(1, 11, b"b")], true)).unwrap());
        let completed = assembler.finish(5.0);
        assert_eq!(completed.messages.len(), 2);
        assert_eq!(completed.pages, 2);
    }

    #[test]
    fn rejects_timestamp_overflow_and_cross_page_disorder() {
        let mut overflow = WindowAssembler::new();
        assert!(
            overflow
                .push_page(page(&[(1, i64::MAX as u64 + 1, b"x")], true))
                .is_err()
        );

        let mut unordered = WindowAssembler::new();
        unordered.push_page(page(&[(2, 20, b"a")], false)).unwrap();
        assert!(unordered.push_page(page(&[(1, 19, b"b")], true)).is_err());
    }

    #[test]
    fn stale_window_result_does_not_advance_complete_until() {
        let client = RemoteApiClient::new("http://localhost").unwrap();
        let mut source = RemoteBatchSource::new(
            client,
            "demo".into(),
            "revision".into(),
            vec![1],
            ArrivalTime(10),
            ArrivalTime(30),
        )
        .unwrap();
        let stale = FetchWindow {
            generation: 1,
            start: ArrivalTime(10),
            end: ArrivalTime(20),
        };
        source.generation = 2;
        source.in_flight = Some(FetchWindow {
            generation: 2,
            start: ArrivalTime(10),
            end: ArrivalTime(20),
        });
        source.inbox.borrow_mut().push_back(FetchResult {
            window: stale,
            result: Ok(CompletedWindow {
                messages: vec![],
                pages: 1,
                bytes: 16,
                latency_ms: 1.0,
            }),
        });

        assert!(!source.poll_completed_fetch().unwrap());
        assert_eq!(source.complete_until(), ArrivalTime(10));
        assert_eq!(source.metrics().stale_results_discarded, 1);
    }

    #[test]
    fn empty_complete_window_advances_exclusive_completeness() {
        let client = RemoteApiClient::new("http://localhost").unwrap();
        let mut source = RemoteBatchSource::new(
            client,
            "demo".into(),
            "revision".into(),
            vec![1],
            ArrivalTime(10),
            ArrivalTime(30),
        )
        .unwrap();

        source.inject_completed_window(ArrivalTime(20), vec![]);
        assert!(source.poll_completed_fetch().unwrap());
        assert_eq!(source.complete_until(), ArrivalTime(20));
        assert!(source.is_complete_through(ArrivalTime(19)));
        assert!(!source.is_complete_through(ArrivalTime(20)));
        assert!(source.drain_through(ArrivalTime(19)).is_empty());
    }
}
