//! Fixed-range recording analysis. No UI state, playback cursor, or feature routing.
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::JoinHandle,
};
use viewer_core::{
    ArrivalTime, DataWindowTimeRange, FieldDecodeBudget, FieldDecodeLimits, InspectorMessage,
    InspectorSchema, RosMessageSchema, SelectedValue, SourceCatalog, StreamId,
};
use viewer_mcap::{
    McapSource, NumericPlotOverview, NumericSeriesRequest, QueryLimits, RangePageCursor, RangeQuery,
};

pub(crate) const MAX_TOPICS: usize = 8;
pub(crate) const MAX_FIELDS: usize = 8;
pub(crate) const PAGE_ROWS: usize = 64;
pub(crate) const PAGE_BYTES: usize = 4 * 1024 * 1024;
const READ_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SCANNED: usize = 100_000;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueryMode {
    Table,
    Plot,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueryTopic {
    pub stream: StreamId,
    pub fields: Vec<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueryRequest {
    pub topics: Vec<QueryTopic>,
    pub range: DataWindowTimeRange,
    pub mode: QueryMode,
    pub after: Option<RangePageCursor>,
}
pub(crate) struct QueryRow {
    pub stream: StreamId,
    pub time: ArrivalTime,
    pub payload_bytes: usize,
    pub values: Vec<Result<SelectedValue, String>>,
    pub message: Result<Arc<InspectorMessage>, String>,
}
pub(crate) struct QuerySeries {
    pub stream: StreamId,
    pub field: String,
    pub overview: NumericPlotOverview,
}
pub(crate) enum QueryResult {
    Table {
        rows: Vec<QueryRow>,
        next: Option<RangePageCursor>,
    },
    Plot(Vec<QuerySeries>),
}
pub(crate) enum QueryState {
    Unavailable,
    Idle,
    Loading,
    Completed(QueryResult),
    Error(String),
}

/// Synchronous operation shared by both presentations and usable without a panel/worker.
pub(crate) fn execute(
    source: &McapSource,
    request: &QueryRequest,
    cancelled: impl FnMut() -> bool,
) -> Result<QueryResult, String> {
    execute_bounded(source, request, READ_BYTES, cancelled)
}

fn execute_bounded(
    source: &McapSource,
    request: &QueryRequest,
    read_bytes: u64,
    mut cancelled: impl FnMut() -> bool,
) -> Result<QueryResult, String> {
    if request.range.start >= request.range.end_exclusive {
        return Err("Start must precede exclusive end".into());
    }
    if request.topics.is_empty() || request.topics.len() > MAX_TOPICS {
        return Err("Select 1..=8 topics".into());
    }
    let mut seen = BTreeSet::new();
    if request.topics.iter().any(|t| !seen.insert(t.stream)) {
        return Err("Duplicate input stream".into());
    }
    if request.topics.iter().map(|t| t.fields.len()).sum::<usize>() > MAX_FIELDS {
        return Err("At most eight selected fields across all topics".into());
    }
    for t in &request.topics {
        if source.catalog().by_id(t.stream).is_none() {
            return Err(format!("Unknown stream {}", t.stream.0));
        }
        if t.fields.iter().any(|f| f.is_empty() || f.len() > 128) {
            return Err("Field paths must be 1..=128 bytes".into());
        }
    }
    let streams = request.topics.iter().map(|t| t.stream).collect::<Vec<_>>();
    // Plot reads once per stream (grouping all its fields); account for mixed chunks
    // being read again. Table merges all selected streams in one traversal.
    let work = if request.mode == QueryMode::Plot {
        let mut work = 0u64;
        for t in &request.topics {
            if t.fields.is_empty() {
                return Err("Plot requires numeric fields for every selected topic".into());
            }
            work = work
                .checked_add(
                    source
                        .range_work_bytes(&[t.stream], request.range, &mut cancelled)
                        .map_err(|e| e.to_string())?,
                )
                .ok_or("Query work overflow")?;
        }
        work
    } else {
        source
            .range_work_bytes(&streams, request.range, &mut cancelled)
            .map_err(|e| e.to_string())?
    };
    if work > read_bytes {
        return Err("Query exceeds 256 MiB input + expansion work; narrow the range or use smaller recording chunks".into());
    }
    if cancelled() {
        return Err("Query cancelled".into());
    }
    match request.mode {
        QueryMode::Plot => {
            let mut series = Vec::new();
            for topic in &request.topics {
                let requests = topic
                    .fields
                    .iter()
                    .map(|field| NumericSeriesRequest {
                        stream_id: topic.stream,
                        field_path: field.clone(),
                        range: request.range,
                        max_points: 2048,
                    })
                    .collect::<Vec<_>>();
                let batch = source
                    .query_numeric_overviews_cancellable(&requests, &mut cancelled)
                    .map_err(|e| e.to_string())?;
                for (field, overview) in topic.fields.iter().zip(batch.series) {
                    series.push(QuerySeries {
                        stream: topic.stream,
                        field: field.clone(),
                        overview,
                    });
                }
            }
            Ok(QueryResult::Plot(series))
        }
        QueryMode::Table => {
            let plans = request
                .topics
                .iter()
                .map(|topic| {
                    let stream = source
                        .catalog()
                        .by_id(topic.stream)
                        .expect("validated stream");
                    let fields = if topic.fields.is_empty() {
                        Ok(Vec::new())
                    } else {
                        RosMessageSchema::for_stream(stream).and_then(|schema| {
                            topic
                                .fields
                                .iter()
                                .map(|p| schema.compile_field(p))
                                .collect()
                        })
                    }
                    .map_err(|e: viewer_core::DecodeError| e.to_string())?;
                    Ok((
                        topic.stream,
                        fields,
                        InspectorSchema::for_stream(stream).map_err(|e| e.to_string()),
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            let (page, next) = source
                .query_range_page_cancellable(
                    &RangeQuery {
                        streams,
                        range: request.range,
                        limits: QueryLimits::new(PAGE_ROWS, READ_BYTES as usize)
                            .map_err(|e| e.to_string())?,
                    },
                    request.after,
                    MAX_SCANNED,
                    &mut cancelled,
                )
                .map_err(|e| e.to_string())?;
            if page.messages.is_empty() && !page.complete {
                return Err("A message exceeds the query read window budget".into());
            }
            let mut rows = Vec::new();
            let mut detail_bytes = 0;
            for raw in page.messages {
                if cancelled() {
                    return Err("Query cancelled".into());
                }
                let (_, fields, schema) = plans
                    .iter()
                    .find(|(id, _, _)| *id == raw.stream_id)
                    .expect("selected stream");
                let mut budget = FieldDecodeBudget::new(FieldDecodeLimits {
                    max_array_len: 64,
                    max_values: 128,
                    max_text_bytes: 1024,
                    max_total_text_bytes: 2048,
                });
                let values = fields
                    .iter()
                    .map(|f| {
                        f.extract(&raw.payload, &mut budget)
                            .map_err(|e| e.to_string())
                    })
                    .collect();
                // Projection/metadata can still succeed for large images. Only bounded
                // Inspector copies survive the query, never the serialized chunk slices.
                let message = if raw.payload.len() > PAGE_BYTES - detail_bytes {
                    Err("Details exceed the 4 MiB page cache; metadata and selected fields remain available".into())
                } else {
                    schema.as_ref().map_err(Clone::clone).and_then(|schema| {
                        InspectorMessage::new(schema, &raw)
                            .map(Arc::new)
                            .map_err(|e| e.to_string())
                    })
                };
                if let Ok(message) = &message {
                    detail_bytes += message.retained_payload_bytes();
                }
                rows.push(QueryRow {
                    stream: raw.stream_id,
                    time: raw.arrival_time,
                    payload_bytes: raw.payload.len(),
                    values,
                    message,
                });
            }
            Ok(QueryResult::Table { rows, next })
        }
    }
}
struct Job {
    generation: u64,
    source: McapSource,
    request: QueryRequest,
}
#[derive(Default)]
struct Mailbox {
    pending: Option<Job>,
    result: Option<(u64, Result<QueryResult, String>)>,
    shutdown: bool,
}
struct Worker {
    mailbox: Arc<(Mutex<Mailbox>, Condvar)>,
    handle: Option<JoinHandle<()>>,
}
pub(crate) struct QueryService {
    source: Option<McapSource>,
    pub state: QueryState,
    generation: Arc<AtomicU64>,
    worker: Option<Worker>,
}
impl Default for QueryService {
    fn default() -> Self {
        Self {
            source: None,
            state: QueryState::Unavailable,
            generation: Arc::new(AtomicU64::new(0)),
            worker: None,
        }
    }
}
impl QueryService {
    pub fn bind_source(&mut self, source: Option<&McapSource>) {
        self.cancel();
        self.source = source.map(McapSource::fork_reader);
        self.state = if self.source.is_some() {
            QueryState::Idle
        } else {
            QueryState::Unavailable
        };
    }
    pub fn catalog(&self) -> Option<&SourceCatalog> {
        self.source.as_ref().map(McapSource::catalog)
    }
    pub fn range(&self) -> Option<viewer_core::RecordingTimeRange> {
        self.source.as_ref().map(McapSource::time_range)
    }
    pub fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Some(w) = &self.worker {
            let mut m = w.mailbox.0.lock().expect("query mailbox");
            m.pending = None;
            m.result = None;
        }
        self.state = if self.source.is_some() {
            QueryState::Idle
        } else {
            QueryState::Unavailable
        };
    }
    pub fn submit(&mut self, request: QueryRequest) {
        self.cancel();
        let Some(source) = &self.source else {
            return;
        };
        if self.worker.is_none() {
            let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
            let thread_shared = shared.clone();
            let generation = self.generation.clone();
            let handle = std::thread::Builder::new()
                .name("recording-query".into())
                .spawn(move || {
                    loop {
                        let job = {
                            let (lock, ready) = &*thread_shared;
                            let mut m = lock.lock().expect("query mailbox");
                            while m.pending.is_none() && !m.shutdown {
                                m = ready.wait(m).expect("query mailbox");
                            }
                            if m.shutdown {
                                return;
                            }
                            m.pending.take().expect("pending job")
                        };
                        let cancelled = || generation.load(Ordering::Acquire) != job.generation;
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            execute(&job.source, &job.request, cancelled)
                        }))
                        .unwrap_or_else(|_| Err("Query worker panicked".into()));
                        let mut m = thread_shared.0.lock().expect("query mailbox");
                        if !m.shutdown && !cancelled() {
                            m.result = Some((job.generation, result));
                        }
                    }
                });
            match handle {
                Ok(handle) => {
                    self.worker = Some(Worker {
                        mailbox: shared,
                        handle: Some(handle),
                    })
                }
                Err(e) => {
                    self.state = QueryState::Error(format!("Start query worker: {e}"));
                    return;
                }
            }
        }
        let w = self.worker.as_ref().expect("worker started");
        let mut m = w.mailbox.0.lock().expect("query mailbox");
        m.pending = Some(Job {
            generation: self.generation.load(Ordering::Acquire),
            source: source.fork_reader(),
            request,
        });
        self.state = QueryState::Loading;
        w.mailbox.1.notify_one();
    }
    pub fn poll(&mut self) {
        let Some(w) = &self.worker else {
            return;
        };
        if let Some((generation, result)) = w.mailbox.0.lock().expect("query mailbox").result.take()
            && generation == self.generation.load(Ordering::Acquire)
        {
            self.state = match result {
                Ok(result) => QueryState::Completed(result),
                Err(error) => QueryState::Error(error),
            };
        }
    }
}
impl Drop for QueryService {
    fn drop(&mut self) {
        self.cancel();
        if let Some(mut w) = self.worker.take() {
            {
                let mut m = w.mailbox.0.lock().expect("query mailbox");
                m.shutdown = true;
                w.mailbox.1.notify_one();
            }
            if let Some(handle) = w.handle.take() {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        io::Cursor,
        time::{Duration, Instant},
    };
    pub(super) fn fixture(count: u32, offset: u32, equal_times: bool, huge: bool) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = mcap::Writer::with_options(
                &mut bytes,
                mcap::WriteOptions::new()
                    .compression(None)
                    .chunk_size(Some(1024)),
            )
            .unwrap();
            let schema = writer
                .add_schema(
                    "example_msgs/msg/Value",
                    "ros2msg",
                    if huge {
                        b"uint32 value\nuint8[] padding\n".as_slice()
                    } else {
                        b"uint32 value\n".as_slice()
                    },
                )
                .unwrap();
            let a = writer
                .add_channel(schema, "/a", "cdr", &BTreeMap::new())
                .unwrap();
            let b = writer
                .add_channel(schema, "/b", "cdr", &BTreeMap::new())
                .unwrap();
            for i in 0..count {
                for channel in [a, b] {
                    let mut data = vec![0, 1, 0, 0];
                    data.extend((offset + i).to_le_bytes());
                    if huge {
                        data.extend(((PAGE_BYTES + 1 - 12) as u32).to_le_bytes());
                        data.resize(PAGE_BYTES + 1, 0);
                    }
                    writer
                        .write_to_known_channel(
                            &mcap::records::MessageHeader {
                                channel_id: channel,
                                sequence: i,
                                log_time: if equal_times { 100 } else { 100 + u64::from(i) },
                                publish_time: 50,
                            },
                            &data,
                        )
                        .unwrap();
                }
            }
            writer.finish().unwrap();
        }
        bytes.into_inner()
    }
    fn source(count: u32, offset: u32, ties: bool) -> McapSource {
        McapSource::from_bytes(fixture(count, offset, ties, false).into()).unwrap()
    }
    fn request(source: &McapSource, mode: QueryMode) -> QueryRequest {
        QueryRequest {
            topics: source
                .catalog()
                .streams
                .iter()
                .map(|s| QueryTopic {
                    stream: s.id,
                    fields: vec!["value".into()],
                })
                .collect(),
            range: DataWindowTimeRange::new(ArrivalTime(100), ArrivalTime(10_000)).unwrap(),
            mode,
            after: None,
        }
    }
    fn wait(service: &mut QueryService) {
        let until = Instant::now() + Duration::from_secs(5);
        while matches!(service.state, QueryState::Loading) {
            service.poll();
            assert!(Instant::now() < until, "query timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn table_paginates_equal_times_without_losing_or_repeating_messages() {
        let source = source(80, 0, true);
        let mut r = request(&source, QueryMode::Table);
        let mut values = Vec::new();
        loop {
            let QueryResult::Table { rows, next } = execute(&source, &r, || false).unwrap() else {
                panic!()
            };
            assert!(rows.len() <= PAGE_ROWS);
            for row in rows {
                assert_eq!(row.time, ArrivalTime(100));
                assert_eq!(row.message.as_ref().unwrap().retained_payload_bytes(), 8);
                let SelectedValue::Scalar(viewer_core::MessageScalar::Unsigned(value)) =
                    row.values[0].as_ref().unwrap()
                else {
                    panic!()
                };
                values.push((row.stream, *value));
            }
            if next.is_none() {
                break;
            }
            r.after = next;
        }
        let mut expected = Vec::new();
        for stream in source.catalog().streams.iter().map(|s| s.id) {
            for value in 0..80 {
                expected.push((stream, value));
            }
        }
        assert_eq!(values, expected);
    }
    #[test]
    fn fixed_range_is_exclusive_and_plot_uses_existing_numeric_reduction() {
        let source = source(3000, 0, false);
        let mut r = request(&source, QueryMode::Plot);
        r.range = DataWindowTimeRange::new(ArrivalTime(101), ArrivalTime(2101)).unwrap();
        let QueryResult::Plot(series) = execute(&source, &r, || false).unwrap() else {
            panic!()
        };
        assert_eq!(series.len(), 2);
        for s in series {
            assert_eq!(s.overview.input_sample_count, 2000);
            assert!(s.overview.samples.len() <= 2048);
            assert!(
                s.overview
                    .samples
                    .iter()
                    .all(|p| r.range.contains(p.arrival_time))
            );
        }
        r.mode = QueryMode::Table;
        let QueryResult::Table { rows, .. } = execute(&source, &r, || false).unwrap() else {
            panic!()
        };
        assert!(rows.iter().all(|row| r.range.contains(row.time)));
        assert_eq!(rows[0].time, ArrivalTime(101));
    }
    #[test]
    fn budgets_invalid_fields_and_cancellation_fail_explicitly() {
        let source = source(2, 0, false);
        let mut r = request(&source, QueryMode::Table);
        assert!(execute_bounded(&source, &r, 1, || false).is_err());
        assert!(execute(&source, &r, || true).is_err());
        r.topics[0].fields = vec!["missing".into()];
        assert!(execute(&source, &r, || false).is_err());
        r.topics = vec![r.topics[1].clone(); 9];
        assert!(execute(&source, &r, || false).is_err());
        let huge = McapSource::from_bytes(fixture(1, 0, false, true).into()).unwrap();
        let QueryResult::Table { rows, .. } =
            execute(&huge, &request(&huge, QueryMode::Table), || false).unwrap()
        else {
            panic!()
        };
        assert_eq!(rows.len(), 2);
        for row in rows {
            assert!(row.payload_bytes > PAGE_BYTES);
            assert!(matches!(
                row.values[0],
                Ok(SelectedValue::Scalar(viewer_core::MessageScalar::Unsigned(
                    0
                )))
            ));
            assert!(row.message.err().unwrap().contains("4 MiB"));
        }
    }
    #[test]
    fn one_worker_rejects_obsolete_requests_and_source_results_and_live_is_unavailable() {
        let source = source(100, 0, false);
        let replacement = source_from_offset(700);
        let mut service = QueryService::default();
        service.bind_source(Some(&source));
        assert!(service.worker.is_none());
        service.submit(request(&source, QueryMode::Table));
        let mut changed = request(&source, QueryMode::Table);
        changed.range = DataWindowTimeRange::new(ArrivalTime(150), ArrivalTime(151)).unwrap();
        service.submit(changed);
        wait(&mut service);
        let QueryState::Completed(QueryResult::Table { rows, .. }) = &service.state else {
            panic!()
        };
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.time == ArrivalTime(150)));
        let handle = service
            .worker
            .as_ref()
            .unwrap()
            .handle
            .as_ref()
            .unwrap()
            .thread()
            .id();
        service.submit(request(&source, QueryMode::Plot));
        service.bind_source(Some(&replacement));
        assert!(matches!(service.state, QueryState::Idle));
        service.submit(request(&replacement, QueryMode::Table));
        wait(&mut service);
        assert_eq!(
            service
                .worker
                .as_ref()
                .unwrap()
                .handle
                .as_ref()
                .unwrap()
                .thread()
                .id(),
            handle
        );
        let QueryState::Completed(QueryResult::Table { rows, .. }) = &service.state else {
            panic!()
        };
        assert!(matches!(
            rows[0].values[0],
            Ok(SelectedValue::Scalar(viewer_core::MessageScalar::Unsigned(
                700
            )))
        ));
        service.cancel();
        assert!(matches!(service.state, QueryState::Idle));
        service.bind_source(None);
        assert!(matches!(service.state, QueryState::Unavailable));
        service.submit(request(&source, QueryMode::Table));
        assert!(matches!(service.state, QueryState::Unavailable));
    }
    fn source_from_offset(offset: u32) -> McapSource {
        source(2, offset, false)
    }
    #[test]
    fn analysis_never_changes_playback_selection_cursor_or_seek() {
        let path = std::env::temp_dir().join(format!("query-session-{}.mcap", std::process::id()));
        std::fs::write(&path, fixture(10, 0, false, false)).unwrap();
        let mut workspace = crate::workspace::NativeWorkspace::load_bundled_or_fallback(
            crate::workspace::WorkspaceLayout::Inspection,
        );
        let demand = workspace.data_requirements().playback;
        let mut session = crate::session::ViewerSession::open(
            &path,
            String::new(),
            &demand,
            workspace.bindings(),
        )
        .unwrap();
        workspace.configure_session(session.plan());
        let selected = session.plan().selected_stream_ids();
        let cursor = session.playback_view().unwrap().cursor;
        assert!(selected.is_empty());
        let source = session.recording_source().unwrap();
        execute(source, &request(source, QueryMode::Table), || false).unwrap();
        assert_eq!(session.plan().selected_stream_ids(), selected);
        assert_eq!(session.playback_view().unwrap().cursor, cursor);
        session
            .apply_playback_command(
                viewer_core::PlaybackCommand::Seek(ArrivalTime(105)),
                |target, messages| workspace.restore_messages(target, messages),
            )
            .unwrap();
        assert_eq!(session.playback_view().unwrap().cursor, ArrivalTime(105));
        assert_eq!(session.plan().selected_stream_ids(), selected);
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
#[test]
fn query_unselected_recording_topic_preserves_camera_routes_and_restored_scene() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap");
    let mut workspace = crate::workspace::NativeWorkspace::load_bundled_or_fallback(
        crate::workspace::WorkspaceLayout::Scene,
    );
    let mut requirements = workspace.data_requirements().playback;
    requirements.require_camera_topic("/camera/front/image/compressed");
    let mut session = crate::session::ViewerSession::open(
        &path,
        "/camera/front/image/compressed".into(),
        &requirements,
        workspace.bindings(),
    )
    .unwrap();
    workspace.configure_session(session.plan());
    let selected = session.plan().selected_stream_ids();
    let cursor = session.playback_view().unwrap().cursor;
    let source = session.recording_source().unwrap();
    let stream = source
        .catalog()
        .streams
        .iter()
        .find(|s| !selected.contains(&s.id))
        .unwrap()
        .id;
    let request = QueryRequest {
        topics: vec![QueryTopic {
            stream,
            fields: vec![],
        }],
        range: DataWindowTimeRange::new(
            source.time_range().start,
            source.time_range().end_exclusive,
        )
        .unwrap(),
        mode: QueryMode::Table,
        after: None,
    };
    let QueryResult::Table { rows, .. } = execute(source, &request, || false).unwrap() else {
        panic!()
    };
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|r| r.stream == stream));
    assert_eq!(session.plan().selected_stream_ids(), selected);
    assert_eq!(session.playback_view().unwrap().cursor, cursor);
    let end = session.playback_view().unwrap().end;
    session
        .apply_playback_command(
            viewer_core::PlaybackCommand::Seek(end),
            |target, messages| workspace.restore_messages(target, messages),
        )
        .unwrap();
    assert_eq!(session.plan().selected_stream_ids(), selected);
    assert!(workspace.cameras().state().latest_by_arrival().is_some());
    assert!(
        workspace
            .runtime()
            .point_cloud()
            .unwrap()
            .point_cloud()
            .latest()
            .is_some()
    );
}
