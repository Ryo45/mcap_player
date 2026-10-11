//! Bounded or explicitly queried derived products kept outside continuous feature state.

mod bookmark;
mod plot;
mod preview;

pub use bookmark::{
    Bookmark, BookmarkDocument, BookmarkId, BookmarkValidationError,
    CURRENT_BOOKMARK_SCHEMA_VERSION, PreviewBuildInfo, SourceFingerprint,
};
pub use plot::{
    LoadedOdometrySignals, LoadedSignal, PlotSeries, SignalId, SignalOverviewReducer, SignalSample,
    arrival_time_from_plot_x, cursor_seconds,
};
pub use preview::{
    CURRENT_PREVIEW_SCHEMA_VERSION, CameraPreviewFrame, DataFidelity, PreviewBudget,
    PreviewImageEncoding, PreviewRequest, PreviewSnapshot, PreviewValidationError, SignalBucket,
    SignalFidelity, SignalOverview, TimeRange, TimedPosition2, merge_signal_buckets,
};
