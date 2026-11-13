use viewer_core::{ArrivalTime, PlaybackCommand};
use viewer_layout::PanelId;

#[derive(Clone, Debug, PartialEq)]
pub enum ViewerAction {
    Playback(PlaybackCommand),
    SelectInspectorTopic(Option<String>),
    AddNumericSeries {
        stream: viewer_core::StreamId,
        field_path: String,
    },
    RemoveNumericSeries {
        stream: viewer_core::StreamId,
        field_path: String,
    },
    SelectCameraTopic {
        panel_id: PanelId,
        topic: String,
    },
    SetAccumulatePoints {
        panel_id: PanelId,
        accumulate: bool,
    },
    SetPreviewTime(Option<ArrivalTime>),
    BeginPreview(ArrivalTime),
    CommitPreview(ArrivalTime),
}
