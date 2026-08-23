#[cfg(test)]
use crate::panels::NativePanel;
use crate::{
    interaction::ViewerAction,
    panels::{PanelDataRequirements, PanelRuntimeStore},
};
use std::time::Duration;
use viewer_core::{
    ArrivalTime, CameraId, FeatureRestoreError, FeatureRuntime, PlaybackCommand,
    PlaybackPerformance, PlaybackView, ProcessingCounters, RawMessage, SessionPlan, StageTiming,
    WorkspaceBindings,
};
use viewer_layout::{
    CURRENT_LAYOUT_SCHEMA_VERSION, LayoutDocument, LayoutNode, PanelId, PanelNode,
};

const BUNDLED_DEFAULT_LAYOUT: &str = include_str!("../../../config/layouts/native_default.json");
const BUNDLED_SHOWCASE_LAYOUT: &str = include_str!("../../../config/layouts/native_showcase.json");
const BUNDLED_WORKSPACE_BINDINGS: &str = include_str!("../../../config/workspace_bindings.json");

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum WorkspaceLayout {
    #[default]
    Standard,
    Showcase,
}

impl WorkspaceLayout {
    fn bundled_json(self) -> &'static str {
        match self {
            Self::Standard => BUNDLED_DEFAULT_LAYOUT,
            Self::Showcase => BUNDLED_SHOWCASE_LAYOUT,
        }
    }
}

pub(crate) struct NativeWorkspace {
    pub(crate) layout: LayoutDocument,
    pub(crate) panels: PanelRuntimeStore,
    pub(crate) interaction: ViewerInteractionState,
    bindings: WorkspaceBindings,
    pub(crate) runtime: Option<FeatureRuntime>,
    startup_warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CameraViewState {
    pub(crate) focused_camera: Option<CameraId>,
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
    pub(crate) fn load_bundled_or_fallback(layout: WorkspaceLayout) -> Self {
        match Self::from_json(layout.bundled_json()) {
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
            bindings: bundled_workspace_bindings(),
            runtime: None,
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
            bindings: bundled_workspace_bindings(),
            runtime: None,
            startup_warnings: vec![format!(
                "Bundled layout could not be loaded; using emergency layout: {error}"
            )],
        }
    }

    pub(crate) fn bindings(&self) -> &WorkspaceBindings {
        &self.bindings
    }

    pub(crate) fn reset_for_source(&mut self, focused_camera: Option<CameraId>) {
        if let Some(runtime) = &mut self.runtime {
            runtime.set_scheduling_priority(focused_camera);
        }
        self.panels.reset_for_source(focused_camera);
        self.interaction = ViewerInteractionState::default();
    }

    pub(crate) fn configure_session(&mut self, plan: &SessionPlan) {
        self.runtime = Some(FeatureRuntime::new(plan, self.panels.has_scene()));
    }

    pub(crate) fn process_messages(&mut self, elapsed: Duration, messages: Vec<RawMessage>) {
        self.runtime_mut().process_messages(elapsed, &messages);
    }

    pub(crate) fn restore_messages(
        &mut self,
        target: ArrivalTime,
        messages: Vec<RawMessage>,
    ) -> Result<(), FeatureRestoreError> {
        self.runtime_mut().restore_transactional(target, &messages)
    }

    pub(crate) fn runtime(&self) -> &FeatureRuntime {
        self.runtime
            .as_ref()
            .expect("workspace is configured for an open session")
    }

    pub(crate) fn runtime_mut(&mut self) -> &mut FeatureRuntime {
        self.runtime
            .as_mut()
            .expect("workspace is configured for an open session")
    }

    pub(crate) fn cameras(&self) -> &viewer_core::CameraController {
        self.runtime().cameras()
    }

    pub(crate) fn path(&self) -> &viewer_core::PathController {
        self.runtime().path()
    }

    pub(crate) fn odometry(&self) -> &viewer_core::OdometryController {
        self.runtime().odometry()
    }

    pub(crate) fn transforms(&self) -> &viewer_core::TransformController {
        self.runtime().transforms()
    }

    pub(crate) fn counters(&self) -> ProcessingCounters {
        self.runtime().counters()
    }

    pub(crate) fn playback_performance(&self, source_read: StageTiming) -> PlaybackPerformance {
        self.runtime().playback_performance(source_read)
    }

    pub(crate) fn apply_action(&mut self, action: ViewerAction) -> WorkspaceEffect {
        match action {
            ViewerAction::Playback(command) => WorkspaceEffect::Playback(command),
            ViewerAction::SetFocusedCamera {
                panel_id,
                camera_id,
            } => {
                if self.panels.set_focused_camera(&panel_id, camera_id) {
                    if let Some(runtime) = &mut self.runtime {
                        runtime.set_scheduling_priority(camera_id);
                    }
                    WorkspaceEffect::None
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
        self.runtime
            .as_ref()
            .and_then(FeatureRuntime::scheduling_priority)
    }

    pub(crate) fn scheduler_priority_topic(&self) -> Option<&str> {
        self.panels.scheduler_priority_topic()
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

fn bundled_workspace_bindings() -> WorkspaceBindings {
    serde_json::from_str(BUNDLED_WORKSPACE_BINDINGS)
        .expect("bundled workspace bindings must be valid")
}

impl Default for NativeWorkspace {
    fn default() -> Self {
        Self::load_bundled_or_fallback(WorkspaceLayout::Standard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use viewer_core::{
        PlaybackRequirements, PlaybackSpeed, PlaybackView, SessionPlan, SignalId, SourceCatalog,
        StreamDescriptor, StreamId, StreamTimingSummary,
    };

    fn playback(cursor: i64) -> PlaybackView {
        PlaybackView {
            start: ArrivalTime(0),
            end: ArrivalTime(100),
            cursor: ArrivalTime(cursor),
            playing: false,
            speed: PlaybackSpeed::Normal,
        }
    }

    fn configure_camera_runtime(workspace: &mut NativeWorkspace, camera_count: u32) {
        let catalog = SourceCatalog {
            time_range: None,
            streams: (0..camera_count)
                .map(|index| StreamDescriptor {
                    id: StreamId(index + 1),
                    topic: if index == 0 {
                        "/camera/front/image/compressed".into()
                    } else {
                        format!("/camera/{index}/image/compressed")
                    },
                    schema: "sensor_msgs/msg/CompressedImage".into(),
                    message_encoding: "cdr".into(),
                    timing: StreamTimingSummary::default(),
                })
                .collect(),
            capabilities: Default::default(),
        };
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_all_cameras();
        let plan = SessionPlan::build(
            &catalog,
            "/camera/front/image/compressed",
            &requirements,
            workspace.bindings(),
        )
        .unwrap();
        workspace.configure_session(&plan);
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
        let mut expected_playback = viewer_core::PlaybackRequirements::empty();
        expected_playback.require_all_cameras();
        expected_playback.require_path();
        expected_playback.require_odometry();
        expected_playback.require_point_cloud();
        expected_playback.require_transforms();
        assert_eq!(
            workspace.data_requirements(),
            PanelDataRequirements {
                playback: expected_playback,
                signals: BTreeSet::from([SignalId::Speed]),
                inspections: Vec::new(),
            }
        );
    }

    #[test]
    fn bundled_showcase_is_selectable_without_changing_the_standard_default() {
        let workspace = NativeWorkspace::load_bundled_or_fallback(WorkspaceLayout::Showcase);
        assert!(workspace.startup_warning().is_none());
        assert_eq!(workspace.panels.len(), 6);
        assert_eq!(workspace.panels.placeholder_count(), 0);
        assert_eq!(workspace.panel("camera-left").kind_name(), "camera");
        assert_eq!(workspace.panel("camera-front").kind_name(), "camera");
        assert_eq!(workspace.panel("camera-right").kind_name(), "camera");
        assert_eq!(workspace.panel("speed-showcase").kind_name(), "plot");
        assert_eq!(workspace.panel("yaw-rate-showcase").kind_name(), "plot");
        assert_eq!(workspace.panel("status-showcase").kind_name(), "status");
        let mut expected_playback = viewer_core::PlaybackRequirements::empty();
        expected_playback.require_camera_topic("/camera/front_left/image/compressed");
        expected_playback.require_camera_topic("/camera/front/image/compressed");
        expected_playback.require_camera_topic("/camera/front_right/image/compressed");
        expected_playback.optional_path();
        expected_playback.require_odometry();
        expected_playback.optional_transforms();
        assert_eq!(
            workspace.data_requirements(),
            PanelDataRequirements {
                playback: expected_playback,
                signals: BTreeSet::from([SignalId::Speed, SignalId::YawRate]),
                inspections: Vec::new(),
            }
        );
        assert_eq!(NativeWorkspace::default().panels.len(), 4);
        assert_eq!(
            workspace.scheduler_priority_topic(),
            Some("/camera/front/image/compressed")
        );
    }

    #[test]
    fn fixed_showcase_camera_is_not_an_interactive_scheduler_focus_target() {
        let mut workspace = NativeWorkspace::load_bundled_or_fallback(WorkspaceLayout::Showcase);
        configure_camera_runtime(&mut workspace, 4);
        workspace.reset_for_source(Some(CameraId(0)));
        assert!(matches!(
            workspace.apply_action(ViewerAction::SetFocusedCamera {
                panel_id: PanelId::new("camera-left").unwrap(),
                camera_id: Some(CameraId(1)),
            }),
            WorkspaceEffect::None
        ));
        assert_eq!(workspace.focused_camera(), Some(CameraId(0)));
    }

    #[test]
    fn scoped_actions_update_only_the_target_panel_state() {
        let mut workspace = NativeWorkspace::default();
        configure_camera_runtime(&mut workspace, 4);
        let camera_id = PanelId::new("camera-main").unwrap();
        let scene_id = PanelId::new("scene-main").unwrap();
        assert!(matches!(
            workspace.apply_action(ViewerAction::SetFocusedCamera {
                panel_id: camera_id,
                camera_id: Some(CameraId(3)),
            }),
            WorkspaceEffect::None
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
