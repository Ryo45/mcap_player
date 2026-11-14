//! Existing independent query/decoder path, presented by the shared egui field tree.
use crate::{
    inspector_query::InspectorQueryState,
    playback::{WebPlayback, WebRestoreLoader},
    topics::{inspection_plan, latest_topic_message},
};
use std::sync::Arc;
use viewer_core::{InspectionMetadata, InspectorMessage, InspectorSchema, StreamId};
use viewer_ui::inspector::InspectionView;

#[derive(Default)]
pub(super) struct Inspection {
    pub selected: Option<String>,
    pub message: Option<Arc<InspectorMessage>>,
    pub status: String,
    pub(super) error: Option<String>,
    pub metadata: Option<InspectionMetadata>,
    target: viewer_core::ArrivalTime,
    pub automatic: bool,
    was_playing: bool,
    query: InspectorQueryState,
    loader: Option<WebRestoreLoader>,
    schema: Option<(StreamId, Result<InspectorSchema, String>)>,
}
impl Inspection {
    pub fn reset(&mut self, session: &WebPlayback) {
        *self = Self {
            loader: Some(session.inspector_loader()),
            automatic: true,
            ..Self::default()
        };
    }
    pub fn select(&mut self, topic: String) {
        if let Some(loader) = &mut self.loader {
            loader.cancel();
        }
        self.query.select(Some(topic.clone()));
        self.selected = Some(topic);
        self.message = None;
        self.schema = None;
        self.metadata = None;
        self.status.clear();
        self.error = None;
    }
    pub fn invalidate(&mut self) {
        if let Some(loader) = &mut self.loader {
            loader.cancel();
        }
        self.query.select(self.selected.clone());
        self.error = None;
        self.message = None;
        self.metadata = None;
    }
    pub fn settle(&mut self, session: &WebPlayback) {
        if let Some(loader) = &mut self.loader {
            loader.cancel();
        }
        self.query.settle(session.clock().cursor());
        self.target = session.clock().cursor();
        self.was_playing = session.clock().is_playing();
        self.message = None;
        self.metadata = None;
        self.error = None;
        self.status = "Loading exact LatestBefore…".into();
    }
    pub fn in_flight(&self) -> bool {
        self.query.active().is_some()
    }
    pub fn poll(&mut self, session: &WebPlayback, admit: bool) {
        if self.was_playing && !session.clock().is_playing() {
            self.settle(session);
        }
        self.was_playing = session.clock().is_playing();
        if let Some(result) = self.loader.as_mut().and_then(WebRestoreLoader::poll)
            && let Some(request) = self.query.active().cloned()
            && self.query.complete(&request)
        {
            let result = result.map_err(|e| e.to_string()).and_then(|messages| {
                latest_topic_message(session.catalog(), &request.topic, request.target, messages)
            });
            self.error = None;
            self.target = request.target;
            match result {
                Ok(Some(raw)) if raw.arrival_time <= session.clock().cursor() => {
                    self.target = request.target;
                    self.metadata = Some(InspectionMetadata {
                        stream_id: raw.stream_id,
                        log_time: raw.arrival_time,
                        publish_time_ns: raw.publish_time_ns,
                        sequence: raw.sequence,
                        payload_bytes: raw.payload.len(),
                    });
                    self.status = format!(
                        "Log: {} ns · {} bytes",
                        raw.arrival_time.0,
                        raw.payload.len()
                    );
                    if !self
                        .message
                        .as_ref()
                        .is_some_and(|message| message.matches_raw(&raw))
                    {
                        if self
                            .schema
                            .as_ref()
                            .is_none_or(|(stream, _)| *stream != raw.stream_id)
                        {
                            self.schema = session.catalog().by_id(raw.stream_id).map(|stream| {
                                (
                                    raw.stream_id,
                                    InspectorSchema::for_stream(stream).map_err(|e| e.to_string()),
                                )
                            });
                        }
                        let decoded = self
                            .schema
                            .as_ref()
                            .ok_or_else(|| "missing schema".to_owned())
                            .and_then(|(_, schema)| schema.as_ref().map_err(Clone::clone))
                            .and_then(|schema| {
                                InspectorMessage::new(schema, &raw).map_err(|e| e.to_string())
                            });
                        match decoded {
                            Ok(message) => self.message = Some(Arc::new(message)),
                            Err(error) => {
                                self.message = None;
                                self.metadata = None;
                                self.error = Some(error.clone());
                                self.status = error;
                            }
                        }
                    }
                }
                Ok(None) => {
                    self.message = None;
                    self.metadata = None;
                    self.status = "No message at or before this cursor".into();
                }
                Ok(Some(_)) => {}
                Err(error) => {
                    self.message = None;
                    self.metadata = None;
                    self.error = Some(error.clone());
                    self.status = error;
                }
            }
        }
        self.query.set_automatic(self.automatic);
        if admit && let Some(request) = self.query.next(session.clock().cursor()) {
            let result = inspection_plan(session.catalog(), &request.topic, request.target)
                .and_then(|plan| {
                    self.loader
                        .as_mut()
                        .ok_or("no inspector loader".to_owned())?
                        .request(plan)
                        .map_err(|e| e.to_string())
                });
            if let Err(error) = result {
                self.query.complete(&request);
                self.target = request.target;
                self.message = None;
                self.metadata = None;
                self.error = Some(error.clone());
                self.status = error;
            }
        }
    }
    pub fn refresh(&mut self, session: &WebPlayback) {
        self.query.refresh(session.clock().cursor());
    }
    pub fn loading(&self) -> bool {
        self.query.waiting()
    }
    pub fn view(&self) -> Option<InspectionView<'_>> {
        self.selected.as_deref().map(|topic| InspectionView {
            topic,
            target: self.target,
            metadata: self.metadata,
            message: self.message.as_ref(),
            error: self.error.as_ref().map(|e| e as &dyn std::fmt::Display),
            loading: self.loading(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{RemoteApiClient, RemoteCatalog};
    use viewer_core::{
        ArrivalTime, PlaybackCommand, PlaybackRequirements, RecordingTimeRange, SchemaDefinition,
        SessionPlan, SourceCapabilities, SourceCatalog, StreamDescriptor, StreamTimingSummary,
    };
    fn session() -> WebPlayback {
        let catalog = SourceCatalog {
            streams: vec![StreamDescriptor {
                id: StreamId(7),
                topic: "/scalar".into(),
                schema: "demo/msg/Scalar".into(),
                message_encoding: "cdr".into(),
                schema_definition: Some(Arc::new(SchemaDefinition {
                    encoding: "ros2msg".into(),
                    data: bytes::Bytes::from_static(b"uint32 value\n"),
                })),
                timing: StreamTimingSummary::default(),
            }],
            time_range: RecordingTimeRange::new(ArrivalTime(0), ArrivalTime(101)),
            capabilities: SourceCapabilities::INDEXED_RECORDING,
        };
        let plan = SessionPlan::build(
            &catalog,
            "",
            &PlaybackRequirements::empty(),
            &crate::workspace_bindings::bundled(),
        )
        .unwrap();
        WebPlayback::from_remote(
            RemoteApiClient::new("http://localhost").unwrap(),
            RemoteCatalog {
                core: catalog,
                recording_id: "test".into(),
                revision: "one".into(),
                plan,
                selected_streams: vec![],
            },
        )
        .unwrap()
    }
    fn raw(time: i64) -> viewer_core::RawMessage {
        viewer_core::RawMessage {
            stream_id: StreamId(7),
            arrival_time: ArrivalTime(time),
            publish_time_ns: None,
            sequence: Some(time as u32),
            payload: bytes::Bytes::from(
                [vec![0, 1, 0, 0], (time as u32).to_le_bytes().to_vec()].concat(),
            ),
        }
    }
    fn remote(inspection: &mut Inspection) -> &mut crate::remote::RemoteRestoreLoader {
        match inspection.loader.as_mut().unwrap() {
            WebRestoreLoader::Remote(loader) => loader,
            #[cfg(target_arch = "wasm32")]
            WebRestoreLoader::LocalFile(_) => panic!("expected fake remote"),
        }
    }
    #[test]
    fn seek_with_automatic_off_clears_snapshot_rejects_delays_and_completes_exactly() {
        let mut session = session();
        let mut inspection = Inspection::default();
        inspection.reset(&session);
        inspection.select("/scalar".into());
        inspection.poll(&session, true);
        remote(&mut inspection).inject(vec![raw(0)]);
        inspection.poll(&session, false);
        assert!(inspection.message.is_some());
        inspection.automatic = false;
        session
            .apply_command(PlaybackCommand::Seek(ArrivalTime(20)))
            .unwrap();
        assert_eq!(
            session.tick(std::time::Duration::ZERO).unwrap(),
            viewer_core::PlaybackEffect::Seeked
        );
        inspection.settle(&session);
        inspection.poll(&session, false); // Waiting for bounded admission still means Loading.
        assert!(inspection.loading());
        assert!(!inspection.in_flight());
        assert!(inspection.view().unwrap().message.is_none());
        inspection.poll(&session, true);
        assert_eq!(inspection.query.active().unwrap().target, ArrivalTime(20));
        remote(&mut inspection).inject_stale(vec![raw(0)]);
        inspection.poll(&session, false);
        assert!(inspection.message.is_none());
        assert!(inspection.loading());
        remote(&mut inspection).inject(vec![raw(20)]);
        inspection.poll(&session, false);
        assert!(!inspection.loading());
        assert_eq!(inspection.metadata.unwrap().log_time, ArrivalTime(20));
        assert_eq!(inspection.view().unwrap().target, ArrivalTime(20));
        inspection.poll(&session, true);
        assert!(!inspection.loading());
        assert_eq!(session.clock().cursor(), ArrivalTime(20));
    }
    #[test]
    fn pause_supersedes_inflight_follow_even_at_same_time_and_reports_failure() {
        let mut session = session();
        session.apply_command(PlaybackCommand::Toggle).unwrap();
        let mut inspection = Inspection::default();
        inspection.reset(&session);
        inspection.select("/scalar".into());
        inspection.poll(&session, true);
        let old = inspection.query.active().unwrap().clone();
        inspection.automatic = false;
        session.apply_command(PlaybackCommand::Toggle).unwrap();
        inspection.poll(&session, true);
        assert_ne!(inspection.query.active(), Some(&old));
        assert!(inspection.message.is_none());
        remote(&mut inspection).inject_stale(vec![raw(0)]);
        inspection.poll(&session, false);
        assert!(inspection.loading());
        remote(&mut inspection).inject_error("read failed");
        inspection.poll(&session, false);
        assert!(!inspection.loading());
        assert!(inspection.error.as_ref().unwrap().contains("read failed"));
        assert!(inspection.view().unwrap().message.is_none());
        assert!(inspection.metadata.is_none());
        assert_eq!(session.clock().cursor(), ArrivalTime(0));
        assert!(!session.clock().is_playing());
        inspection.refresh(&session);
        inspection.poll(&session, true);
        assert!(inspection.loading());
        inspection.selected = None;
        inspection.invalidate();
        inspection.poll(&session, true);
        assert!(!inspection.loading());
        assert!(inspection.view().is_none());
    }
}
