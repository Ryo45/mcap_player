use viewer_core::ArrivalTime;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InspectorRequirement {
    pub(crate) topic: String,
    pub(crate) max_messages: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InspectedMessage {
    pub(crate) arrival_time: ArrivalTime,
    pub(crate) payload_bytes: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct TopicInspection {
    pub(crate) topic: String,
    pub(crate) messages: Vec<InspectedMessage>,
    pub(crate) error: Option<std::sync::Arc<crate::query_error::QueryLoadError>>,
    pub(crate) loading: bool,
}

impl TopicInspection {
    pub(crate) fn loading(topic: String) -> Self {
        Self {
            topic,
            messages: Vec::new(),
            error: None,
            loading: true,
        }
    }

    pub(crate) fn ready(topic: String, messages: Vec<InspectedMessage>) -> Self {
        Self {
            topic,
            messages,
            error: None,
            loading: false,
        }
    }

    pub(crate) fn failed(
        topic: String,
        error: std::sync::Arc<crate::query_error::QueryLoadError>,
    ) -> Self {
        Self {
            topic,
            messages: Vec::new(),
            error: Some(error),
            loading: false,
        }
    }
}
