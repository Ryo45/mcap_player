//! Query worker failures remain typed until the view formats them.
#[derive(Debug)]
pub(crate) enum QueryLoadError {
    Source(viewer_mcap::McapOpenError),
    Numeric(viewer_mcap::NumericPlotError),
    Inspection(viewer_mcap::RangeQueryError),
    StartWorker {
        worker: &'static str,
        source: std::io::Error,
    },
    WorkerPanicked {
        worker: &'static str,
    },
}
impl std::fmt::Display for QueryLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source(error) => write!(f, "recording query: {error}"),
            Self::Numeric(error) => error.fmt(f),
            Self::Inspection(error) => error.fmt(f),
            Self::StartWorker { worker, source } => write!(f, "start {worker} worker: {source}"),
            Self::WorkerPanicked { worker } => write!(f, "{worker} worker panicked"),
        }
    }
}
impl std::error::Error for QueryLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            Self::Numeric(error) => Some(error),
            Self::Inspection(error) => Some(error),
            Self::StartWorker { source, .. } => Some(source),
            _ => None,
        }
    }
}
impl From<viewer_mcap::McapOpenError> for QueryLoadError {
    fn from(error: viewer_mcap::McapOpenError) -> Self {
        Self::Source(error)
    }
}
impl From<viewer_mcap::NumericPlotError> for QueryLoadError {
    fn from(error: viewer_mcap::NumericPlotError) -> Self {
        Self::Numeric(error)
    }
}
impl From<viewer_mcap::RangeQueryError> for QueryLoadError {
    fn from(error: viewer_mcap::RangeQueryError) -> Self {
        Self::Inspection(error)
    }
}
