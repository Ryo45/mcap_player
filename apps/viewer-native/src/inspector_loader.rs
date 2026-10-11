//! One coalescing cursor-query worker; no recording-duration history and no playback routes.
use crate::inspection::{InspectionMetadata, InspectorRequirement, TopicInspection};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::JoinHandle,
};
use viewer_core::{ArrivalTime, InspectorDecodeError, InspectorMessage, InspectorSchema, StreamId};
use viewer_mcap::{IndexedMessagePosition, LatestMessageReader, McapSource};
const MAX_TOPICS: usize = 8;
#[derive(Debug)]
pub(crate) enum InspectorError {
    MissingTopic(String),
    TooManyTopics(usize),
    Source(viewer_mcap::McapOpenError),
    Decode(InspectorDecodeError),
    Start(std::io::Error),
    WorkerPanicked,
}
impl std::fmt::Display for InspectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingTopic(topic) => write!(f, "recording has no topic {topic}"),
            Self::TooManyTopics(count) => write!(
                f,
                "Inspector supports at most {MAX_TOPICS} topics, got {count}"
            ),
            Self::Source(error) => error.fmt(f),
            Self::Decode(error) => error.fmt(f),
            Self::Start(error) => write!(f, "start Inspector worker: {error}"),
            Self::WorkerPanicked => write!(f, "Inspector worker stopped unexpectedly"),
        }
    }
}
impl std::error::Error for InspectorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            Self::Decode(error) => Some(error),
            Self::Start(error) => Some(error),
            _ => None,
        }
    }
}
impl From<viewer_mcap::McapOpenError> for InspectorError {
    fn from(error: viewer_mcap::McapOpenError) -> Self {
        Self::Source(error)
    }
}
impl From<InspectorDecodeError> for InspectorError {
    fn from(error: InspectorDecodeError) -> Self {
        Self::Decode(error)
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct DemandKey {
    target: ArrivalTime,
    topics: Vec<String>,
}
struct Demand {
    generation: u64,
    key: DemandKey,
    source: Option<McapSource>,
}
struct ResultSnapshot {
    generation: u64,
    key: DemandKey,
    inspections: Vec<TopicInspection>,
}
#[derive(Default)]
struct Mailbox {
    pending: Option<Demand>,
    result: Option<ResultSnapshot>,
    shutdown: bool,
}
struct Worker {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
    generation: Arc<AtomicU64>,
    handle: JoinHandle<()>,
}
#[derive(Default)]
pub(crate) struct InspectorLoader {
    requirements: Vec<InspectorRequirement>,
    selected_topic: Option<String>,
    desired: Option<DemandKey>,
    inspections: Vec<TopicInspection>,
    worker: Option<Worker>,
    reset_source: bool,
}
impl InspectorLoader {
    pub fn configure(&mut self, requirements: &[InspectorRequirement]) {
        self.requirements = requirements.to_vec();
        self.desired = None;
    }
    pub fn select_topic(&mut self, topic: Option<String>) {
        self.selected_topic = topic;
        self.desired = None;
    }
    pub fn selected_topic(&self) -> Option<&str> {
        self.selected_topic.as_deref()
    }
    pub fn inspections(&self) -> &[TopicInspection] {
        &self.inspections
    }
    pub fn take_worker_from(&mut self, previous: &mut Self) {
        if let Some(worker) = previous.worker.take() {
            worker.generation.fetch_add(1, Ordering::AcqRel);
            {
                let mut mailbox = worker.shared.0.lock().expect("Inspector mailbox");
                mailbox.pending = None;
                mailbox.result = None;
            }
            self.worker = Some(worker);
            self.reset_source = true;
        }
    }
    pub fn poll(&mut self, source: Option<&McapSource>, target: Option<ArrivalTime>) {
        let (Some(source), Some(target)) = (source, target) else {
            return;
        };
        let mut topics = self
            .requirements
            .iter()
            .map(|r| r.topic.clone())
            .collect::<BTreeSet<_>>();
        if let Some(topic) = &self.selected_topic {
            topics.insert(topic.clone());
        }
        let mut topics = topics.into_iter().collect::<Vec<_>>();
        if let Some(selected) = &self.selected_topic
            && let Some(index) = topics.iter().position(|topic| topic == selected)
        {
            topics.swap(0, index);
        }
        let key = DemandKey { target, topics };
        if self.desired.as_ref() != Some(&key) {
            if key.topics.is_empty() {
                self.inspections.clear();
                self.desired = Some(key);
                if let Some(worker) = &self.worker {
                    worker.generation.fetch_add(1, Ordering::AcqRel);
                }
                return;
            }
            if self.worker.is_none() {
                let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
                let generation = Arc::new(AtomicU64::new(0));
                let (thread_shared, thread_generation) = (shared.clone(), generation.clone());
                match std::thread::Builder::new()
                    .name("inspector-cursor".into())
                    .spawn(move || run_worker(thread_shared, thread_generation))
                {
                    Ok(handle) => {
                        self.worker = Some(Worker {
                            shared,
                            generation,
                            handle,
                        });
                        self.reset_source = true;
                    }
                    Err(error) => {
                        let error = Arc::new(InspectorError::Start(error));
                        self.inspections = key
                            .topics
                            .iter()
                            .map(|topic| failed(topic.clone(), target, error.clone()))
                            .collect();
                        self.desired = Some(key);
                        return;
                    }
                }
            }
            let worker = self.worker.as_ref().unwrap();
            let generation = if self.reset_source
                || self
                    .desired
                    .as_ref()
                    .is_none_or(|old| old.topics != key.topics)
            {
                worker.generation.fetch_add(1, Ordering::AcqRel) + 1
            } else {
                worker.generation.load(Ordering::Acquire)
            };
            if self
                .desired
                .as_ref()
                .is_none_or(|old| old.topics != key.topics)
            {
                self.inspections = key
                    .topics
                    .iter()
                    .map(|topic| TopicInspection::loading(topic.clone(), target))
                    .collect();
            } else {
                for inspection in &mut self.inspections {
                    inspection.loading = true;
                }
            }
            let mut mailbox = worker.shared.0.lock().expect("Inspector mailbox");
            let source = self.reset_source.then(|| source.fork_reader()).or_else(|| {
                mailbox
                    .pending
                    .as_mut()
                    .and_then(|pending| pending.source.take())
            });
            mailbox.pending = Some(Demand {
                generation,
                key: key.clone(),
                source,
            });
            self.reset_source = false;
            // A pending source reset must not be lost when a cursor update replaces its request.
            self.desired = Some(key);
            worker.shared.1.notify_one();
        }
        if let Some(worker) = &self.worker {
            let result = worker
                .shared
                .0
                .lock()
                .expect("Inspector mailbox")
                .result
                .take();
            if let Some(result) = result
                && result.generation == worker.generation.load(Ordering::Acquire)
                && self
                    .desired
                    .as_ref()
                    .is_some_and(|desired| desired.topics == result.key.topics)
            {
                self.inspections = result.inspections;
                for inspection in &mut self.inspections {
                    inspection.loading = self
                        .desired
                        .as_ref()
                        .is_some_and(|desired| desired.target != inspection.target);
                }
            }
            if worker.handle.is_finished() && self.inspections.iter().any(|i| i.loading) {
                let error = Arc::new(InspectorError::WorkerPanicked);
                self.inspections = self
                    .inspections
                    .iter()
                    .map(|i| failed(i.topic.clone(), target, error.clone()))
                    .collect();
            }
        }
    }
}
impl Drop for InspectorLoader {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.generation.fetch_add(1, Ordering::AcqRel);
            {
                worker.shared.0.lock().expect("Inspector mailbox").shutdown = true;
            }
            worker.shared.1.notify_one();
            let _ = worker.handle.join();
        }
    }
}
fn failed(topic: String, target: ArrivalTime, error: Arc<InspectorError>) -> TopicInspection {
    let mut result = TopicInspection::loading(topic, target);
    result.loading = false;
    result.error = Some(error);
    result
}
#[derive(Default)]
struct CachedView {
    streams: Option<Vec<StreamId>>,
    schema: Option<(StreamId, InspectorSchema)>,
    identity: Option<(StreamId, IndexedMessagePosition)>,
    message: Option<Arc<InspectorMessage>>,
}
fn run_worker(shared: Arc<(Mutex<Mailbox>, Condvar)>, generation: Arc<AtomicU64>) {
    let mut reader = None;
    let mut views = BTreeMap::<String, CachedView>::new();
    loop {
        let demand = {
            let mut mailbox = shared.0.lock().expect("Inspector mailbox");
            while mailbox.pending.is_none() && !mailbox.shutdown {
                mailbox = shared.1.wait(mailbox).expect("Inspector mailbox");
            }
            if mailbox.shutdown {
                return;
            }
            mailbox.pending.take().unwrap()
        };
        if let Some(source) = demand.source {
            reader = Some(LatestMessageReader::new(source));
            views.clear();
        }
        let Some(reader) = &mut reader else {
            continue;
        };
        views.retain(|topic, _| demand.key.topics.contains(topic));
        let mut inspections = Vec::new();
        let mut payload_bytes = 0usize;
        for topic in &demand.key.topics {
            if generation.load(Ordering::Acquire) != demand.generation {
                break;
            }
            if demand.key.topics.len() > MAX_TOPICS {
                inspections.push(failed(
                    topic.clone(),
                    demand.key.target,
                    Arc::new(InspectorError::TooManyTopics(demand.key.topics.len())),
                ));
                continue;
            }
            let view = views.entry(topic.clone()).or_default();
            let inspection = inspect_topic(
                reader,
                view,
                topic,
                demand.key.target,
                viewer_core::INSPECTOR_MAX_PAYLOAD_BYTES.saturating_sub(payload_bytes),
                || generation.load(Ordering::Acquire) != demand.generation,
            );
            if let Some(message) = &inspection.message {
                payload_bytes += message.retained_payload_bytes();
            }
            inspections.push(inspection);
        }
        if generation.load(Ordering::Acquire) == demand.generation {
            let mut mailbox = shared.0.lock().expect("Inspector mailbox");
            if generation.load(Ordering::Acquire) == demand.generation {
                mailbox.result = Some(ResultSnapshot {
                    generation: demand.generation,
                    key: demand.key,
                    inspections,
                });
            }
        }
    }
}
fn inspect_topic(
    reader: &mut LatestMessageReader,
    view: &mut CachedView,
    topic: &str,
    target: ArrivalTime,
    remaining: usize,
    mut cancelled: impl FnMut() -> bool,
) -> TopicInspection {
    let mut result = TopicInspection::loading(topic.into(), target);
    result.loading = false;
    let outcome = (|| -> Result<(), InspectorError> {
        let streams = view.streams.get_or_insert_with(|| {
            reader
                .catalog()
                .streams
                .iter()
                .filter(|s| s.topic == topic)
                .map(|s| s.id)
                .collect()
        });
        if streams.is_empty() {
            return Err(InspectorError::MissingTopic(topic.into()));
        }
        let mut selected = None;
        for stream in streams.iter().copied() {
            if let Some((position, message)) =
                reader.read_at_cancellable(stream, target, &mut cancelled)?
                && selected.as_ref().is_none_or(
                    |(_, old): &(IndexedMessagePosition, viewer_core::RawMessage)| {
                        (message.arrival_time, message.stream_id)
                            >= (old.arrival_time, old.stream_id)
                    },
                )
            {
                selected = Some((position, message));
            }
        }
        let Some((position, raw)) = selected else {
            view.message = None;
            view.identity = None;
            return Ok(());
        };
        result.metadata = Some(InspectionMetadata {
            stream_id: raw.stream_id,
            log_time: raw.arrival_time,
            publish_time_ns: raw.publish_time_ns,
            sequence: raw.sequence,
            payload_bytes: raw.payload.len(),
        });
        if raw.payload.len() > remaining {
            return Err(InspectorDecodeError::PayloadTooLarge {
                size: raw.payload.len(),
                limit: remaining,
            }
            .into());
        }
        let identity = (raw.stream_id, position);
        if view.identity != Some(identity) {
            if view
                .schema
                .as_ref()
                .is_none_or(|(stream, _)| *stream != raw.stream_id)
            {
                view.schema = Some((
                    raw.stream_id,
                    InspectorSchema::for_stream(
                        reader
                            .catalog()
                            .by_id(raw.stream_id)
                            .expect("query stream in catalog"),
                    )?,
                ));
            }
            let message = InspectorMessage::new(&view.schema.as_ref().unwrap().1, &raw)?;
            view.message = Some(Arc::new(message));
            view.identity = Some(identity);
        }
        result.message = view.message.clone();
        Ok(())
    })();
    if let Err(error) = outcome {
        view.message = None;
        view.identity = None;
        result.error = Some(Arc::new(error));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        io::Cursor,
        time::{Duration, Instant},
    };
    fn source(value_offset: u32) -> McapSource {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = mcap::Writer::new(&mut bytes).unwrap();
            let schema = writer
                .add_schema("demo/msg/Scalar", "ros2msg", b"uint32 value\n")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/scalar", "cdr", &BTreeMap::new())
                .unwrap();
            for time in [10u64, 20, 30] {
                let mut payload = vec![0, 1, 0, 0];
                payload.extend((value_offset + time as u32).to_le_bytes());
                writer
                    .write_to_known_channel(
                        &mcap::records::MessageHeader {
                            channel_id: channel,
                            sequence: time as u32,
                            log_time: time,
                            publish_time: time - 1,
                        },
                        &payload,
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        McapSource::from_bytes(bytes.into_inner().into()).unwrap()
    }
    fn settle(loader: &mut InspectorLoader, source: &McapSource, target: ArrivalTime) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            loader.poll(Some(source), Some(target));
            if !loader.inspections.is_empty()
                && loader
                    .inspections
                    .iter()
                    .all(|i| !i.loading && i.target == target)
            {
                return;
            }
            assert!(Instant::now() < deadline, "Inspector did not settle");
            std::thread::yield_now();
        }
    }
    fn value(model: &InspectorMessage) -> u64 {
        let page = model.page(&model.root, None).unwrap();
        let viewer_core::InspectorValue::Scalar(viewer_core::InspectorScalar::Unsigned(value)) =
            page.fields[0].value
        else {
            panic!("expected scalar")
        };
        value
    }
    #[test]
    fn cursor_boundaries_empty_predecessor_and_same_message_view_reuse() {
        let source = source(0);
        let mut loader = InspectorLoader::default();
        loader.select_topic(Some("/scalar".into()));
        settle(&mut loader, &source, ArrivalTime(9));
        assert!(loader.inspections[0].metadata.is_none());
        assert!(loader.inspections[0].error.is_none());
        settle(&mut loader, &source, ArrivalTime(10));
        let message = loader.inspections[0].message.clone().unwrap();
        assert_eq!(value(&message), 10);
        settle(&mut loader, &source, ArrivalTime(19));
        assert!(Arc::ptr_eq(
            &message,
            loader.inspections[0].message.as_ref().unwrap()
        ));
        settle(&mut loader, &source, ArrivalTime(20));
        assert_eq!(value(loader.inspections[0].message.as_ref().unwrap()), 20);
        settle(&mut loader, &source, ArrivalTime(10));
        assert_eq!(
            loader.inspections[0].metadata.unwrap().log_time,
            ArrivalTime(10)
        );
    }
    #[test]
    fn coalescing_does_not_lose_initial_source_or_latest_cursor() {
        let source = source(0);
        let mut loader = InspectorLoader::default();
        loader.select_topic(Some("/scalar".into()));
        for target in 10..1000 {
            loader.poll(Some(&source), Some(ArrivalTime(target)));
        }
        settle(&mut loader, &source, ArrivalTime(999));
        assert_eq!(value(loader.inspections[0].message.as_ref().unwrap()), 30);
        loader.select_topic(None);
        loader.poll(Some(&source), Some(ArrivalTime(999)));
        assert!(loader.inspections.is_empty());
    }
    #[test]
    fn source_replacement_keeps_one_worker_and_rejects_old_recording_results() {
        let first = source(0);
        let mut old = InspectorLoader::default();
        old.select_topic(Some("/scalar".into()));
        old.poll(Some(&first), Some(ArrivalTime(10)));
        let second = source(100);
        let mut new = InspectorLoader::default();
        new.take_worker_from(&mut old);
        assert!(old.worker.is_none());
        new.select_topic(Some("/scalar".into()));
        settle(&mut new, &second, ArrivalTime(10));
        assert_eq!(value(new.inspections[0].message.as_ref().unwrap()), 110);
    }
    #[test]
    fn missing_topic_reports_a_typed_error_without_expanding_playback_requirements() {
        let source = source(0);
        let mut loader = InspectorLoader::default();
        loader.configure(&[InspectorRequirement {
            topic: "/missing".into(),
        }]);
        settle(&mut loader, &source, ArrivalTime(10));
        assert!(
            matches!(loader.inspections[0].error.as_deref(),Some(InspectorError::MissingTopic(topic)) if topic=="/missing")
        );
    }
}

#[cfg(test)]
mod topic_merge_tests {
    use super::*;
    #[test]
    fn latest_topic_message_is_selected_across_channels_and_metadata_survives_unsupported_schema() {
        let mut output = std::io::Cursor::new(Vec::new());
        {
            let mut writer = mcap::Writer::new(&mut output).unwrap();
            let schema = writer
                .add_schema("demo/msg/A", "ros2msg", b"uint32 value\n")
                .unwrap();
            let one = writer
                .add_channel(schema, "/topic", "cdr", &std::collections::BTreeMap::new())
                .unwrap();
            let two = writer
                .add_channel(
                    schema,
                    "/topic",
                    "cdr",
                    &std::collections::BTreeMap::from([("publisher".into(), "second".into())]),
                )
                .unwrap();
            let unsupported = writer
                .add_channel(0, "/json", "json", &std::collections::BTreeMap::new())
                .unwrap();
            for (channel, time) in [(one, 10u64), (two, 20), (one, 30), (unsupported, 25)] {
                writer
                    .write_to_known_channel(
                        &mcap::records::MessageHeader {
                            channel_id: channel,
                            sequence: 7,
                            log_time: time,
                            publish_time: time - 1,
                        },
                        &[0, 1, 0, 0, 42, 0, 0, 0],
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        let source = McapSource::from_bytes(output.into_inner().into()).unwrap();
        let mut reader = LatestMessageReader::new(source);
        let mut view = CachedView::default();
        let latest = inspect_topic(
            &mut reader,
            &mut view,
            "/topic",
            ArrivalTime(25),
            16 * 1024 * 1024,
            || false,
        );
        assert_eq!(latest.metadata.unwrap().log_time, ArrivalTime(20));
        assert!(latest.message.is_some());
        let latest = inspect_topic(
            &mut reader,
            &mut view,
            "/topic",
            ArrivalTime(35),
            16 * 1024 * 1024,
            || false,
        );
        assert_eq!(latest.metadata.unwrap().log_time, ArrivalTime(30));
        let unknown = inspect_topic(
            &mut reader,
            &mut CachedView::default(),
            "/json",
            ArrivalTime(30),
            16 * 1024 * 1024,
            || false,
        );
        assert_eq!(unknown.metadata.unwrap().sequence, Some(7));
        assert_eq!(unknown.metadata.unwrap().publish_time_ns, Some(24));
        assert!(matches!(
            unknown.error.as_deref(),
            Some(InspectorError::Decode(_))
        ));
    }
}
