#[cfg(test)]
use crate::panels::NativePanel;
use crate::panels::{CameraSelection, PanelDataRequirements};
use crate::{interaction::ViewerAction, panels::PanelRuntimeStore};
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
    Explore,
    Monitor,
    Health,
    Scene,
    Cameras,
    Navigation,
    Inspection,
}

impl WorkspaceLayout {
    fn bundled_json(self) -> &'static str {
        match self {
            Self::Scene => include_str!("../../../config/layouts/native_scene.json"),
            Self::Cameras => include_str!("../../../config/layouts/native_cameras.json"),
            Self::Navigation => include_str!("../../../config/layouts/native_navigation.json"),
            Self::Inspection => include_str!("../../../config/layouts/native_inspection.json"),
            Self::Standard => BUNDLED_DEFAULT_LAYOUT,
            Self::Showcase => BUNDLED_SHOWCASE_LAYOUT,
            Self::Monitor => include_str!("../../../config/layouts/native_monitor.json"),
            Self::Health => include_str!("../../../config/layouts/native_health.json"),
            Self::Explore => include_str!("../../../config/layouts/native_explore.json"),
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
    #[cfg(feature = "scene-extension")]
    scene_extension: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CameraViewState {
    pub(crate) focused_camera: Option<CameraId>,
    pub(crate) topic: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SceneViewState {
    pub(crate) coloring: viewer_core::PointCloudColoring,
    pub(crate) accumulate_points: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ViewerInteractionState {
    pub(crate) preview_time: Option<ArrivalTime>,
}

pub(crate) enum WorkspaceEffect {
    None,
    Playback(PlaybackCommand),
    SelectInspectorTopic(Option<String>),
    SelectCameraTopic(PanelId, String),
    AddNumericSeries(viewer_core::StreamId, String),
    RemoveNumericSeries(viewer_core::StreamId, String),
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
            #[cfg(feature = "scene-extension")]
            scene_extension: false,
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
            #[cfg(feature = "scene-extension")]
            scene_extension: false,
            startup_warnings: vec![format!(
                "Bundled layout could not be loaded; using emergency layout: {error}"
            )],
        }
    }

    #[cfg(feature = "scene-extension")]
    pub(crate) fn enable_scene_extension(&mut self, topic: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.panels.has_scene(),
            "Scene extension needs a Scene panel"
        );
        anyhow::ensure!(
            topic == self.bindings.path_topic,
            "this slice shares the workspace Path topic {}; change the binding to use another Path",
            self.bindings.path_topic
        );
        self.scene_extension = true;
        Ok(())
    }

    pub(crate) fn requirements_for_cameras(
        &self,
        cameras: &CameraSelection,
    ) -> PanelDataRequirements {
        let requirements = self.panels.requirements_for_cameras(cameras);
        self.with_extension_requirements(requirements)
    }

    fn with_extension_requirements(
        &self,
        requirements: PanelDataRequirements,
    ) -> PanelDataRequirements {
        #[cfg(feature = "scene-extension")]
        let mut requirements = requirements;
        #[cfg(feature = "scene-extension")]
        if self.scene_extension {
            requirements.playback.require_path();
            requirements.playback.optional_transforms();
        }
        requirements
    }

    pub(crate) fn set_point_cloud_topic(&mut self, topic: String) {
        self.bindings.point_cloud_topic = topic;
    }

    pub(crate) fn point_cloud_coloring(&self) -> viewer_core::PointCloudColoring {
        self.panels.first_point_cloud_coloring()
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
            ViewerAction::SelectCameraTopic { panel_id, topic } => {
                WorkspaceEffect::SelectCameraTopic(panel_id, topic)
            }
            ViewerAction::SelectInspectorTopic(topic) => {
                WorkspaceEffect::SelectInspectorTopic(topic)
            }
            ViewerAction::AddNumericSeries { stream, field_path } => {
                WorkspaceEffect::AddNumericSeries(stream, field_path)
            }
            ViewerAction::RemoveNumericSeries { stream, field_path } => {
                WorkspaceEffect::RemoveNumericSeries(stream, field_path)
            }
            ViewerAction::Playback(command) => WorkspaceEffect::Playback(command),
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

    #[cfg(any(test, feature = "ros2-live"))]
    pub(crate) fn data_requirements(&self) -> PanelDataRequirements {
        self.with_extension_requirements(self.panels.data_requirements())
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
                    schema_definition: None,
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
    fn camera_wall_selects_only_its_four_streams() {
        let workspace = NativeWorkspace::load_bundled_or_fallback(WorkspaceLayout::Cameras);
        assert!(workspace.startup_warning().is_none());
        assert_eq!(workspace.panels.placeholder_count(), 0);
        let mut expected = PlaybackRequirements::empty();
        for topic in [
            "/camera/front/image/compressed",
            "/camera/front_left/image/compressed",
            "/camera/front_right/image/compressed",
            "/camera/rear/image/compressed",
        ] {
            expected.require_camera_topic(topic);
        }
        assert_eq!(workspace.data_requirements().playback, expected);
        assert!(workspace.data_requirements().signals.is_empty());
        assert_eq!(
            workspace.scheduler_priority_topic(),
            Some("/camera/front/image/compressed")
        );
    }

    #[test]
    fn analysis_layouts_load_and_inspection_does_not_decode_cameras() {
        for layout in [WorkspaceLayout::Navigation, WorkspaceLayout::Inspection] {
            let workspace = NativeWorkspace::load_bundled_or_fallback(layout);
            assert!(workspace.startup_warning().is_none());
            assert_eq!(workspace.panels.placeholder_count(), 0);
            assert_eq!(
                workspace.data_requirements().signals,
                BTreeSet::from([SignalId::Speed, SignalId::YawRate])
            );
            if layout == WorkspaceLayout::Inspection {
                assert!(workspace.panels.has_embedded_topics());
                // Plots query their series independently of forward feature playback.
                assert_eq!(
                    workspace.data_requirements().playback,
                    PlaybackRequirements::empty()
                );
            }
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
        let mut expected_playback = viewer_core::PlaybackRequirements::empty();
        expected_playback.require_all_cameras();
        expected_playback.require_path();
        expected_playback.optional_odometry();
        expected_playback.require_point_cloud();
        expected_playback.require_transforms();
        assert_eq!(
            workspace.data_requirements(),
            PanelDataRequirements {
                playback: expected_playback,
                signals: BTreeSet::from([SignalId::Speed]),
                inspections: Vec::new(),
                topic_health: BTreeSet::new(),
                diagnostics: BTreeSet::new(),
            }
        );
    }

    #[test]
    fn monitor_embeds_queries_without_overview_or_extra_camera_demand() {
        let mut workspace = NativeWorkspace::load_bundled_or_fallback(WorkspaceLayout::Monitor);
        assert!(workspace.startup_warning().is_none());
        assert_eq!(workspace.panels.len(), 3);
        assert_eq!(workspace.panels.placeholder_count(), 0);
        assert!(workspace.panels.has_embedded_topics());
        assert_eq!(workspace.panel("monitor-topics").kind_name(), "topics");
        assert_eq!(workspace.panel("monitor-front").kind_name(), "camera");
        assert_eq!(workspace.panel("monitor-scene").kind_name(), "scene-3d");
        let mut expected = PlaybackRequirements::empty();
        expected.require_camera_topic("/camera/front/image/compressed");
        expected.optional_path();
        expected.optional_odometry();
        expected.optional_point_cloud();
        expected.optional_transforms();
        let before = workspace.data_requirements();
        assert_eq!(before.playback, expected);
        assert!(before.signals.is_empty());
        assert!(before.inspections.is_empty());
        assert!(
            matches!(workspace.apply_action(ViewerAction::SelectInspectorTopic(Some("/arbitrary".into()))),
            WorkspaceEffect::SelectInspectorTopic(Some(topic)) if topic == "/arbitrary")
        );
        assert_eq!(workspace.data_requirements(), before);
        let LayoutNode::Split {
            direction,
            children,
        } = &workspace.layout.root
        else {
            panic!("row expected");
        };
        assert_eq!(*direction, viewer_layout::SplitDirection::Row);
        assert_eq!((children[0].weight, children[1].weight), (0.3, 0.7));
        let LayoutNode::Split {
            direction,
            children,
        } = &children[1].node
        else {
            panic!("column expected");
        };
        assert_eq!(*direction, viewer_layout::SplitDirection::Column);
        assert_eq!((children[0].weight, children[1].weight), (0.5, 0.5));
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
                topic_health: BTreeSet::new(),
                diagnostics: BTreeSet::new(),
            }
        );
        assert_eq!(NativeWorkspace::default().panels.len(), 4);
        assert_eq!(
            workspace.scheduler_priority_topic(),
            Some("/camera/front/image/compressed")
        );
    }

    #[test]
    fn camera_topic_selection_is_staged_before_changing_focus() {
        let mut workspace = NativeWorkspace::load_bundled_or_fallback(WorkspaceLayout::Showcase);
        configure_camera_runtime(&mut workspace, 4);
        workspace.reset_for_source(Some(CameraId(0)));
        assert!(matches!(
            workspace.apply_action(ViewerAction::SelectCameraTopic {
                panel_id: PanelId::new("camera-left").unwrap(),
                topic: "/camera/front_left/image/compressed".into(),
            }),
            WorkspaceEffect::SelectCameraTopic(_, _)
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
            workspace.apply_action(ViewerAction::SelectCameraTopic {
                panel_id: camera_id,
                topic: "/camera/3".into(),
            }),
            WorkspaceEffect::SelectCameraTopic(_, _)
        ));
        assert_eq!(workspace.focused_camera(), Some(CameraId(0)));
        assert!(!workspace.accumulate_points());

        assert!(matches!(
            workspace.apply_action(ViewerAction::SetAccumulatePoints {
                panel_id: scene_id,
                accumulate: true,
            }),
            WorkspaceEffect::None
        ));
        assert_eq!(workspace.focused_camera(), Some(CameraId(0)));
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

#[cfg(all(test, feature = "scene-extension"))]
mod extension_tests {
    use super::*;
    #[test]
    fn extension_demand_is_startup_only_and_shares_existing_path_route() {
        let mut workspace = NativeWorkspace::load_bundled_or_fallback(WorkspaceLayout::Scene);
        let catalog = viewer_mcap::McapPlayback::open_file(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap"),
        )
        .unwrap()
        .catalog()
        .clone();
        let cameras = workspace
            .panels
            .camera_selection(&catalog, "", None, true)
            .unwrap();
        let before = workspace.requirements_for_cameras(&cameras);
        assert_eq!(
            before.playback,
            workspace.panels.requirements_for_cameras(&cameras).playback
        );
        let plan_before =
            SessionPlan::build(&catalog, "", &before.playback, workspace.bindings()).unwrap();
        assert!(workspace.enable_scene_extension("/different-path").is_err());
        workspace.enable_scene_extension("/planning/path").unwrap();
        let after = workspace.requirements_for_cameras(&cameras);
        let plan_after =
            SessionPlan::build(&catalog, "", &after.playback, workspace.bindings()).unwrap();
        assert_eq!(
            plan_after.selected_stream_ids(),
            plan_before.selected_stream_ids()
        );
        let path_id = plan_after.path_stream().unwrap().id;
        assert_eq!(
            plan_after
                .restore_inputs()
                .iter()
                .filter(|r| r.stream_id == path_id)
                .count(),
            1
        );
        let mut missing = catalog;
        missing.streams.retain(|s| s.id != path_id);
        assert!(SessionPlan::build(&missing, "", &before.playback, workspace.bindings()).is_ok());
        assert!(SessionPlan::build(&missing, "", &after.playback, workspace.bindings()).is_err());
        let mut explorer = NativeWorkspace::load_bundled_or_fallback(WorkspaceLayout::Explore);
        assert!(explorer.enable_scene_extension("/planning/path").is_err());
    }
}
