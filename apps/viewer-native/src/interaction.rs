use viewer_core::{ArrivalTime, CameraId, PlaybackCommand};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ViewerAction {
    Playback(PlaybackCommand),
    SetFocusedCamera(Option<CameraId>),
    SetAccumulatePoints(bool),
    // Reserved for panel hover/preview wiring; no current fixed view emits it.
    #[allow(dead_code)]
    SetPreviewTime(Option<ArrivalTime>),
}
