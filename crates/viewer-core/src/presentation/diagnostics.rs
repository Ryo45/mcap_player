use crate::{PipelineCounters, PlaybackPerformance, PresentationSnapshot};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiagnosticsPresentation {
    pub source: String,
    pub primary_topic: String,
    pub counters: PipelineCounters,
    pub playback_performance: Option<PlaybackPerformance>,
    pub performance: PresentationSnapshot,
    pub path_points: usize,
    pub scan_points: usize,
    pub cursor_seconds: Option<f64>,
    pub error: Option<String>,
}
