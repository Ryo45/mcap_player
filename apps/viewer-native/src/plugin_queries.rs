//! Bounded PluginPanel queries over existing indexed readers; no playback stream routes.
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use viewer_core::{
    ArrivalTime, DataWindowTimeRange, MessageScalar, RosMessageSchema, ScalarFieldExtractor,
    StreamId,
};
use viewer_extension::panel::{InputStatus, PanelDefinition, PanelScalar, PanelUpdate};
use viewer_mcap::{LatestMessageReader, McapSource, NumericOverviewPlan, NumericSeriesRequest};
type Definitions = Vec<(String, PanelDefinition)>;
struct Job {
    generation: u64,
    revision: u64,
    target: ArrivalTime,
    playing: bool,
    source: McapSource,
    definitions: Definitions,
}
struct Completion {
    generation: u64,
    target: ArrivalTime,
    data: BTreeMap<String, PanelUpdate>,
}
#[derive(Default)]
struct Mailbox {
    pending: Option<Job>,
    result: Option<Completion>,
    shutdown: bool,
}
struct Worker {
    mailbox: Arc<(Mutex<Mailbox>, Condvar)>,
    generation: Arc<AtomicU64>,
    handle: Option<JoinHandle<()>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        {
            let mut state = self.mailbox.0.lock().unwrap();
            state.shutdown = true;
            state.pending = None;
            self.generation.fetch_add(1, Ordering::AcqRel);
            self.mailbox.1.notify_one();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
#[derive(Default)]
pub(crate) struct PluginQueries {
    definitions: Definitions,
    worker: Option<Worker>,
    pub generation: u64,
    pub data: BTreeMap<String, PanelUpdate>,
    last_request: Option<(ArrivalTime, bool)>,
    revision: u64,
    busy: bool,
    last_started: Option<Instant>,
}
impl PluginQueries {
    pub fn configure(&mut self, definitions: Definitions) {
        self.definitions = definitions;
        self.invalidate();
    }
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.data.clear();
        self.last_request = None;
        self.last_started = None;
        if let Some(w) = &self.worker {
            w.generation.store(self.generation, Ordering::Release);
            let mut m = w.mailbox.0.lock().unwrap();
            m.pending = None;
            m.result = None;
        }
        self.busy = false;
    }
    pub fn take_worker_from(&mut self, previous: &mut Self) {
        self.generation = previous.generation.wrapping_add(1);
        self.worker = previous.worker.take();
        self.invalidate();
    }
    pub fn poll(&mut self, source: Option<&McapSource>, cursor: Option<viewer_core::PlaybackView>) {
        if self.definitions.is_empty() {
            return;
        }
        let Some((source, view)) = source.zip(cursor) else {
            self.data.clear();
            return;
        };
        if self.worker.is_none() {
            let mailbox = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
            let generation = Arc::new(AtomicU64::new(self.generation));
            let shared = mailbox.clone();
            let epoch = generation.clone();
            match std::thread::Builder::new()
                .name("plugin-panel-query".into())
                .spawn(move || run(shared, epoch))
            {
                Ok(handle) => {
                    self.worker = Some(Worker {
                        mailbox,
                        generation,
                        handle: Some(handle),
                    })
                }
                Err(error) => {
                    for (id, _) in &self.definitions {
                        let mut data = empty(self.generation, 0, view.cursor, view.start);
                        data.errors
                            .push(format!("start plugin query worker: {error}"));
                        self.data.insert(id.clone(), data);
                    }
                    return;
                }
            }
        }
        let worker = self.worker.as_ref().unwrap();
        {
            let mut mailbox = worker.mailbox.0.lock().unwrap();
            if let Some(result) = mailbox.result.take()
                && result.generation == self.generation
                && result.target <= view.cursor
            {
                self.busy = false;
                self.data = result.data;
            }
        }
        let target = if view.playing {
            ArrivalTime(view.start.0.saturating_add(
                view.cursor.0.saturating_sub(view.start.0) / 100_000_000 * 100_000_000,
            ))
        } else {
            view.cursor
        };
        if self.busy
            || self.last_request == Some((target, view.playing))
            || (view.playing && !due(self.last_started, Duration::from_millis(100)))
        {
            return;
        }
        self.last_started = Some(Instant::now());
        self.revision = self.revision.wrapping_add(1);
        self.last_request = Some((target, view.playing));
        self.busy = true;
        let job = Job {
            generation: self.generation,
            revision: self.revision,
            target,
            playing: view.playing,
            source: source.fork_reader(),
            definitions: self.definitions.clone(),
        };
        worker.mailbox.0.lock().unwrap().pending = Some(job);
        worker.mailbox.1.notify_one();
    }
}
fn due(previous: Option<Instant>, interval: Duration) -> bool {
    previous.is_none_or(|at| at.elapsed() >= interval)
}
fn empty(generation: u64, revision: u64, target: ArrivalTime, start: ArrivalTime) -> PanelUpdate {
    PanelUpdate {
        generation,
        revision,
        cursor_ns: target.0.to_string(),
        source_start_ns: start.0.to_string(),
        status: InputStatus::Unavailable,
        values: BTreeMap::new(),
        histories: BTreeMap::new(),
        errors: vec![],
        events: vec![],
        settings: serde_json::Value::Null,
    }
}
struct Projection {
    stream: StreamId,
    fields: Vec<(String, ScalarFieldExtractor)>,
    history_key: Option<DataWindowTimeRange>,
    history_plan: Option<NumericOverviewPlan>,
    history_updated_at: Option<Instant>,
    history: BTreeMap<String, Vec<[f64; 2]>>,
    latest_time: Option<ArrivalTime>,
    latest_values: BTreeMap<String, PanelScalar>,
}
struct QueryContext {
    generation: u64,
    latest: LatestMessageReader,
    projections: BTreeMap<String, Result<Projection, String>>,
}
fn compile(source: &McapSource, definition: &PanelDefinition) -> Result<Projection, String> {
    let streams = source
        .catalog()
        .streams
        .iter()
        .filter(|s| s.topic == definition.topic)
        .collect::<Vec<_>>();
    if streams.len() != 1 {
        return Err(if streams.is_empty() {
            "missing topic".into()
        } else {
            "PoC requires one stream/schema per topic".into()
        });
    }
    let stream = streams[0];
    let schema = RosMessageSchema::for_stream(stream).map_err(|e| e.to_string())?;
    let fields = definition
        .fields
        .iter()
        .map(|path| {
            schema
                .compile_scalar(path)
                .map(|plan| (path.clone(), plan))
                .map_err(|e| format!("{path}: {e}"))
        })
        .collect::<Result<_, _>>()?;
    let requests = definition
        .plots
        .iter()
        .map(|path| NumericSeriesRequest {
            stream_id: stream.id,
            field_path: path.clone(),
            range: DataWindowTimeRange::new(
                source.time_range().start,
                source.time_range().end_exclusive,
            )
            .expect("nonempty source"),
            max_points: definition.max_points,
        })
        .collect::<Vec<_>>();
    let history_plan = if requests.is_empty() {
        None
    } else {
        Some(
            source
                .compile_numeric_overview(&requests)
                .map_err(|e| e.to_string())?,
        )
    };
    Ok(Projection {
        stream: stream.id,
        fields,
        history_key: None,
        history_plan,
        history_updated_at: None,
        history: BTreeMap::new(),
        latest_time: None,
        latest_values: BTreeMap::new(),
    })
}
fn query(
    job: &Job,
    context: &mut QueryContext,
    cancelled: impl Fn() -> bool,
) -> BTreeMap<String, PanelUpdate> {
    let range = job.source.time_range();
    let start = range.start;
    let mut results = BTreeMap::new();
    for (id, definition) in &job.definitions {
        let mut update = empty(job.generation, job.revision, job.target, start);
        let projection = context
            .projections
            .entry(id.clone())
            .or_insert_with(|| compile(&job.source, definition));
        match projection {
            Err(error) => {
                update.status = if error == "missing topic" {
                    InputStatus::MissingTopic
                } else {
                    InputStatus::DecodeError
                };
                update.errors.push(error.clone());
            }
            Ok(p) => {
                let outcome = (|| -> anyhow::Result<()> {
                    let Some((_, raw)) = context
                        .latest
                        .read_at_cancellable(p.stream, job.target, &cancelled)?
                    else {
                        update.status = InputStatus::NoPredecessor;
                        return Ok(());
                    };
                    anyhow::ensure!(
                        raw.payload.len() <= 2 * 1024 * 1024,
                        "selected message exceeds 2 MiB panel limit"
                    );
                    if p.latest_time != Some(raw.arrival_time) {
                        let mut values = BTreeMap::new();
                        for (path, extractor) in &p.fields {
                            let value = match extractor.extract(&raw.payload, 2048)? {
                                MessageScalar::Bool(v) => PanelScalar::Bool(v),
                                MessageScalar::Signed(v) => PanelScalar::Signed(v.to_string()),
                                MessageScalar::Unsigned(v) => PanelScalar::Unsigned(v.to_string()),
                                MessageScalar::Float(v) => PanelScalar::Number(v),
                                MessageScalar::Text(v) => PanelScalar::Text(v),
                            };
                            values.insert(path.clone(), value);
                        }
                        p.latest_time = Some(raw.arrival_time);
                        p.latest_values = values;
                    }
                    update.values = p.latest_values.clone();
                    update.status = InputStatus::Ready;
                    let end = if job.playing {
                        start.0.saturating_add(
                            job.target.0.saturating_sub(start.0) / 1_000_000_000 * 1_000_000_000,
                        )
                    } else {
                        job.target.0.saturating_add(1).min(range.end_exclusive.0)
                    };
                    let begin = end
                        .saturating_sub(i64::from(definition.history_seconds) * 1_000_000_000)
                        .max(start.0);
                    if !definition.plots.is_empty() && begin < end {
                        let window =
                            DataWindowTimeRange::new(ArrivalTime(begin), ArrivalTime(end))?;
                        if p.history_key != Some(window)
                            && (!job.playing || due(p.history_updated_at, Duration::from_secs(1)))
                        {
                            p.history.clear();
                            p.history_key = None;
                            let batch = job.source.query_numeric_overview_plan_cancellable(
                                p.history_plan
                                    .as_ref()
                                    .expect("nonempty plots have compiled plans"),
                                window,
                                &cancelled,
                            )?;
                            for (path, series) in definition.plots.iter().zip(batch.series) {
                                if series.decode_errors > 0 {
                                    update.errors.push(format!(
                                        "{path}: {} historical decode errors",
                                        series.decode_errors
                                    ));
                                }
                                p.history.insert(
                                    path.clone(),
                                    series
                                        .samples
                                        .iter()
                                        .filter(|s| s.arrival_time <= job.target)
                                        .map(|s| {
                                            [
                                                (s.arrival_time.0 as i128 - start.0 as i128) as f64
                                                    / 1e9,
                                                s.value,
                                            ]
                                        })
                                        .collect(),
                                );
                            }
                            p.history_key = Some(window);
                            p.history_updated_at = Some(Instant::now());
                        }
                        update.histories = p.history.clone();
                    }
                    Ok(())
                })();
                if let Err(error) = outcome {
                    p.latest_time = None;
                    p.latest_values.clear();
                    p.history.clear();
                    p.history_key = None;
                    p.history_updated_at = None;
                    update.status = InputStatus::DecodeError;
                    update.values.clear();
                    update.histories.clear();
                    update.errors.push(error.to_string());
                }
            }
        }
        results.insert(id.clone(), update);
    }
    results
}
fn run(shared: Arc<(Mutex<Mailbox>, Condvar)>, generation: Arc<AtomicU64>) {
    let mut context: Option<QueryContext> = None;
    loop {
        let job = {
            let mut state = shared.0.lock().unwrap();
            while state.pending.is_none() && !state.shutdown {
                state = shared.1.wait(state).unwrap();
            }
            if state.shutdown {
                return;
            }
            state.pending.take().unwrap()
        };
        if context
            .as_ref()
            .is_none_or(|c| c.generation != job.generation)
        {
            context = Some(QueryContext {
                generation: job.generation,
                latest: LatestMessageReader::new(job.source.fork_reader()),
                projections: BTreeMap::new(),
            });
        }
        let data = query(&job, context.as_mut().unwrap(), || {
            generation.load(Ordering::Acquire) != job.generation
        });
        shared.0.lock().unwrap().result = Some(Completion {
            generation: job.generation,
            target: job.target,
            data,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{PlaybackSpeed, PlaybackView};
    use viewer_extension::WasmPanel;
    fn root() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../")
    }
    fn panel() -> WasmPanel {
        WasmPanel::load(&root().join("examples/robot-status-panel/panel.json")).unwrap()
    }
    fn source(name: &str) -> McapSource {
        McapSource::open_file(&root().join(format!("target/plugin-panel/{name}.mcap"))).unwrap()
    }
    fn view(source: &McapSource, cursor: ArrivalTime) -> PlaybackView {
        let r = source.time_range();
        PlaybackView {
            start: r.start,
            end: ArrivalTime(r.end_exclusive.0 - 1),
            cursor,
            playing: false,
            speed: PlaybackSpeed::Normal,
        }
    }
    fn wait(queries: &mut PluginQueries, source: &McapSource, cursor: ArrivalTime) -> PanelUpdate {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            queries.poll(Some(source), Some(view(source, cursor)));
            if let Some(data) = queries.data.get("panel") {
                return data.clone();
            }
            assert!(std::time::Instant::now() < deadline, "worker timed out");
            std::thread::yield_now();
        }
    }
    #[test]
    fn query_cadence_uses_wall_time_independently_of_playback_speed() {
        assert!(due(None, Duration::from_millis(100)));
        let now = Instant::now();
        assert!(!due(Some(now), Duration::from_millis(100)));
        assert!(due(
            Some(now - Duration::from_millis(101)),
            Duration::from_millis(100)
        ));
        assert!(!due(
            Some(now - Duration::from_millis(500)),
            Duration::from_secs(1)
        ));
        assert!(due(
            Some(now - Duration::from_millis(1001)),
            Duration::from_secs(1)
        ));
    }
    #[test]
    fn disabled_queries_do_not_start_io_or_threads() {
        let mut queries = PluginQueries::default();
        queries.poll(None, None);
        assert!(queries.worker.is_none());
        assert!(queries.data.is_empty());
    }
    #[test]
    #[ignore = "build RobotStatus guest and generate target/plugin-panel fixtures first"]
    fn actual_robot_status_query_seek_source_and_failures() {
        let mut wasm = panel();
        let definition = wasm.definition().clone();
        let source = source("recording");
        let range = source.time_range();
        let end = ArrivalTime(range.end_exclusive.0 - 1);
        let mut queries = PluginQueries::default();
        queries.configure(vec![("panel".into(), definition.clone())]);
        let first = wait(&mut queries, &source, range.start);
        assert_eq!(first.status, InputStatus::NoPredecessor);
        queries.invalidate();
        let last = wait(&mut queries, &source, end);
        assert_eq!(last.status, InputStatus::Ready);
        assert_eq!(last.values["state"], PanelScalar::Text("navigating".into()));
        assert!(
            matches!(&last.values["sequence"],PanelScalar::Unsigned(v) if v.parse::<u64>().unwrap()>9_007_199_254_740_992)
        );
        assert!(!last.histories["battery"].is_empty());
        assert!(last.histories.values().all(|points| {
            points.len() <= 128
                && points
                    .iter()
                    .all(|p| p[0] <= ((end.0 - range.start.0) as f64) / 1e9)
        }));
        let rendered = serde_json::to_string(&wasm.update(&last).unwrap()).unwrap();
        assert!(rendered.contains("navigating"));
        assert!(rendered.contains("history"));
        // Faster playback must not turn the latest query into a faster wall-time poll.
        let mut throttled = PluginQueries::default();
        throttled.configure(vec![("panel".into(), definition.clone())]);
        throttled.last_started = Some(Instant::now());
        let reads = source.read_diagnostics().file_read_calls;
        let mut fast = view(&source, end);
        fast.playing = true;
        fast.speed = PlaybackSpeed::Double;
        throttled.poll(Some(&source), Some(fast));
        assert!(throttled.data.is_empty());
        assert_eq!(source.read_diagnostics().file_read_calls, reads);
        // Pausing must bypass that timer and restore exact cursor results immediately.
        assert_eq!(
            wait(&mut throttled, &source, end).status,
            InputStatus::Ready
        );
        drop(throttled);
        let mut job = Job {
            generation: 1,
            revision: 1,
            target: ArrivalTime(range.start.0 + 1_500_000_000),
            playing: true,
            source: source.fork_reader(),
            definitions: vec![("panel".into(), definition.clone())],
        };
        let mut context = QueryContext {
            generation: 1,
            latest: LatestMessageReader::new(source.fork_reader()),
            projections: BTreeMap::new(),
        };
        let short = query(&job, &mut context, || false);
        context
            .projections
            .get_mut("panel")
            .unwrap()
            .as_mut()
            .unwrap()
            .history_updated_at = Some(Instant::now());
        job.target = end;
        let fast = query(&job, &mut context, || false);
        assert_eq!(short["panel"].histories, fast["panel"].histories);
        job.playing = false;
        let paused = query(&job, &mut context, || false);
        assert!(
            paused["panel"].histories["battery"].last().unwrap()[0]
                > short["panel"].histories["battery"].last().unwrap()[0]
        );
        let generation = queries.generation;
        queries.invalidate();
        assert!(queries.data.is_empty());
        assert_ne!(queries.generation, generation);
        let earlier = ArrivalTime(range.start.0 + (end.0 - range.start.0) / 2);
        let restored = wait(&mut queries, &source, earlier);
        assert_eq!(restored.status, InputStatus::Ready);
        assert_ne!(restored.values["battery"], last.values["battery"]);
        assert!(
            restored
                .histories
                .values()
                .flatten()
                .all(|p| p[0] <= (earlier.0 - range.start.0) as f64 / 1e9)
        );
        // Invalidate an active old-source request, then reuse the worker for a new session.
        queries.invalidate();
        queries.poll(Some(&source), Some(view(&source, end)));
        let alternate = super::tests::source("alternate");
        let mut replacement = PluginQueries::default();
        replacement.configure(vec![("panel".into(), definition.clone())]);
        replacement.take_worker_from(&mut queries);
        let changed = wait(&mut replacement, &alternate, end);
        assert_eq!(
            changed.values["state"],
            PanelScalar::Text("charging".into())
        );
        assert_ne!(changed.values["battery"], last.values["battery"]);
        replacement.invalidate();
        let malformed = super::tests::source("malformed");
        let bad = wait(&mut replacement, &malformed, end);
        assert_eq!(bad.status, InputStatus::DecodeError);
        assert!(bad.values.is_empty());
        assert!(bad.histories.is_empty());
        replacement.configure(vec![(
            "panel".into(),
            PanelDefinition {
                topic: "/missing".into(),
                ..definition
            },
        )]);
        let missing = wait(&mut replacement, &source, end);
        assert_eq!(missing.status, InputStatus::MissingTopic);
        assert!(missing.values.is_empty());
        assert!(missing.histories.is_empty());
    }
}
