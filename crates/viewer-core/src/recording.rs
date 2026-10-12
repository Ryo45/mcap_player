//! Source-neutral recording facts, requirements, and restore planning.

mod catalog;
mod raw_message;
mod restore;
mod session_plan;

pub use raw_message::RawMessage;
pub use restore::{
    RestoreInput, RestorePlan, RestorePlanError, RestorePlanner, RestoreRead, RestoreSemantics,
};
pub use session_plan::{
    CameraRoute, PlaybackRequirements, SessionPlan, SessionPlanError, WorkspaceBindings,
};

pub use catalog::{
    RecordingTimeRange, SchemaDefinition, SourceCapabilities, SourceCatalog, StreamDescriptor,
    StreamId, StreamTimingSummary,
};

mod topic_tree;
pub use topic_tree::{SchemaExplorationError, TopicTree, TopicTreeEntry};
