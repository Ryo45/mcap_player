use viewer_core::{ArrivalTime, CameraId, PlaybackCommand};
use viewer_layout::PanelId;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ViewerAction {
    Playback(PlaybackCommand),
    SetFocusedCamera {
        panel_id: PanelId,
        camera_id: Option<CameraId>,
    },
    SetAccumulatePoints {
        panel_id: PanelId,
        accumulate: bool,
    },
    // Reserved for panel hover/preview wiring; no current fixed view emits it.
    #[allow(dead_code)]
    SetPreviewTime(Option<ArrivalTime>),
}
