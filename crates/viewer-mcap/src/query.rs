//! Concrete bounded recording queries, independent of continuous playback features.
mod timestamp_window;
mod topic_health;
pub use topic_health::{TopicHealthRequirement, TopicHealthSnapshot, query_topic_health};

use crate::McapSourceError;

use viewer_core::{ArrivalTime, DataWindowTimeRange};

pub const DEFAULT_QUERY_WINDOW_SECONDS: u32 = 10;
pub const MAX_QUERY_WINDOW_SECONDS: u32 = 60;

#[derive(Debug)]
pub enum WindowQueryError {
    InvalidWindow,
    MissingTopic(String),
    UnsupportedDiagnosticStream(viewer_core::StreamId),
    Limit {
        resource: &'static str,
        maximum: usize,
    },
    Source(McapSourceError),
}
impl std::fmt::Display for WindowQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidWindow => write!(
                f,
                "query window must be 1..={MAX_QUERY_WINDOW_SECONDS} seconds with a representable exclusive end"
            ),
            Self::MissingTopic(topic) => write!(f, "recording has no topic {topic}"),
            Self::UnsupportedDiagnosticStream(stream) => write!(
                f,
                "stream {} must contain CDR diagnostic_msgs/msg/DiagnosticArray",
                stream.0
            ),
            Self::Limit { resource, maximum } => {
                write!(f, "query exceeds {resource} limit ({maximum})")
            }
            Self::Source(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for WindowQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            _ => None,
        }
    }
}
impl From<McapSourceError> for WindowQueryError {
    fn from(value: McapSourceError) -> Self {
        Self::Source(value)
    }
}

/// Query windows include the cursor: [cursor - lookback, cursor + 1 ns).
pub(crate) fn cursor_window(
    cursor: ArrivalTime,
    seconds: u32,
) -> Result<DataWindowTimeRange, WindowQueryError> {
    if seconds == 0 || seconds > MAX_QUERY_WINDOW_SECONDS {
        return Err(WindowQueryError::InvalidWindow);
    }
    let end = cursor
        .0
        .checked_add(1)
        .ok_or(WindowQueryError::InvalidWindow)?;
    DataWindowTimeRange::new(
        ArrivalTime(cursor.0.saturating_sub(i64::from(seconds) * 1_000_000_000)),
        ArrivalTime(end),
    )
    .map_err(|_| WindowQueryError::InvalidWindow)
}

mod diagnostics;
pub use diagnostics::{DiagnosticsRequirement, query_diagnostics};

impl From<viewer_core::DiagnosticsReduceError> for WindowQueryError {
    fn from(value: viewer_core::DiagnosticsReduceError) -> Self {
        match value {
            viewer_core::DiagnosticsReduceError::Limit { resource, maximum } => {
                Self::Limit { resource, maximum }
            }
        }
    }
}

mod numeric_plot;
mod odometry_signals;
pub use numeric_plot::{
    NumericOverviewPlan, NumericPlotDetail, NumericPlotError, NumericPlotOverview,
    NumericPlotOverviewBatch, NumericSample, NumericSeriesRequest,
};
pub use odometry_signals::{
    load_odometry_signals, load_odometry_signals_for_topic_with_progress, load_speed_signal,
    load_yaw_rate_signal,
};
