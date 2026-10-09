use std::sync::Arc;
use viewer_core::{ArrivalTime, InspectorMessage, StreamId};
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InspectorRequirement {
    pub topic: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InspectionMetadata {
    pub stream_id: StreamId,
    pub log_time: ArrivalTime,
    pub publish_time_ns: Option<u64>,
    pub sequence: Option<u32>,
    pub payload_bytes: usize,
}
#[derive(Clone, Debug)]
pub(crate) struct TopicInspection {
    pub topic: String,
    pub target: ArrivalTime,
    pub metadata: Option<InspectionMetadata>,
    pub message: Option<Arc<InspectorMessage>>,
    pub error: Option<Arc<crate::inspector_loader::InspectorError>>,
    pub loading: bool,
}
impl TopicInspection {
    pub fn loading(topic: String, target: ArrivalTime) -> Self {
        Self {
            topic,
            target,
            metadata: None,
            message: None,
            error: None,
            loading: true,
        }
    }
}
