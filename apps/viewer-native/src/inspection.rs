use std::sync::Arc;
use viewer_core::{ArrivalTime, InspectionMetadata, InspectorMessage};
use viewer_ui::inspector::InspectionView;
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InspectorRequirement {
    pub topic: String,
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

    pub fn view(&self) -> InspectionView<'_> {
        InspectionView {
            topic: &self.topic,
            target: self.target,
            metadata: self.metadata,
            message: self.message.as_ref(),
            error: self.error.as_deref().map(|e| e as &dyn std::fmt::Display),
            loading: self.loading,
        }
    }
}
