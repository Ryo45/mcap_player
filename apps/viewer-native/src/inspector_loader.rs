use crate::inspection::{InspectedMessage, InspectorRequirement, TopicInspection};
use crate::query_error::QueryLoadError;
use anyhow::Result;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc::{Receiver, SyncSender, TryRecvError},
};

#[cfg(test)]
fn load_inspections_from_path(
    path: &Path,
    requirements: &[InspectorRequirement],
    is_current: impl Fn() -> bool,
) -> std::result::Result<Vec<TopicInspection>, QueryLoadError> {
    let source = viewer_core::McapSource::open_file(path)?;
    load_inspections_from_source(&source, requirements, is_current)
}

#[cfg(test)]
use std::path::{Path, PathBuf};

const INSPECTOR_PAYLOAD_BUDGET: usize = 16 * 1024 * 1024;

pub(crate) struct InspectorRequest {
    pub(crate) source: viewer_core::McapSource,
    pub(crate) requirements: Vec<InspectorRequirement>,
}

pub(crate) struct InspectorLoader {
    generation: Arc<AtomicU64>,
    state: InspectorLoadState,
    result_sender: SyncSender<InspectorLoadResult>,
    result_receiver: Receiver<InspectorLoadResult>,
}

enum InspectorLoadState {
    Idle,
    Loading {
        generation: u64,
        inspections: Vec<TopicInspection>,
    },
    Ready {
        generation: u64,
        inspections: Vec<TopicInspection>,
    },
}

struct InspectorLoadResult {
    generation: u64,
    result: std::result::Result<Vec<TopicInspection>, Arc<QueryLoadError>>,
    requirements: Vec<InspectorRequirement>,
}

impl Default for InspectorLoader {
    fn default() -> Self {
        let (result_sender, result_receiver) = std::sync::mpsc::sync_channel(1);
        Self {
            generation: Arc::new(AtomicU64::new(0)),
            state: InspectorLoadState::Idle,
            result_sender,
            result_receiver,
        }
    }
}

impl InspectorLoader {
    pub(crate) fn clear(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.state = InspectorLoadState::Idle;
        self.discard_pending_results();
    }

    pub(crate) fn start(&mut self, request: InspectorRequest) -> Result<()> {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.discard_pending_results();
        let requirements = request.requirements;
        let loading = requirements
            .iter()
            .map(|requirement| TopicInspection::loading(requirement.topic.clone()))
            .collect();
        let sender = self.result_sender.clone();
        let active_generation = Arc::clone(&self.generation);
        let worker_requirements = requirements.clone();
        let worker = std::thread::Builder::new()
            .name("inspector-loader".to_owned())
            .spawn(move || {
                let result =
                    load_inspections_from_source(&request.source, &worker_requirements, || {
                        active_generation.load(Ordering::Acquire) == generation
                    })
                    .map_err(Arc::new);
                if active_generation.load(Ordering::Acquire) == generation {
                    let _ = sender.send(InspectorLoadResult {
                        generation,
                        result,
                        requirements: worker_requirements,
                    });
                }
            });
        match worker {
            Ok(_) => {
                self.state = InspectorLoadState::Loading {
                    generation,
                    inspections: loading,
                };
                Ok(())
            }
            Err(error) => {
                let error = Arc::new(QueryLoadError::StartWorker {
                    worker: "inspector",
                    source: error,
                });
                self.state = InspectorLoadState::Ready {
                    generation,
                    inspections: failed_inspections(&requirements, error.clone()),
                };
                Err(anyhow::Error::new(error))
            }
        }
    }

    pub(crate) fn poll(&mut self) {
        loop {
            match self.result_receiver.try_recv() {
                Ok(result) => self.apply_result(result),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    pub(crate) fn inspections(&self) -> &[TopicInspection] {
        match &self.state {
            InspectorLoadState::Loading {
                generation,
                inspections,
            }
            | InspectorLoadState::Ready {
                generation,
                inspections,
            } if *generation == self.generation.load(Ordering::Acquire) => inspections,
            _ => &[],
        }
    }

    #[cfg(test)]
    pub(crate) fn is_loading(&self) -> bool {
        matches!(
            self.state,
            InspectorLoadState::Loading { generation, .. }
                if generation == self.generation.load(Ordering::Acquire)
        )
    }

    fn apply_result(&mut self, result: InspectorLoadResult) {
        if result.generation != self.generation.load(Ordering::Acquire) {
            return;
        }
        let inspections = match result.result {
            Ok(inspections) => inspections,
            Err(error) => failed_inspections(&result.requirements, error),
        };
        self.state = InspectorLoadState::Ready {
            generation: result.generation,
            inspections,
        };
    }

    fn discard_pending_results(&mut self) {
        while self.result_receiver.try_recv().is_ok() {}
    }
}

impl Drop for InspectorLoader {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

fn failed_inspections(
    requirements: &[InspectorRequirement],
    error: Arc<QueryLoadError>,
) -> Vec<TopicInspection> {
    requirements
        .iter()
        .map(|requirement| TopicInspection::failed(requirement.topic.clone(), error.clone()))
        .collect()
}

fn load_inspections_from_source(
    source: &viewer_core::McapSource,
    requirements: &[InspectorRequirement],
    is_current: impl Fn() -> bool,
) -> std::result::Result<Vec<TopicInspection>, QueryLoadError> {
    let recording = source.time_range();
    let range = viewer_core::DataWindowTimeRange::new(recording.start, recording.end_exclusive)
        .expect("validated recording range");
    let mut inspections = Vec::with_capacity(requirements.len());
    for requirement in requirements {
        if !is_current() {
            break;
        }
        let result = source
            .catalog()
            .by_topic(&requirement.topic)
            .ok_or_else(|| QueryLoadError::MissingTopic(requirement.topic.clone()))
            .and_then(|stream| {
                let limits = viewer_core::QueryLimits::new(
                    requirement.max_messages,
                    INSPECTOR_PAYLOAD_BUDGET,
                )?;
                source
                    .query_range(&viewer_core::RangeQuery {
                        streams: vec![stream.id],
                        range,
                        limits,
                    })
                    .map_err(QueryLoadError::from)
            });
        inspections.push(match result {
            Ok(result) => TopicInspection::ready(
                requirement.topic.clone(),
                result
                    .messages
                    .into_iter()
                    .map(|message| InspectedMessage {
                        arrival_time: message.arrival_time,
                        payload_bytes: message.payload.len(),
                    })
                    .collect(),
            ),
            Err(error) => TopicInspection::failed(requirement.topic.clone(), Arc::new(error)),
        });
    }
    Ok(inspections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{session::ViewerSession, workspace::NativeWorkspace};
    use std::time::Duration;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap")
    }

    fn requirement() -> InspectorRequirement {
        InspectorRequirement {
            topic: "/odom".to_owned(),
            max_messages: 3,
        }
    }

    impl InspectorLoader {
        fn poll_until_settled_for_test(&mut self) {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while self.is_loading() && std::time::Instant::now() < deadline {
                self.poll();
                std::thread::yield_now();
            }
            assert!(!self.is_loading(), "inspector worker did not settle");
        }
    }

    #[test]
    fn exact_inspection_is_bounded_and_ordered() {
        let inspections =
            load_inspections_from_path(&fixture(), &[requirement()], || true).unwrap();
        let messages = &inspections[0].messages;
        assert_eq!(messages.len(), 3);
        assert!(
            messages
                .windows(2)
                .all(|pair| pair[0].arrival_time <= pair[1].arrival_time)
        );
        assert!(messages.iter().all(|message| message.payload_bytes > 0));
    }

    #[test]
    fn start_publishes_loading_before_the_background_result() {
        let mut loader = InspectorLoader::default();
        loader
            .start(InspectorRequest {
                source: viewer_core::McapSource::open_file(&fixture()).unwrap(),
                requirements: vec![requirement()],
            })
            .unwrap();
        assert!(loader.is_loading());
        assert!(loader.inspections()[0].loading);
        loader.poll_until_settled_for_test();
        assert!(!loader.inspections()[0].loading);
        assert_eq!(loader.inspections()[0].messages.len(), 3);
    }

    #[test]
    fn stale_generation_result_cannot_replace_current_state() {
        let mut loader = InspectorLoader::default();
        loader
            .start(InspectorRequest {
                source: viewer_core::McapSource::open_file(&fixture()).unwrap(),
                requirements: vec![requirement()],
            })
            .unwrap();
        let stale = loader.generation.load(Ordering::Acquire);
        loader.clear();
        loader.apply_result(InspectorLoadResult {
            generation: stale,
            result: Err(Arc::new(QueryLoadError::MissingTopic(
                "stale failure".into(),
            ))),
            requirements: vec![requirement()],
        });
        assert!(loader.inspections().is_empty());
    }

    #[test]
    fn playback_progresses_while_inspection_uses_its_own_source() {
        let workspace = NativeWorkspace::default();
        let mut requirements = viewer_core::PlaybackRequirements::empty();
        requirements.require_all_cameras();
        let mut session = ViewerSession::open(
            &fixture(),
            "/camera/front/image/compressed".to_owned(),
            &requirements,
            workspace.bindings(),
        )
        .unwrap();
        let start = session.playback_view().unwrap().cursor;
        let mut loader = InspectorLoader::default();
        loader
            .start(InspectorRequest {
                source: viewer_core::McapSource::open_file(&fixture()).unwrap(),
                requirements: vec![requirement()],
            })
            .unwrap();

        session.tick(Duration::from_millis(250), |_, _| {}).unwrap();

        let progressed = session.playback_view().unwrap().cursor;
        assert!(progressed > start);
        loader.poll_until_settled_for_test();
        assert_eq!(session.playback_view().unwrap().cursor, progressed);
    }
}
