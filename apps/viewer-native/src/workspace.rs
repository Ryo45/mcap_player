#[cfg(test)]
use crate::panels::NativePanel;
use crate::{
    interaction::ViewerAction,
    panels::{PanelDataRequirements, PanelRuntimeStore},
};
use egui_plot::PlotPoint;
use viewer_core::{ArrivalTime, CameraId, PlaybackCommand, PlaybackView, PlotPanelState};
use viewer_layout::{
    CURRENT_LAYOUT_SCHEMA_VERSION, LayoutDocument, LayoutNode, PanelId, PanelNode,
};

const BUNDLED_DEFAULT_LAYOUT: &str = include_str!("../../../config/layouts/native_default.json");

pub(crate) struct NativeWorkspace {
    pub(crate) layout: LayoutDocument,
    pub(crate) panels: PanelRuntimeStore,
    pub(crate) interaction: ViewerInteractionState,
    scheduler_focused_camera: Option<CameraId>,
    startup_warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CameraViewState {
    pub(crate) focused_camera: Option<CameraId>,
}

#[derive(Default)]
pub(crate) struct PlotViewState {
    pub(crate) panel: Option<PlotPanelState>,
    pub(crate) cache: Option<SpeedPlotCache>,
    pub(crate) preview_cache: Option<PreviewPlotCache>,
}

pub(crate) struct SpeedPlotCache {
    pub(crate) origin: ArrivalTime,
    pub(crate) display_len: usize,
    pub(crate) points: Vec<PlotPoint>,
}

pub(crate) struct PreviewPlotCache {
    pub(crate) origin: ArrivalTime,
    pub(crate) first_bucket: Option<ArrivalTime>,
    pub(crate) bucket_len: usize,
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

pub(crate) enum WorkspaceEffect {
    None,
    Playback(PlaybackCommand),
    FocusedCameraChanged(Option<CameraId>),
    BeginPreview(ArrivalTime),
    UpdatePreview(Option<ArrivalTime>),
    CommitPreview(ArrivalTime),
}

impl ViewerInteractionState {
    pub(crate) fn display_time(&self, playback: PlaybackView) -> ArrivalTime {
        self.preview_time.unwrap_or(playback.cursor)
    }
}

impl NativeWorkspace {
    pub(crate) fn load_bundled_or_fallback() -> Self {
        match Self::from_json(BUNDLED_DEFAULT_LAYOUT) {
            Ok(workspace) => workspace,
            Err(error) => Self::emergency(error),
        }
    }

    pub(crate) fn from_json(json: &str) -> Result<Self, String> {
        let document = LayoutDocument::from_json(json).map_err(|error| error.to_string())?;
        Self::from_document(document)
    }

    pub(crate) fn from_document(document: LayoutDocument) -> Result<Self, String> {
        document.validate().map_err(|error| error.to_string())?;
        let runtime = PanelRuntimeStore::from_layout(&document);
        Ok(Self {
            layout: document,
            panels: runtime.store,
            interaction: ViewerInteractionState::default(),
            scheduler_focused_camera: None,
            startup_warnings: runtime.warnings,
        })
    }

    fn emergency(error: String) -> Self {
        let panel = PanelNode {
            id: PanelId::new("layout-error").expect("static emergency panel id is valid"),
            panel_type: "layout-error".to_owned(),
            config_version: 1,
            title: Some("Layout Error".to_owned()),
            config: serde_json::json!({ "error": error }),
        };
        let document = LayoutDocument {
            schema_version: CURRENT_LAYOUT_SCHEMA_VERSION,
            root: LayoutNode::Panel(panel),
        };
        let runtime = PanelRuntimeStore::from_layout(&document);
        Self {
            layout: document,
            panels: runtime.store,
            interaction: ViewerInteractionState::default(),
            scheduler_focused_camera: None,
            startup_warnings: vec![format!(
                "Bundled layout could not be loaded; using emergency layout: {error}"
            )],
        }
    }

    pub(crate) fn reset_for_source(&mut self, focused_camera: Option<CameraId>) {
        self.scheduler_focused_camera = focused_camera;
        self.panels.reset_for_source(focused_camera);
        self.interaction = ViewerInteractionState::default();
    }

    pub(crate) fn apply_action(&mut self, action: ViewerAction) -> WorkspaceEffect {
        match action {
            ViewerAction::Playback(command) => WorkspaceEffect::Playback(command),
            ViewerAction::SetFocusedCamera {
                panel_id,
                camera_id,
            } => {
                if self.panels.set_focused_camera(&panel_id, camera_id) {
                    self.scheduler_focused_camera = camera_id;
                    WorkspaceEffect::FocusedCameraChanged(camera_id)
                } else {
                    WorkspaceEffect::None
                }
            }
            ViewerAction::SetAccumulatePoints {
                panel_id,
                accumulate,
            } => {
                self.panels.set_accumulate_points(&panel_id, accumulate);
                WorkspaceEffect::None
            }
            ViewerAction::SetPreviewTime(preview_time) => {
                self.interaction.preview_time = preview_time;
                WorkspaceEffect::UpdatePreview(preview_time)
            }
            ViewerAction::BeginPreview(time) => {
                self.interaction.preview_time = Some(time);
                WorkspaceEffect::BeginPreview(time)
            }
            ViewerAction::CommitPreview(time) => {
                self.interaction.preview_time = None;
                WorkspaceEffect::CommitPreview(time)
            }
        }
    }

    pub(crate) fn focused_camera(&self) -> Option<CameraId> {
        self.scheduler_focused_camera
            .or_else(|| self.panels.first_focused_camera())
    }

    pub(crate) fn accumulate_points(&self) -> bool {
        self.panels.first_accumulate_points()
    }

    pub(crate) fn startup_warning(&self) -> Option<String> {
        (!self.startup_warnings.is_empty()).then(|| self.startup_warnings.join("; "))
    }

    pub(crate) fn data_requirements(&self) -> PanelDataRequirements {
        self.panels.data_requirements()
    }

    #[cfg(test)]
    fn panel(&self, id: &str) -> &NativePanel {
        self.panels
            .get(&PanelId::new(id).unwrap())
            .expect("panel runtime")
    }
}

impl Default for NativeWorkspace {
    fn default() -> Self {
        Self::load_bundled_or_fallback()
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
    fn bundled_workspace_has_four_typed_panels_without_placeholders() {
        let workspace = NativeWorkspace::default();
        assert!(workspace.startup_warning().is_none());
        assert_eq!(workspace.panels.len(), 4);
        assert_eq!(workspace.panels.placeholder_count(), 0);
        assert_eq!(workspace.panel("camera-main").kind_name(), "camera");
        assert_eq!(workspace.panel("bev-main").kind_name(), "bev");
        assert_eq!(workspace.panel("speed-main").kind_name(), "plot");
        assert_eq!(workspace.panel("scene-main").kind_name(), "scene-3d");
        assert_eq!(
            workspace.data_requirements(),
            PanelDataRequirements {
                vehicle_speed: true,
                inspections: Vec::new(),
            }
        );
    }

    #[test]
    fn scoped_actions_update_only_the_target_panel_state() {
        let mut workspace = NativeWorkspace::default();
        let camera_id = PanelId::new("camera-main").unwrap();
        let scene_id = PanelId::new("scene-main").unwrap();
        assert!(matches!(
            workspace.apply_action(ViewerAction::SetFocusedCamera {
                panel_id: camera_id,
                camera_id: Some(CameraId(3)),
            }),
            WorkspaceEffect::FocusedCameraChanged(Some(CameraId(3)))
        ));
        assert_eq!(workspace.focused_camera(), Some(CameraId(3)));
        assert!(!workspace.accumulate_points());

        assert!(matches!(
            workspace.apply_action(ViewerAction::SetAccumulatePoints {
                panel_id: scene_id,
                accumulate: true,
            }),
            WorkspaceEffect::None
        ));
        assert_eq!(workspace.focused_camera(), Some(CameraId(3)));
        assert!(workspace.accumulate_points());
        assert_eq!(workspace.interaction.preview_time, None);
    }

    #[test]
    fn playback_and_preview_keep_their_distinct_effects() {
        let mut workspace = NativeWorkspace::default();
        let command = PlaybackCommand::SetSpeed(PlaybackSpeed::Double);
        assert!(matches!(
            workspace.apply_action(ViewerAction::Playback(command)),
            WorkspaceEffect::Playback(forwarded) if forwarded == command
        ));

        assert!(matches!(
            workspace.apply_action(ViewerAction::SetPreviewTime(Some(ArrivalTime(42)))),
            WorkspaceEffect::UpdatePreview(Some(ArrivalTime(42)))
        ));
        assert_eq!(workspace.interaction.preview_time, Some(ArrivalTime(42)));
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

    #[test]
    fn invalid_bundled_equivalent_uses_a_visible_emergency_panel() {
        let workspace = NativeWorkspace::from_json("{invalid");
        assert!(workspace.is_err());
        let emergency = NativeWorkspace::emergency("invalid test layout".to_owned());
        assert_eq!(emergency.panels.len(), 1);
        assert_eq!(emergency.panels.placeholder_count(), 1);
        assert!(emergency.startup_warning().is_some());
    }
}
