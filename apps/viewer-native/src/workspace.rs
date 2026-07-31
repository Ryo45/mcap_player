use crate::interaction::ViewerAction;
use egui_plot::PlotPoint;
use viewer_core::{ArrivalTime, CameraId, PlaybackCommand, PlaybackView, PlotPanelState};

#[derive(Default)]
pub(crate) struct WorkspaceState {
    pub(crate) camera: CameraViewState,
    pub(crate) plot: PlotViewState,
    pub(crate) scene: SceneViewState,
    pub(crate) interaction: ViewerInteractionState,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CameraViewState {
    pub(crate) focused_camera: Option<CameraId>,
}

#[derive(Default)]
pub(crate) struct PlotViewState {
    pub(crate) panel: Option<PlotPanelState>,
    pub(crate) cache: Option<SpeedPlotCache>,
}

pub(crate) struct SpeedPlotCache {
    pub(crate) origin: ArrivalTime,
    pub(crate) display_len: usize,
    pub(crate) points: Vec<PlotPoint>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SceneViewState {
    pub(crate) accumulate_points: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ViewerInteractionState {
    pub(crate) preview_time: Option<ArrivalTime>,
}

impl ViewerInteractionState {
    pub(crate) fn display_time(&self, playback: PlaybackView) -> ArrivalTime {
        self.preview_time.unwrap_or(playback.cursor)
    }
}

impl WorkspaceState {
    pub(crate) fn reset_for_source(&mut self, focused_camera: Option<CameraId>) {
        self.camera.focused_camera = focused_camera;
        self.plot = PlotViewState::default();
        self.interaction = ViewerInteractionState::default();
    }

    /// Applies non-Playback state transitions and returns Playback work for the App boundary.
    pub(crate) fn apply_action(&mut self, action: ViewerAction) -> Option<PlaybackCommand> {
        match action {
            ViewerAction::Playback(command) => Some(command),
            ViewerAction::SetFocusedCamera(camera_id) => {
                self.camera.focused_camera = camera_id;
                None
            }
            ViewerAction::SetAccumulatePoints(accumulate) => {
                self.scene.accumulate_points = accumulate;
                None
            }
            ViewerAction::SetPreviewTime(preview_time) => {
                self.interaction.preview_time = preview_time;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{PlaybackSpeed, PlaybackView};

    fn playback(cursor: i64) -> PlaybackView {
        PlaybackView {
            start: ArrivalTime(0),
            end: ArrivalTime(100),
            cursor: ArrivalTime(cursor),
            playing: false,
            speed: PlaybackSpeed::Normal,
        }
    }

    #[test]
    fn defaults_to_no_selection_or_preview() {
        let workspace = WorkspaceState::default();
        assert_eq!(workspace.camera.focused_camera, None);
        assert!(!workspace.scene.accumulate_points);
        assert!(workspace.plot.panel.is_none());
        assert!(workspace.plot.cache.is_none());
        assert_eq!(workspace.interaction.preview_time, None);
    }

    #[test]
    fn actions_update_only_their_owned_state() {
        let mut workspace = WorkspaceState::default();
        workspace.apply_action(ViewerAction::SetFocusedCamera(Some(CameraId(3))));
        assert_eq!(workspace.camera.focused_camera, Some(CameraId(3)));
        assert!(!workspace.scene.accumulate_points);
        assert_eq!(workspace.interaction.preview_time, None);

        workspace.apply_action(ViewerAction::SetAccumulatePoints(true));
        assert_eq!(workspace.camera.focused_camera, Some(CameraId(3)));
        assert!(workspace.scene.accumulate_points);
        assert_eq!(workspace.interaction.preview_time, None);

        assert_eq!(
            workspace.apply_action(ViewerAction::SetPreviewTime(Some(ArrivalTime(42)))),
            None
        );
        assert_eq!(workspace.camera.focused_camera, Some(CameraId(3)));
        assert!(workspace.scene.accumulate_points);
        assert_eq!(workspace.interaction.preview_time, Some(ArrivalTime(42)));
        workspace.apply_action(ViewerAction::SetPreviewTime(None));
        assert_eq!(workspace.interaction.preview_time, None);
    }

    #[test]
    fn playback_action_is_forwarded_without_mutating_workspace() {
        let mut workspace = WorkspaceState::default();
        let command = PlaybackCommand::SetSpeed(PlaybackSpeed::Double);
        assert_eq!(
            workspace.apply_action(ViewerAction::Playback(command)),
            Some(command)
        );
        assert_eq!(workspace.camera, CameraViewState::default());
        assert_eq!(workspace.scene, SceneViewState::default());
        assert_eq!(workspace.interaction, ViewerInteractionState::default());
    }

    #[test]
    fn display_time_prefers_preview_and_falls_back_to_cursor() {
        let mut interaction = ViewerInteractionState::default();
        assert_eq!(interaction.display_time(playback(25)), ArrivalTime(25));
        interaction.preview_time = Some(ArrivalTime(70));
        assert_eq!(interaction.display_time(playback(25)), ArrivalTime(70));
        interaction.preview_time = None;
        assert_eq!(interaction.display_time(playback(25)), ArrivalTime(25));
    }
}
