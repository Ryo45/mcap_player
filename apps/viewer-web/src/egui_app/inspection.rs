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
        self.query.invalidate();
        self.error = None;
        self.message = None;
        self.metadata = None;
    }
    pub fn poll(&mut self, session: &WebPlayback, admit: bool) {
        if let Some(result) = self.loader.as_mut().and_then(WebRestoreLoader::poll)
            && let Some(request) = self.query.active().cloned()
            && self.query.complete(&request)
        {
            let result = result.map_err(|e| e.to_string()).and_then(|messages| {
                latest_topic_message(session.catalog(), &request.topic, request.target, messages)
            });
            self.error = None;
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
        self.query.active().is_some()
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
