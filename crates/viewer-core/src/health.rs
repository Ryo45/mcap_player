//! Source-independent timing facts and producer diagnostics; no recording I/O.
mod diagnostics;
mod topic_timing;
pub use diagnostics::{
    DiagnosticAggregator, DiagnosticEvent, DiagnosticsReduceError, DiagnosticsSnapshot,
};
pub use topic_timing::{TopicTimingFacts, summarize_topic_timing};
