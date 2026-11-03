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
    ArrivalTime, DataWindowTimeRange, FieldDecodeBudget, FieldDecodeLimits, MessageScalar,
    RosMessageSchema, SelectedFieldExtractor, SelectedValue, StreamId,
};
use viewer_extension::panel::{
    InputStatus, PanelDefinition, PanelInputDefinition, PanelInputSnapshot, PanelUpdate, PanelValue,
};
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
                    for (id, definition) in &self.definitions {
                        let mut data = empty(self.generation, 0, view.cursor, view.start);
                        for input in &definition.inputs {
                            let mut snapshot = missing(InputStatus::Unavailable);
                            snapshot
                                .errors
                                .push(format!("start plugin query worker: {error}"));
                            data.inputs.insert(input.id.clone(), snapshot);
                        }
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
        // Resuming after an exact paused query must not round the requested time backwards:
        // its cached history may contain samples later than that rounded target.
        let target = if view.playing {
            target
                .max(self.last_request.map_or(target, |(last, _)| last))
                .min(view.cursor)
        } else {
            target
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
        inputs: BTreeMap::new(),
        events: vec![],
        settings: serde_json::Value::Null,
    }
}
struct Projection {
    stream: StreamId,
    fields: Vec<(String, SelectedFieldExtractor)>,
    history_key: Option<DataWindowTimeRange>,
    history_plan: Option<NumericOverviewPlan>,
    history_updated_at: Option<Instant>,
    history: BTreeMap<String, Vec<[f64; 2]>>,
    latest_time: Option<ArrivalTime>,
    latest_values: BTreeMap<String, PanelValue>,
}
struct QueryContext {
    generation: u64,
    latest: LatestMessageReader,
    projections: BTreeMap<(String, String), Result<Projection, String>>,
}
fn compile(source: &McapSource, definition: &PanelInputDefinition) -> Result<Projection, String> {
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
                .compile_field(path)
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
fn missing(status: InputStatus) -> PanelInputSnapshot {
    PanelInputSnapshot {
        status,
        log_time_ns: None,
        values: BTreeMap::new(),
        histories: BTreeMap::new(),
        errors: vec![],
    }
}
fn panel_value(value: SelectedValue) -> PanelValue {
    match value {
        SelectedValue::Array(values) => {
            PanelValue::Array(values.into_iter().map(panel_value).collect())
        }
        SelectedValue::Scalar(s) => match s {
            MessageScalar::Bool(v) => PanelValue::Bool(v),
            MessageScalar::Signed(v) => PanelValue::Signed(v.to_string()),
            MessageScalar::Unsigned(v) => PanelValue::Unsigned(v.to_string()),
            MessageScalar::Float(v) => PanelValue::Number(v),
            MessageScalar::Text(v) => PanelValue::Text(v),
        },
    }
}
fn query_input(
    job: &Job,
    context: &mut QueryContext,
    panel: &str,
    definition: &PanelInputDefinition,
    cancelled: &impl Fn() -> bool,
) -> PanelInputSnapshot {
    let range = job.source.time_range();
    let start = range.start;
    let projection = context
        .projections
        .entry((panel.into(), definition.id.clone()))
        .or_insert_with(|| compile(&job.source, definition));
    let p = match projection {
        Ok(p) => p,
        Err(error) => {
            let mut input = missing(if error == "missing topic" {
                InputStatus::MissingTopic
            } else {
                InputStatus::DecodeError
            });
            input.errors.push(error.chars().take(512).collect());
            return input;
        }
    };
    let outcome = (|| -> anyhow::Result<PanelInputSnapshot> {
        let Some((_, raw)) = context
            .latest
            .read_at_cancellable(p.stream, job.target, cancelled)?
        else {
            return Ok(missing(InputStatus::NoPredecessor));
        };
        anyhow::ensure!(
            raw.payload.len() <= 2 * 1024 * 1024,
            "selected message exceeds 2 MiB panel limit"
        );
        if p.latest_time != Some(raw.arrival_time) {
            let mut budget = FieldDecodeBudget::new(FieldDecodeLimits {
                max_array_len: 128,
                max_values: 512,
                max_text_bytes: 2048,
                max_total_text_bytes: 8192,
            });
            let mut values = BTreeMap::new();
            for (path, extractor) in &p.fields {
                values.insert(
                    path.clone(),
                    panel_value(extractor.extract(&raw.payload, &mut budget)?),
                );
            }
            p.latest_time = Some(raw.arrival_time);
            p.latest_values = values;
        }
        let mut input = missing(InputStatus::Ready);
        input.log_time_ns = Some(raw.arrival_time.0.to_string());
        input.values = p.latest_values.clone();
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
            let window = DataWindowTimeRange::new(ArrivalTime(begin), ArrivalTime(end))?;
            if p.history_key != Some(window)
                && (!job.playing || due(p.history_updated_at, Duration::from_secs(1)))
            {
                p.history.clear();
                p.history_key = None;
                let batch = job.source.query_numeric_overview_plan_cancellable(
                    p.history_plan.as_ref().expect("plots have compiled plans"),
                    window,
                    cancelled,
                )?;
                for (path, series) in definition.plots.iter().zip(batch.series) {
                    if series.decode_errors > 0 {
                        input.errors.push(format!(
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
                                    (s.arrival_time.0 as i128 - start.0 as i128) as f64 / 1e9,
                                    s.value,
                                ]
                            })
                            .collect(),
                    );
                }
                p.history_key = Some(window);
                p.history_updated_at = Some(Instant::now());
            }
            input.histories = p.history.clone();
        }
        input.validate_size()?;
        Ok(input)
    })();
    match outcome {
        Ok(input) => input,
        Err(error) => {
            p.latest_time = None;
            p.latest_values.clear();
            p.history.clear();
            p.history_key = None;
            p.history_updated_at = None;
            let mut input = missing(InputStatus::DecodeError);
            input
                .errors
                .push(error.to_string().chars().take(512).collect());
            input
        }
    }
}
fn query(
    job: &Job,
    context: &mut QueryContext,
    cancelled: impl Fn() -> bool,
) -> BTreeMap<String, PanelUpdate> {
    let mut results = BTreeMap::new();
    for (id, definition) in &job.definitions {
        if cancelled() {
            break;
        }
        let mut update = empty(
            job.generation,
            job.revision,
            job.target,
            job.source.time_range().start,
        );
        for input in &definition.inputs {
            if cancelled() {
                break;
            }
            update.inputs.insert(
                input.id.clone(),
                query_input(job, context, id, input, &cancelled),
            );
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
        let source =
            McapSource::open_file(&root().join("tests/fixtures/camera-jpeg/camera_7_5s.mcap"))
                .unwrap();
        let mut queries = PluginQueries::default();
        queries.poll(
            Some(&source),
            Some(view(&source, source.time_range().start)),
        );
        assert!(queries.worker.is_none());
        assert!(queries.data.is_empty());
        assert!(queries.last_request.is_none());
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
        assert_eq!(first.inputs["status"].status, InputStatus::NoPredecessor);
        queries.invalidate();
        let last = wait(&mut queries, &source, end);
        assert_eq!(last.inputs["status"].status, InputStatus::Ready);
        assert_eq!(
            last.inputs["status"].values["state"],
            PanelValue::Text("navigating".into())
        );
        assert!(
            matches!(&last.inputs["status"].values["sequence"],PanelValue::Unsigned(v) if v.parse::<u64>().unwrap()>9_007_199_254_740_992)
        );
        assert!(!last.inputs["status"].histories["battery"].is_empty());
        assert!(last.inputs["status"].histories.values().all(|points| {
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
            wait(&mut throttled, &source, end).inputs["status"].status,
            InputStatus::Ready
        );
        drop(throttled);
        let mut resumed = PluginQueries::default();
        resumed.configure(vec![("panel".into(), definition.clone())]);
        let paused_cursor = ArrivalTime(range.start.0 + 1_550_000_000);
        let paused = wait(&mut resumed, &source, paused_cursor);
        resumed.last_started = None;
        let mut playing = view(&source, paused_cursor);
        playing.playing = true;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            resumed.poll(Some(&source), Some(playing));
            if resumed.data["panel"].revision > paused.revision {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let restored = &resumed.data["panel"];
        assert_eq!(restored.cursor_ns, paused_cursor.0.to_string());
        assert!(
            restored.inputs["status"]
                .histories
                .values()
                .flatten()
                .all(|point| point[0] <= (paused_cursor.0 - range.start.0) as f64 / 1e9)
        );
        drop(resumed);
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
            .get_mut(&("panel".into(), "status".into()))
            .unwrap()
            .as_mut()
            .unwrap()
            .history_updated_at = Some(Instant::now());
        job.target = end;
        let fast = query(&job, &mut context, || false);
        assert_eq!(
            short["panel"].inputs["status"].histories,
            fast["panel"].inputs["status"].histories
        );
        job.playing = false;
        let paused = query(&job, &mut context, || false);
        assert!(
            paused["panel"].inputs["status"].histories["battery"]
                .last()
                .unwrap()[0]
                > short["panel"].inputs["status"].histories["battery"]
                    .last()
                    .unwrap()[0]
        );
        let generation = queries.generation;
        queries.invalidate();
        assert!(queries.data.is_empty());
        assert_ne!(queries.generation, generation);
        let earlier = ArrivalTime(range.start.0 + (end.0 - range.start.0) / 2);
        let restored = wait(&mut queries, &source, earlier);
        assert_eq!(restored.inputs["status"].status, InputStatus::Ready);
        assert_ne!(
            restored.inputs["status"].values["battery"],
            last.inputs["status"].values["battery"]
        );
        assert!(
            restored.inputs["status"]
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
            changed.inputs["status"].values["state"],
            PanelValue::Text("charging".into())
        );
        assert_ne!(
            changed.inputs["status"].values["battery"],
            last.inputs["status"].values["battery"]
        );
        replacement.invalidate();
        let malformed = super::tests::source("malformed");
        let bad = wait(&mut replacement, &malformed, end);
        assert_eq!(bad.inputs["status"].status, InputStatus::DecodeError);
        assert!(bad.inputs["status"].values.is_empty());
        assert!(bad.inputs["status"].histories.is_empty());
        replacement.configure(vec![(
            "panel".into(),
            PanelDefinition {
                inputs: vec![PanelInputDefinition {
                    topic: "/missing".into(),
                    ..definition.inputs[0].clone()
                }],
                ..definition
            },
        )]);
        let missing = wait(&mut replacement, &source, end);
        assert_eq!(missing.inputs["status"].status, InputStatus::MissingTopic);
        assert!(missing.inputs["status"].values.is_empty());
        assert!(missing.inputs["status"].histories.is_empty());
    }
    #[test]
    #[ignore = "build robot-monitor-panel and generate target/plugin-multi-input fixtures first"]
    fn actual_robot_monitor_arrays_multiple_inputs_and_source_isolation() {
        let dir = root().join("target/plugin-multi-input");
        let mut wasm =
            WasmPanel::load(&root().join("examples/robot-monitor-panel/panel.json")).unwrap();
        let definition = wasm.definition().clone();
        assert_eq!(definition.inputs.len(), 2);
        let source = McapSource::open_file(&dir.join("recording.mcap")).unwrap();
        let mut queries = PluginQueries::default();
        queries.configure(vec![("panel".into(), definition.clone())]);
        let frames = viewer_mcap::FileMessageStream::open(&dir.join("recording.mcap"))
            .unwrap()
            .map(Result::unwrap)
            .filter(|m| m.channel.topic == "/perception/tracks")
            .map(|m| {
                (
                    ArrivalTime(m.log_time as i64),
                    u32::from_le_bytes(m.data[4..8].try_into().unwrap()),
                )
            })
            .collect::<Vec<_>>();
        let end = ArrivalTime(source.time_range().end_exclusive.0 - 1);
        let before = wait(&mut queries, &source, ArrivalTime(frames[0].0.0 - 1));
        assert_eq!(before.inputs["status"].status, InputStatus::Ready);
        assert_eq!(before.inputs["tracks"].status, InputStatus::NoPredecessor);
        for index in [frames.len() - 1, 0, 5, 4, frames.len() - 1] {
            queries.invalidate();
            assert!(queries.data.is_empty());
            wasm.reset().unwrap();
            let (target, frame) = frames[index];
            let data = wait(&mut queries, &source, target);
            for id in ["status", "tracks"] {
                assert_eq!(data.inputs[id].status, InputStatus::Ready);
                assert!(
                    data.inputs[id]
                        .log_time_ns
                        .as_ref()
                        .unwrap()
                        .parse::<i64>()
                        .unwrap()
                        <= target.0
                );
            }
            let tracks = &data.inputs["tracks"];
            for path in [
                "objects[].id",
                "objects[].label",
                "objects[].confidence",
                "objects[].distance",
            ] {
                let PanelValue::Array(values) = &tracks.values[path] else {
                    panic!("expected selected array")
                };
                assert_eq!(values.len(), (frame % 4) as usize);
            }
            let PanelValue::Array(timings) = &tracks.values["timings_ms[]"] else {
                panic!()
            };
            assert_eq!(timings.len(), (frame % 3 + 1) as usize);
            let output = wasm.update(&data).unwrap();
            let text = serde_json::to_string(&output).unwrap();
            if frame % 4 == 0 {
                assert!(text.contains("No objects in latest perception frame"));
                assert!(!text.contains("9007199254740993"));
            } else {
                assert!(text.contains("9007199254740993"));
                assert!(text.contains("person"));
            }
        }
        queries.invalidate();
        let data = wait(&mut queries, &source, end);
        assert_ne!(
            data.inputs["status"].log_time_ns,
            data.inputs["tracks"].log_time_ns
        );
        assert!(!data.inputs["status"].histories["battery"].is_empty());
        for mode in [
            "malformed",
            "oversized",
            "missing-tracks",
            "missing-status",
            "alternate",
        ] {
            queries.invalidate();
            queries.poll(Some(&source), Some(view(&source, end)));
            let replacement = McapSource::open_file(&dir.join(format!("{mode}.mcap"))).unwrap();
            let mut next = PluginQueries::default();
            next.configure(vec![("panel".into(), definition.clone())]);
            next.take_worker_from(&mut queries);
            queries = next;
            wasm.reset().unwrap();
            let data = wait(&mut queries, &replacement, end);
            let text = serde_json::to_string(&wasm.update(&data).unwrap()).unwrap();
            match mode {
                "malformed" | "oversized" => {
                    assert_eq!(data.inputs["tracks"].status, InputStatus::DecodeError);
                    assert!(data.inputs["tracks"].values.is_empty());
                    assert!(data.inputs["tracks"].log_time_ns.is_none());
                    assert_eq!(data.inputs["status"].status, InputStatus::Ready);
                    assert!(!text.contains("9007199254740993"));
                }
                "missing-tracks" => {
                    assert_eq!(data.inputs["tracks"].status, InputStatus::MissingTopic);
                    assert_eq!(data.inputs["status"].status, InputStatus::Ready);
                }
                "missing-status" => {
                    assert_eq!(data.inputs["status"].status, InputStatus::MissingTopic);
                    assert_eq!(data.inputs["tracks"].status, InputStatus::Ready);
                    assert!(data.inputs["status"].histories.is_empty());
                    assert!(text.contains("person"));
                }
                _ => {
                    assert_eq!(data.inputs["tracks"].status, InputStatus::Ready);
                    assert!(text.contains("alternate"));
                    assert!(!text.contains("forklift"));
                }
            }
        }
    }
}
