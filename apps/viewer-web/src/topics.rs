//! Catalog browsing and an independent latest-before Inspector query.
#[cfg(target_arch = "wasm32")]
use crate::playback::{WebPlayback, WebRestoreLoader};
#[cfg(target_arch = "wasm32")]
use std::collections::BTreeMap;
use viewer_core::{ArrivalTime, RawMessage, SourceCatalog, StreamId};
#[cfg(target_arch = "wasm32")]
use viewer_core::{InspectorMessage, InspectorSchema};

const MAX_TOPIC_STREAMS: usize = 64;

fn inspection_plan(
    catalog: &SourceCatalog,
    topic: &str,
    target: ArrivalTime,
) -> Result<viewer_core::RestorePlan, String> {
    let streams = catalog
        .streams
        .iter()
        .filter(|stream| stream.topic == topic)
        .collect::<Vec<_>>();
    if streams.is_empty() {
        return Err("Select a topic first".into());
    }
    if streams.len() > MAX_TOPIC_STREAMS {
        return Err("Topic exceeds the 64-channel inspection budget".into());
    }
    viewer_core::RestorePlanner::new(catalog)
        .plan(
            target,
            streams.into_iter().map(|stream| viewer_core::RestoreInput {
                stream_id: stream.id,
                semantics: viewer_core::RestoreSemantics::LatestBefore,
            }),
        )
        .map_err(|error| error.to_string())
}

fn latest_topic_message(
    catalog: &SourceCatalog,
    topic: &str,
    target: ArrivalTime,
    messages: Vec<RawMessage>,
) -> Result<Option<RawMessage>, String> {
    let mut seen = std::collections::BTreeSet::new();
    for message in &messages {
        if message.arrival_time > target
            || catalog
                .by_id(message.stream_id)
                .is_none_or(|stream| stream.topic != topic)
            || !seen.insert(message.stream_id)
        {
            return Err("Invalid latest-before inspection response".into());
        }
    }
    Ok(messages
        .into_iter()
        .max_by_key(|message| (message.arrival_time, message.stream_id)))
}

#[cfg(target_arch = "wasm32")]
mod dom {
    use super::*;
    use std::rc::Rc;
    use viewer_core::{InspectorContainer, InspectorPageCursor, InspectorScalar, InspectorValue};
    use wasm_bindgen::JsCast;
    use web_sys::{HtmlButtonElement, HtmlElement};

    fn element(id: &str) -> HtmlElement {
        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .get_element_by_id(id)
            .unwrap()
            .dyn_into()
            .unwrap()
    }
    fn node(tag: &str, text: &str) -> HtmlElement {
        let node: HtmlElement = web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .create_element(tag)
            .unwrap()
            .dyn_into()
            .unwrap();
        node.set_inner_text(text);
        node
    }
    struct ContainerPage {
        container: InspectorContainer,
        next: Option<InspectorPageCursor>,
        node: HtmlElement,
    }

    #[derive(Default)]
    pub(crate) struct TopicBrowser {
        topics: BTreeMap<String, Vec<StreamId>>,
        pub(crate) selected: Option<String>,
        loader: Option<WebRestoreLoader>,
        query: crate::inspector_query::InspectorQueryState,
        origin: Option<web_time::Instant>,
        schema: Option<(StreamId, Result<InspectorSchema, String>)>,
        snapshot_info: String,
        message: Option<Rc<InspectorMessage>>,
        pages: Vec<ContainerPage>,
        fields: usize,
        rendered: bool,
    }

    impl TopicBrowser {
        pub(crate) fn reset(&mut self, session: &WebPlayback) {
            self.clear_message();
            self.topics.clear();
            for stream in &session.catalog().streams {
                self.topics
                    .entry(stream.topic.clone())
                    .or_default()
                    .push(stream.id);
            }
            self.selected = None;
            self.query.select(None);
            self.schema = None;
            element("topic-info")
                .set_inner_text("Select a topic to see its schema and message count.");
            element("inspect-message")
                .dyn_into::<HtmlButtonElement>()
                .unwrap()
                .set_disabled(true);
            self.loader = Some(session.inspector_loader());
            self.rendered = false;
        }

        pub(crate) fn render_topics(&mut self, filter: &str) {
            let list = element("topic-list");
            list.set_inner_text("");
            for topic in self
                .topics
                .keys()
                .filter(|topic| topic.to_lowercase().contains(&filter.to_lowercase()))
            {
                let button = node("button", topic);
                button.set_attribute("data-topic", topic).unwrap();
                button.set_class_name(if self.selected.as_ref() == Some(topic) {
                    "topic-row selected"
                } else {
                    "topic-row"
                });
                list.append_child(&button).unwrap();
            }
            self.rendered = true;
        }

        pub(crate) fn select(&mut self, topic: String, session: &WebPlayback) {
            self.clear_message();
            let info = session
                .catalog()
                .streams
                .iter()
                .filter(|stream| stream.topic == topic)
                .map(|stream| {
                    format!(
                        "Channel {} · {}\n{} · {} messages",
                        stream.id.0,
                        stream.schema,
                        stream.message_encoding,
                        stream
                            .timing
                            .message_count
                            .map_or_else(|| "unknown".into(), |count| count.to_string())
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            element("topic-info").set_inner_text(&format!("{topic}\n\n{info}"));
            self.query.select(Some(topic.clone()));
            self.schema = None;
            self.selected = Some(topic);
            self.rendered = false;
            element("message-info").set_inner_text(
                "Following the committed cursor. Use Refresh when automatic updates are off.",
            );
            element("inspect-message")
                .dyn_into::<HtmlButtonElement>()
                .unwrap()
                .set_disabled(false);
        }

        pub(crate) fn set_automatic(&mut self, automatic: bool) {
            self.query.set_automatic(automatic);
        }

        pub(crate) fn request(&mut self, session: &WebPlayback) {
            self.query.refresh(session.clock().cursor());
        }

        fn clear_fields(&mut self) {
            self.message = None;
            self.pages.clear();
            self.fields = 0;
            element("message-fields").set_inner_text("");
        }

        pub(crate) fn poll(&mut self, session: &WebPlayback) {
            if !self.rendered {
                let filter: web_sys::HtmlInputElement = element("topic-filter").dyn_into().unwrap();
                self.render_topics(&filter.value());
            }
            if let Some(result) = self.loader.as_mut().and_then(WebRestoreLoader::poll)
                && let Some(request) = self.query.active().cloned()
                && self.query.complete(&request)
            {
                self.publish(
                    session,
                    &request.topic,
                    request.target,
                    result.map_err(|e| e.to_string()),
                );
            }
            let now = self
                .origin
                .get_or_insert_with(web_time::Instant::now)
                .elapsed()
                .as_millis() as u64;
            if let Some(request) = self.query.next(session.clock().cursor(), now) {
                let result = inspection_plan(session.catalog(), &request.topic, request.target)
                    .and_then(|plan| {
                        self.loader
                            .as_mut()
                            .unwrap()
                            .request(plan)
                            .map_err(|e| e.to_string())
                    });
                if let Err(error) = result {
                    self.query.complete(&request);
                    self.snapshot_info = error;
                }
            }
            if self.selected.is_some() {
                let loading = self.query.active().map_or(String::new(), |r| {
                    format!("\nLoading at {} ns…", r.target.0)
                });
                element("message-info").set_inner_text(&format!(
                    "{}\nCurrent cursor: {} ns{}",
                    self.snapshot_info,
                    session.clock().cursor().0,
                    loading
                ));
            }
        }

        fn publish(
            &mut self,
            session: &WebPlayback,
            topic: &str,
            target: ArrivalTime,
            result: Result<Vec<RawMessage>, String>,
        ) {
            let raw = match result.and_then(|messages| {
                latest_topic_message(session.catalog(), topic, target, messages)
            }) {
                Ok(Some(raw)) if raw.arrival_time <= session.clock().cursor() => raw,
                Ok(Some(_)) => return,
                Ok(None) => {
                    self.clear_fields();
                    self.snapshot_info = format!(
                        "Query cursor: {} ns\nNo message at or before the cursor",
                        target.0
                    );
                    return;
                }
                Err(error) => {
                    // Retain the previous completed snapshot and its metadata while reporting failure.
                    self.snapshot_info
                        .push_str(&format!("\nRefresh failed: {error}"));
                    // Keep the retained status bounded even across repeated errors.
                    if self.snapshot_info.len() > 4096 {
                        self.snapshot_info = error;
                    }
                    return;
                }
            };
            self.clear_fields();
            self.snapshot_info = format!(
                "Query cursor: {} ns\nLog time: {} ns\nPublish time: {}\nSequence: {}\nPayload: {} bytes",
                target.0,
                raw.arrival_time.0,
                raw.publish_time_ns
                    .map_or_else(|| "unavailable".into(), |time| format!("{time} ns")),
                raw.sequence
                    .map_or_else(|| "unavailable".into(), |sequence| sequence.to_string()),
                raw.payload.len()
            );
            if self
                .schema
                .as_ref()
                .is_none_or(|(stream, _)| *stream != raw.stream_id)
            {
                let descriptor = session.catalog().by_id(raw.stream_id).unwrap();
                self.schema = Some((
                    raw.stream_id,
                    InspectorSchema::for_stream(descriptor).map_err(|e| e.to_string()),
                ));
            }
            let decoded = self
                .schema
                .as_ref()
                .unwrap()
                .1
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|schema| InspectorMessage::new(schema, &raw).map_err(|e| e.to_string()));
            match decoded {
                Ok(message) => {
                    let root = message.root.clone();
                    self.message = Some(Rc::new(message));
                    self.pages.push(ContainerPage {
                        container: root,
                        next: None,
                        node: element("message-fields"),
                    });
                    self.show_page(0, false);
                }
                Err(error) => element("message-fields").set_inner_text(&error),
            }
        }

        pub(crate) fn clear_message(&mut self) {
            if let Some(loader) = &mut self.loader {
                loader.cancel();
            }
            self.query.invalidate();
            self.clear_fields();
            self.snapshot_info = "Waiting for a message at the committed cursor.".into();
            element("message-info").set_inner_text(&self.snapshot_info);
            element("inspect-message")
                .dyn_into::<HtmlButtonElement>()
                .unwrap()
                .set_disabled(self.selected.is_none());
        }

        pub(crate) fn show_page(&mut self, index: usize, next: bool) {
            let Some(slot) = self.pages.get(index) else {
                return;
            };
            let cursor = if next { slot.next } else { None };
            let container = slot.container.clone();
            let output = slot.node.clone();
            let Some(message) = &self.message else {
                return;
            };
            let page = match message.page(&container, cursor) {
                Ok(page) => page,
                Err(error) => {
                    output.set_inner_text(&error.to_string());
                    return;
                }
            };
            output.set_inner_text("");
            self.pages[index].next = page.next;
            for field in page.fields {
                if self.fields >= 512 {
                    output
                        .append_child(&node(
                            "p",
                            "Field budget reached. Read the message again to reset.",
                        ))
                        .unwrap();
                    break;
                }
                self.fields += 1;
                match field.value {
                    InspectorValue::Scalar(value) => {
                        let value = match value {
                            InspectorScalar::Bool(value) => value.to_string(),
                            InspectorScalar::Signed(value) => value.to_string(),
                            InspectorScalar::Unsigned(value) => value.to_string(),
                            InspectorScalar::Float(value) => value.to_string(),
                            InspectorScalar::Text {
                                preview,
                                bytes,
                                truncated,
                            } => format!(
                                "{preview:?}{} ({bytes} bytes)",
                                if truncated { "…" } else { "" }
                            ),
                        };
                        output
                            .append_child(&node("div", &format!("{}: {value}", field.name)))
                            .unwrap();
                    }
                    InspectorValue::Container(child) => {
                        if self.pages.len() >= 32 {
                            output
                                .append_child(&node("p", "Container budget reached"))
                                .unwrap();
                            continue;
                        }
                        let details = node("details", "");
                        details
                            .append_child(&node(
                                "summary",
                                &format!(
                                    "{}: {} ({} fields/items)",
                                    field.name,
                                    field.ros_type,
                                    child.len()
                                ),
                            ))
                            .unwrap();
                        let body = node("div", "");
                        let button = node("button", "Expand");
                        button
                            .set_attribute("data-expand", &self.pages.len().to_string())
                            .unwrap();
                        body.append_child(&button).unwrap();
                        details.append_child(&body).unwrap();
                        output.append_child(&details).unwrap();
                        self.pages.push(ContainerPage {
                            container: child,
                            next: None,
                            node: body,
                        });
                    }
                }
            }
            if page.next.is_some() {
                let button = node("button", "Next page");
                button
                    .set_attribute("data-next", &index.to_string())
                    .unwrap();
                output.append_child(&button).unwrap();
            }
        }
    }
}
#[cfg(target_arch = "wasm32")]
pub(crate) use dom::TopicBrowser;

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{RecordingTimeRange, StreamDescriptor, StreamTimingSummary};
    fn catalog() -> SourceCatalog {
        SourceCatalog {
            time_range: Some(RecordingTimeRange {
                start: ArrivalTime(0),
                end_exclusive: ArrivalTime(100),
            }),
            capabilities: Default::default(),
            streams: [1, 2]
                .into_iter()
                .map(|id| StreamDescriptor {
                    id: StreamId(id),
                    topic: "/a".into(),
                    schema: String::new(),
                    schema_definition: None,
                    message_encoding: "cdr".into(),
                    timing: StreamTimingSummary {
                        message_count: Some(1),
                    },
                })
                .collect(),
        }
    }
    fn raw(stream: u32, time: i64) -> RawMessage {
        RawMessage {
            stream_id: StreamId(stream),
            arrival_time: ArrivalTime(time),
            publish_time_ns: Some(0),
            sequence: Some(0),
            payload: bytes::Bytes::new(),
        }
    }
    #[test]
    fn inspection_uses_all_topic_channels_without_playback_routes() {
        let catalog = catalog();
        let plan = inspection_plan(&catalog, "/a", ArrivalTime(50)).unwrap();
        assert_eq!(plan.latest_before, [StreamId(1), StreamId(2)]);
        assert!(plan.histories.is_empty() && plan.persistent.is_empty());
        assert!(inspection_plan(&catalog, "/missing", ArrivalTime(50)).is_err());
        assert!(inspection_plan(&catalog, "/a", ArrivalTime(100)).is_err());
    }
    #[test]
    fn exact_response_rejects_future_unknown_and_duplicate_messages_and_preserves_ties() {
        let catalog = catalog();
        assert_eq!(
            latest_topic_message(
                &catalog,
                "/a",
                ArrivalTime(50),
                vec![raw(1, 50), raw(2, 50)]
            )
            .unwrap()
            .unwrap()
            .stream_id,
            StreamId(2)
        );
        for messages in [
            vec![raw(1, 51)],
            vec![raw(3, 40)],
            vec![raw(1, 40), raw(1, 45)],
        ] {
            assert!(latest_topic_message(&catalog, "/a", ArrivalTime(50), messages).is_err());
        }
        assert!(
            latest_topic_message(&catalog, "/a", ArrivalTime(50), vec![])
                .unwrap()
                .is_none()
        );
    }
}
