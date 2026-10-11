//! Concrete recent-window demand; one worker, one pending request and one result mailbox.
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use viewer_core::{
    ArrivalTime, McapSource, TopicHealthRequirement, TopicHealthSnapshot, WindowQueryError,
};

#[derive(Debug)]
pub(crate) enum WindowLoadError {
    Query(WindowQueryError),
    Start(std::io::Error),
    WorkerStopped,
}
impl std::fmt::Display for WindowLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Query(error) => error.fmt(f),
            Self::Start(error) => write!(f, "start window query worker: {error}"),
            Self::WorkerStopped => write!(f, "window query worker stopped"),
        }
    }
}
impl std::error::Error for WindowLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Query(error) => Some(error),
            Self::Start(error) => Some(error),
            Self::WorkerStopped => None,
        }
    }
}

pub(crate) struct TopicHealthQueryState {
    pub(crate) requirement: TopicHealthRequirement,
    pub(crate) cursor: ArrivalTime,
    pub(crate) loading: bool,
    pub(crate) snapshot: Option<Arc<Vec<TopicHealthSnapshot>>>,
    pub(crate) error: Option<Arc<WindowLoadError>>,
}

pub(crate) struct DiagnosticsQueryState {
    pub(crate) requirement: viewer_core::DiagnosticsRequirement,
    pub(crate) cursor: ArrivalTime,
    pub(crate) loading: bool,
    pub(crate) snapshot: Option<Arc<viewer_core::DiagnosticsSnapshot>>,
    pub(crate) error: Option<Arc<WindowLoadError>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DemandKey {
    cursor: ArrivalTime,
    health: BTreeSet<TopicHealthRequirement>,
    diagnostics: BTreeSet<viewer_core::DiagnosticsRequirement>,
}
struct Demand {
    key: DemandKey,
    generation: u64,
    source: Option<McapSource>,
}
struct QueryResults {
    key: DemandKey,
    generation: u64,
    health: Vec<TopicHealthQueryState>,
    diagnostics: Vec<DiagnosticsQueryState>,
}
#[derive(Default)]
struct Mailbox {
    pending: Option<Demand>,
    result: Option<QueryResults>,
    shutdown: bool,
}
struct Worker {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
    generation: Arc<AtomicU64>,
    handle: JoinHandle<()>,
}

#[derive(Default)]
pub(crate) struct WindowQueryLoader {
    health_requirements: BTreeSet<TopicHealthRequirement>,
    diagnostics_requirements: BTreeSet<viewer_core::DiagnosticsRequirement>,
    desired: Option<DemandKey>,
    health: Vec<TopicHealthQueryState>,
    diagnostics: Vec<DiagnosticsQueryState>,
    worker: Option<Worker>,
    reset_source: bool,
    queued_at: Option<Instant>,
}
impl WindowQueryLoader {
    pub(crate) fn invalidate(&mut self) {
        self.desired = None;
        self.queued_at = None;
        self.health.clear();
        self.diagnostics.clear();
        self.reset_source = true;
        if let Some(worker) = &self.worker {
            worker.generation.fetch_add(1, Ordering::AcqRel);
            let mut mailbox = worker.shared.0.lock().expect("window query mailbox");
            mailbox.pending = None;
            mailbox.result = None;
        }
    }
    pub(crate) fn configure_health(&mut self, requirements: &BTreeSet<TopicHealthRequirement>) {
        self.health_requirements = requirements.clone();
    }
    pub(crate) fn configure_diagnostics(
        &mut self,
        requirements: &BTreeSet<viewer_core::DiagnosticsRequirement>,
    ) {
        self.diagnostics_requirements = requirements.clone();
    }
    pub(crate) fn diagnostics(&self) -> &[DiagnosticsQueryState] {
        &self.diagnostics
    }
    pub(crate) fn health(&self) -> &[TopicHealthQueryState] {
        &self.health
    }
    pub(crate) fn take_worker_from(&mut self, previous: &mut Self) {
        self.worker = previous.worker.take();
        self.desired = None;
        self.health.clear();
        self.diagnostics.clear();
        self.reset_source = true;
        self.queued_at = None;
        if let Some(worker) = &self.worker {
            worker.generation.fetch_add(1, Ordering::AcqRel);
            let mut mailbox = worker.shared.0.lock().expect("window query mailbox");
            mailbox.pending = None;
            mailbox.result = None;
        }
    }
    pub(crate) fn poll(&mut self, source: Option<&McapSource>, cursor: Option<ArrivalTime>) {
        let (Some(source), Some(cursor)) = (source, cursor) else {
            self.health.clear();
            self.diagnostics.clear();
            if self.desired.take().is_some()
                && let Some(worker) = &self.worker
            {
                worker.generation.fetch_add(1, Ordering::AcqRel);
                let mut mailbox = worker.shared.0.lock().expect("window query mailbox");
                mailbox.pending = None;
                mailbox.result = None;
            }
            return;
        };
        let key = DemandKey {
            cursor,
            health: self.health_requirements.clone(),
            diagnostics: self.diagnostics_requirements.clone(),
        };
        let changed = self.desired.as_ref() != Some(&key);
        let urgent = self.reset_source
            || self.desired.as_ref().is_none_or(|old| {
                cursor < old.cursor
                    || old.health != key.health
                    || old.diagnostics != key.diagnostics
            });
        let due = self
            .queued_at
            .is_none_or(|queued| queued.elapsed() >= Duration::from_millis(250));
        if changed {
            for state in &mut self.health {
                state.loading = true;
            }
            for state in &mut self.diagnostics {
                state.loading = true;
            }
        }
        if changed && (urgent || due) {
            let backwards = self.desired.as_ref().is_some_and(|old| cursor < old.cursor);
            let changed_selection = self
                .desired
                .as_ref()
                .is_none_or(|old| old.health != key.health || old.diagnostics != key.diagnostics);
            if backwards || changed_selection {
                self.health = loading_health(&key);
                self.diagnostics = loading_diagnostics(&key);
            } else {
                for state in &mut self.health {
                    state.loading = true;
                }
                for state in &mut self.diagnostics {
                    state.loading = true;
                }
            }
            if key.health.is_empty() && key.diagnostics.is_empty() {
                if let Some(worker) = &self.worker {
                    worker.generation.fetch_add(1, Ordering::AcqRel);
                    let mut mailbox = worker.shared.0.lock().expect("window query mailbox");
                    mailbox.pending = None;
                    mailbox.result = None;
                }
                self.desired = Some(key);
                return;
            }
            if self.worker.is_none() {
                match start_worker() {
                    Ok(worker) => {
                        self.worker = Some(worker);
                        self.reset_source = true;
                    }
                    Err(error) => {
                        self.fail(Arc::new(WindowLoadError::Start(error)));
                        self.desired = Some(key);
                        return;
                    }
                }
            }
            let worker = self.worker.as_ref().unwrap();
            // Forward cursor churn coalesces without repeatedly starving a running query.
            // Backward seeks and source/demand changes cancel the old generation.
            let generation = if backwards || changed_selection || self.reset_source {
                worker.generation.fetch_add(1, Ordering::AcqRel) + 1
            } else {
                worker.generation.load(Ordering::Acquire)
            };
            let mut mailbox = worker.shared.0.lock().expect("window query mailbox");
            let new_source = if self.reset_source {
                Some(source.fork_reader())
            } else {
                mailbox
                    .pending
                    .as_mut()
                    .and_then(|demand| demand.source.take())
            };
            mailbox.pending = Some(Demand {
                key: key.clone(),
                generation,
                source: new_source,
            });
            self.reset_source = false;
            self.queued_at = Some(Instant::now());
            self.desired = Some(key);
            worker.shared.1.notify_one();
        }
        if let Some(worker) = &self.worker {
            let result = worker
                .shared
                .0
                .lock()
                .expect("window query mailbox")
                .result
                .take();
            if let Some(result) = result
                && result.generation == worker.generation.load(Ordering::Acquire)
                && self.desired.as_ref().is_some_and(|desired| {
                    desired.health == result.key.health
                        && desired.diagnostics == result.key.diagnostics
                        && result.key.cursor <= cursor
                })
            {
                let loading = cursor != result.key.cursor;
                self.health = result.health;
                self.diagnostics = result.diagnostics;
                for state in &mut self.health {
                    state.loading = loading;
                }
                for state in &mut self.diagnostics {
                    state.loading = loading;
                }
            }
            if worker.handle.is_finished() {
                self.fail(Arc::new(WindowLoadError::WorkerStopped));
            }
        }
    }
    fn fail(&mut self, error: Arc<WindowLoadError>) {
        for state in &mut self.diagnostics {
            state.loading = false;
            state.snapshot = None;
            state.error = Some(error.clone());
        }
        for state in &mut self.health {
            state.loading = false;
            state.snapshot = None;
            state.error = Some(error.clone());
        }
    }
}
fn loading_health(key: &DemandKey) -> Vec<TopicHealthQueryState> {
    key.health
        .iter()
        .map(|requirement| TopicHealthQueryState {
            requirement: *requirement,
            cursor: key.cursor,
            loading: true,
            snapshot: None,
            error: None,
        })
        .collect()
}
fn loading_diagnostics(key: &DemandKey) -> Vec<DiagnosticsQueryState> {
    key.diagnostics
        .iter()
        .map(|requirement| DiagnosticsQueryState {
            requirement: requirement.clone(),
            cursor: key.cursor,
            loading: true,
            snapshot: None,
            error: None,
        })
        .collect()
}
fn start_worker() -> std::io::Result<Worker> {
    let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
    let generation = Arc::new(AtomicU64::new(0));
    let thread_shared = shared.clone();
    let thread_generation = generation.clone();
    let handle = std::thread::Builder::new()
        .name("recording-window-query".into())
        .spawn(move || run_worker(thread_shared, thread_generation))?;
    Ok(Worker {
        shared,
        generation,
        handle,
    })
}
fn run_worker(shared: Arc<(Mutex<Mailbox>, Condvar)>, generation: Arc<AtomicU64>) {
    let mut source = None;
    loop {
        let demand = {
            let mut mailbox = shared.0.lock().expect("window query mailbox");
            while mailbox.pending.is_none() && !mailbox.shutdown {
                mailbox = shared.1.wait(mailbox).expect("window query mailbox");
            }
            if mailbox.shutdown {
                return;
            }
            mailbox.pending.take().unwrap()
        };
        if let Some(new_source) = demand.source {
            source = Some(new_source);
        }
        let Some(source) = &source else {
            continue;
        };
        let mut health = loading_health(&demand.key);
        let mut diagnostics = loading_diagnostics(&demand.key);
        let too_many = demand.key.health.len() + demand.key.diagnostics.len() > 8;
        for state in &mut health {
            if generation.load(Ordering::Acquire) != demand.generation {
                break;
            }
            let result = if too_many {
                Err(WindowQueryError::Limit {
                    resource: "distinct window queries",
                    maximum: 8,
                })
            } else {
                viewer_core::query_topic_health(source, state.requirement, state.cursor, || {
                    generation.load(Ordering::Acquire) != demand.generation
                })
            };
            state.loading = false;
            match result {
                Ok(snapshot) => state.snapshot = Some(Arc::new(snapshot)),
                Err(error) => state.error = Some(Arc::new(WindowLoadError::Query(error))),
            }
        }
        for state in &mut diagnostics {
            if generation.load(Ordering::Acquire) != demand.generation {
                break;
            }
            let result = if too_many {
                Err(WindowQueryError::Limit {
                    resource: "distinct window queries",
                    maximum: 8,
                })
            } else {
                viewer_core::query_diagnostics(source, &state.requirement, state.cursor, || {
                    generation.load(Ordering::Acquire) != demand.generation
                })
            };
            state.loading = false;
            match result {
                Ok(snapshot) => state.snapshot = Some(Arc::new(snapshot)),
                Err(error) => state.error = Some(Arc::new(WindowLoadError::Query(error))),
            }
        }
        if generation.load(Ordering::Acquire) == demand.generation {
            let mut mailbox = shared.0.lock().expect("window query mailbox");
            if generation.load(Ordering::Acquire) == demand.generation {
                mailbox.result = Some(QueryResults {
                    key: demand.key,
                    generation: demand.generation,
                    health,
                    diagnostics,
                });
            }
        }
    }
}
impl Drop for WindowQueryLoader {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.generation.fetch_add(1, Ordering::AcqRel);
            worker
                .shared
                .0
                .lock()
                .expect("window query mailbox")
                .shutdown = true;
            worker.shared.1.notify_one();
            let _ = worker.handle.join();
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

    fn payload(message: &str) -> Vec<u8> {
        fn u32_value(bytes: &mut Vec<u8>, value: u32) {
            while !(bytes.len() - 4).is_multiple_of(4) {
                bytes.push(0);
            }
            bytes.extend(value.to_le_bytes());
        }
        fn string(bytes: &mut Vec<u8>, value: &str) {
            u32_value(bytes, value.len() as u32 + 1);
            bytes.extend(value.as_bytes());
            bytes.push(0);
        }
        let mut bytes = vec![0, 1, 0, 0];
        u32_value(&mut bytes, 0);
        u32_value(&mut bytes, 0);
        string(&mut bytes, "");
        u32_value(&mut bytes, 1);
        bytes.push(2);
        string(&mut bytes, "camera");
        string(&mut bytes, message);
        string(&mut bytes, "hw");
        u32_value(&mut bytes, 0);
        bytes
    }
    fn source(message: &str) -> McapSource {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = mcap::Writer::with_options(
                &mut output,
                mcap::WriteOptions::new().compression(None),
            )
            .unwrap();
            let schema = writer
                .add_schema("diagnostic_msgs/msg/DiagnosticArray", "ros2msg", b"schema")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/diagnostics", "cdr", &BTreeMap::new())
                .unwrap();
            for time in [100, 200] {
                writer
                    .write_to_known_channel(
                        &mcap::records::MessageHeader {
                            channel_id: channel,
                            sequence: 0,
                            log_time: time,
                            publish_time: time,
                        },
                        &payload(message),
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        McapSource::copy_from_slice(&output.into_inner()).unwrap()
    }
    fn configured() -> WindowQueryLoader {
        let mut loader = WindowQueryLoader::default();
        loader.configure_health(&BTreeSet::from([TopicHealthRequirement::default()]));
        loader.configure_diagnostics(&BTreeSet::from([
            viewer_core::DiagnosticsRequirement::default(),
        ]));
        loader
    }
    fn wait(loader: &mut WindowQueryLoader, source: &McapSource, cursor: ArrivalTime) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            loader.poll(Some(source), Some(cursor));
            if loader.health.iter().all(|s| !s.loading)
                && loader.diagnostics.iter().all(|s| !s.loading)
            {
                break;
            }
            assert!(Instant::now() < deadline, "window query did not finish");
            std::thread::yield_now();
        }
    }
    #[test]
    fn backward_seek_immediately_hides_future_windows_and_requeries() {
        let source = source("timeout");
        let mut loader = configured();
        wait(&mut loader, &source, ArrivalTime(200));
        assert_eq!(
            loader.health[0].snapshot.as_ref().unwrap()[0].sample_count,
            2
        );
        assert_eq!(
            loader.diagnostics[0].snapshot.as_ref().unwrap().events[0].count,
            2
        );
        loader.poll(Some(&source), Some(ArrivalTime(100)));
        assert!(loader.health.iter().all(|s| s.cursor <= ArrivalTime(100)));
        assert!(
            loader
                .diagnostics
                .iter()
                .all(|s| s.cursor <= ArrivalTime(100))
        );
        wait(&mut loader, &source, ArrivalTime(100));
        assert_eq!(
            loader.health[0].snapshot.as_ref().unwrap()[0].sample_count,
            1
        );
        let event = &loader.diagnostics[0].snapshot.as_ref().unwrap().events[0];
        assert_eq!(event.count, 1);
        assert_eq!(event.last_seen, ArrivalTime(100));
    }
    #[test]
    fn source_replacement_reuses_one_worker_and_discards_old_results() {
        let old = source("old");
        let new = source("new");
        let mut previous = configured();
        wait(&mut previous, &old, ArrivalTime(200));
        let worker_id = previous.worker.as_ref().unwrap().handle.thread().id();
        previous.poll(Some(&old), Some(ArrivalTime(100)));
        let mut replacement = configured();
        replacement.take_worker_from(&mut previous);
        assert!(previous.worker.is_none());
        assert!(replacement.health.is_empty());
        assert_eq!(
            replacement.worker.as_ref().unwrap().handle.thread().id(),
            worker_id
        );
        // Replacement cursor churn must preserve a pending source reset.
        replacement.poll(Some(&new), Some(ArrivalTime(100)));
        replacement.poll(Some(&new), Some(ArrivalTime(200)));
        wait(&mut replacement, &new, ArrivalTime(200));
        assert_eq!(
            replacement.diagnostics[0].snapshot.as_ref().unwrap().events[0].message,
            "new"
        );
    }
    #[test]
    fn session_queries_preserve_playback_selection_and_seek_invalidates_results() {
        let workspace = crate::workspace::NativeWorkspace::load_bundled_or_fallback(
            crate::workspace::WorkspaceLayout::Health,
        );
        let requirements = workspace.data_requirements();
        let bindings = viewer_core::WorkspaceBindings {
            path_topic: "/path".into(),
            odometry_topic: "/odom".into(),
            point_cloud_topic: "/scan".into(),
            dynamic_tf_topic: "/tf".into(),
            static_tf_topic: "/tf_static".into(),
        };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/camera-jpeg/camera_front_3s.mcap");
        let mut session = crate::session::ViewerSession::open(
            &path,
            "/unused".into(),
            &requirements.playback,
            &bindings,
        )
        .unwrap();
        session.configure_window_queries(&requirements);
        let cursor = session.playback_view().unwrap().cursor;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            session.poll_queries();
            if session.topic_health().first().is_some_and(|s| !s.loading) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(session.topic_health()[0].snapshot.is_some());
        assert!(session.plan().selected_stream_ids().is_empty());
        assert_eq!(session.playback_view().unwrap().cursor, cursor);
        let end = session.playback_view().unwrap().end;
        let effect = session
            .apply_playback_command(viewer_core::PlaybackCommand::Seek(end), |_, _| Ok(()))
            .unwrap();
        assert_eq!(effect, viewer_core::PlaybackEffect::Seeked);
        assert!(session.topic_health().is_empty());
        assert!(session.producer_diagnostics().is_empty());
        assert!(session.plan().selected_stream_ids().is_empty());
    }

    #[test]
    fn unchanged_demand_reuses_one_result_and_one_worker() {
        let source = source("timeout");
        let mut loader = configured();
        wait(&mut loader, &source, ArrivalTime(200));
        let snapshot = loader.health[0].snapshot.as_ref().unwrap().clone();
        let generation = loader
            .worker
            .as_ref()
            .unwrap()
            .generation
            .load(Ordering::Acquire);
        for _ in 0..50 {
            loader.poll(Some(&source), Some(ArrivalTime(200)));
        }
        assert_eq!(loader.health.len(), 1);
        assert_eq!(loader.diagnostics.len(), 1);
        assert!(Arc::ptr_eq(
            &snapshot,
            loader.health[0].snapshot.as_ref().unwrap()
        ));
        assert_eq!(
            loader
                .worker
                .as_ref()
                .unwrap()
                .generation
                .load(Ordering::Acquire),
            generation
        );
        assert!(
            loader
                .worker
                .as_ref()
                .unwrap()
                .shared
                .0
                .lock()
                .unwrap()
                .pending
                .is_none()
        );
    }
    #[test]
    fn forward_updates_publish_completed_results_and_errors_are_isolated() {
        let source = source("timeout");
        let mut loader = configured();
        loader.configure_diagnostics(&BTreeSet::from([viewer_core::DiagnosticsRequirement {
            topic: "/absent".into(),
            ..Default::default()
        }]));
        wait(&mut loader, &source, ArrivalTime(100));
        loader.poll(Some(&source), Some(ArrivalTime(150)));
        assert!(loader.health[0].snapshot.is_some());
        wait(&mut loader, &source, ArrivalTime(150));
        assert!(loader.health[0].error.is_none());
        assert!(loader.diagnostics[0].error.is_some());
    }
}
