//! Bounded or explicitly queried derived products kept outside continuous feature state.

mod bookmark;
mod plot;
mod preview;
mod source_fingerprint;

pub use bookmark::{
    Bookmark, BookmarkDocument, BookmarkId, BookmarkValidationError,
    CURRENT_BOOKMARK_SCHEMA_VERSION, PreviewBuildInfo, SourceFingerprint,
};
pub use plot::{
    LoadedOdometrySignals, LoadedSignal, PlotSeries, SignalId, SignalOverviewReducer, SignalSample,
    arrival_time_from_plot_x, cursor_seconds, load_odometry_signals,
    load_odometry_signals_for_topic_with_progress, load_speed_signal, load_yaw_rate_signal,
};
pub use preview::{
    CURRENT_PREVIEW_SCHEMA_VERSION, CameraPreviewFrame, DataFidelity, PreviewBudget,
    PreviewImageEncoding, PreviewRequest, PreviewSnapshot, PreviewValidationError, SignalBucket,
    SignalFidelity, SignalOverview, TimeRange, TimedPosition2, merge_signal_buckets,
};
pub use source_fingerprint::mcap_summary_fingerprint;
