use super::{CameraSelection, NativePanel, PanelDataRequirements, PlaceholderPanel};
#[cfg(test)]
use crate::inspection::InspectorRequirement;
use viewer_core::CameraId;
#[cfg(test)]
use viewer_core::SignalId;
use viewer_layout::{LayoutDocument, PanelId, PanelNode};

pub(crate) struct PanelRuntimeStore {
    pub(crate) panels: viewer_ui::workspace::PanelStore<NativePanel>,
    topic_inspector: viewer_ui::inspection::InspectorTreeState,
}

pub(crate) struct PanelRuntimeBuildResult {
    pub(crate) store: PanelRuntimeStore,
    pub(crate) warnings: Vec<String>,
}

impl PanelRuntimeStore {
    pub(crate) fn from_layout(document: &LayoutDocument) -> PanelRuntimeBuildResult {
        let (panels, warnings) = viewer_ui::workspace::PanelStore::from_layout(
            document,
            create_native_panel,
            NativePanel::kind_name,
            |node, error| NativePanel::Placeholder(PlaceholderPanel::invalid_config(node, error)),
        );
        PanelRuntimeBuildResult {
            store: Self {
                panels,
                topic_inspector: Default::default(),
            },
            warnings,
        }
    }

    pub(crate) fn show_topic_inspector(
        &mut self,
        context: &egui::Context,
        topic: Option<&str>,
        inspections: &[crate::inspection::TopicInspection],
        cursor: Option<viewer_core::ArrivalTime>,
    ) -> Vec<viewer_ui::action::ViewerAction> {
        viewer_ui::inspector::show_topic_inspector(
            context,
            topic,
            super::InspectorPanelInput {
                inspections: &inspections
                    .iter()
                    .map(crate::inspection::TopicInspection::view)
                    .collect::<Vec<_>>(),
                cursor,
            },
            &mut self.topic_inspector,
        )
    }
    #[cfg(any(test, feature = "ros2-live"))]
    pub(crate) fn data_requirements(&self) -> PanelDataRequirements {
        let mut requirements = PanelDataRequirements::default();
        for panel in self.panels.values() {
            panel.contribute_data_requirements(&mut requirements);
        }
        requirements
    }

    pub(crate) fn camera_selection(
        &self,
        catalog: &viewer_core::SourceCatalog,
        preferred: &str,
        change: Option<(&PanelId, &str)>,
        reset: bool,
    ) -> Result<CameraSelection, String> {
        viewer_ui::camera::select_topics(
            catalog,
            preferred,
            self.panels.iter().filter_map(|(id, panel)| match panel {
                NativePanel::Camera(camera) => Some((id, camera)),
                _ => None,
            }),
            change,
            reset,
        )
    }

    pub(crate) fn requirements_for_cameras(
        &self,
        selection: &CameraSelection,
    ) -> PanelDataRequirements {
        let mut requirements = PanelDataRequirements::default();
        for (id, panel) in self.panels.iter() {
            if let NativePanel::Camera(panel) = panel {
                panel.contribute_camera_requirements(
                    selection.panels.get(id).and_then(|topic| topic.as_deref()),
                    &mut requirements.playback,
                );
            } else {
                panel.contribute_data_requirements(&mut requirements);
            }
        }
        requirements
    }

    pub(crate) fn bind_cameras(
        &mut self,
        selection: &CameraSelection,
        plan: &viewer_core::SessionPlan,
    ) {
        let routes = plan
            .camera_routes()
            .iter()
            .map(|route| (route.camera_id, route.stream.topic.clone()))
            .collect::<Vec<_>>();
        for (id, panel) in self.panels.iter_mut() {
            if let NativePanel::Camera(panel) = panel {
                panel.bind_topics(
                    &selection.available_topics,
                    selection.panels.get(id).and_then(|topic| topic.as_deref()),
                    &routes,
                );
            }
        }
    }

    pub(crate) fn reset_for_source(&mut self, focused_camera: Option<CameraId>) {
        self.topic_inspector = Default::default();
        for panel in self.panels.values_mut() {
            panel.reset_for_source(focused_camera);
        }
    }

    pub(crate) fn has_embedded_topics(&self) -> bool {
        self.panels
            .values()
            .any(|panel| matches!(panel, NativePanel::Topics(_)))
    }

    pub(crate) fn has_scene(&self) -> bool {
        self.panels
            .values()
            .any(|panel| matches!(panel, NativePanel::Scene(_)))
    }

    pub(crate) fn set_accumulate_points(&mut self, panel_id: &PanelId, accumulate: bool) -> bool {
        self.panels
            .get_mut(panel_id)
            .is_some_and(|panel| panel.set_accumulate_points(accumulate))
    }

    pub(crate) fn first_point_cloud_coloring(&self) -> viewer_core::PointCloudColoring {
        self.panels
            .values()
            .find_map(|panel| match panel {
                NativePanel::Scene(panel) => Some(panel.state.coloring),
                _ => None,
            })
            .unwrap_or_default()
    }

    pub(crate) fn first_accumulate_points(&self) -> bool {
        self.panels
            .values()
            .find_map(NativePanel::accumulate_points)
            .unwrap_or(false)
    }

    pub(crate) fn scheduler_priority_topic(&self) -> Option<&str> {
        self.panels
            .values()
            .find_map(NativePanel::scheduler_priority_topic)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.panels.len()
    }

    #[cfg(test)]
    pub(crate) fn placeholder_count(&self) -> usize {
        self.panels
            .values()
            .filter(|panel| matches!(panel, NativePanel::Placeholder(_)))
            .count()
    }

    #[cfg(test)]
    pub(crate) fn get(&self, id: &PanelId) -> Option<&NativePanel> {
        self.panels.get(id)
    }
}

pub(crate) fn create_native_panel(node: &PanelNode) -> NativePanel {
    if let Some(panel) = viewer_ui::panel::Panel::create(node) {
        return match panel {
            Ok(viewer_ui::panel::Panel::Camera(panel)) => NativePanel::Camera(panel),
            Ok(viewer_ui::panel::Panel::Scene(panel)) => NativePanel::Scene(panel),
            Ok(viewer_ui::panel::Panel::Plot(panel)) => NativePanel::Plot(panel),
            Ok(viewer_ui::panel::Panel::Status(panel)) => NativePanel::Status(panel),
            Ok(viewer_ui::panel::Panel::Inspector(panel)) => NativePanel::Inspector(panel),
            Ok(viewer_ui::panel::Panel::Bev(panel)) => NativePanel::Bev(panel),
            Ok(viewer_ui::panel::Panel::Topics(panel)) => NativePanel::Topics(panel),
            Ok(viewer_ui::panel::Panel::Placeholder(panel)) => NativePanel::Placeholder(panel),
            Err(error) => NativePanel::Placeholder(PlaceholderPanel::invalid_config(node, error)),
        };
    }
    match node.panel_type.as_str() {
        "explore" if node.config_version == 1 && node.config == serde_json::json!({}) => {
            NativePanel::Explore
        }
        #[cfg(feature = "plugin-panels")]
        "plugin" => super::plugin::PluginPanel::create(node),
        "topic-health" => super::topic_health::TopicHealthPanel::create(node),
        "diagnostics" => super::diagnostics::DiagnosticsPanel::create(node),
        _ => NativePanel::Placeholder(PlaceholderPanel::unknown_type(node)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use viewer_layout::{CURRENT_LAYOUT_SCHEMA_VERSION, LayoutNode, SplitChild, SplitDirection};

    fn node(id: &str, panel_type: &str, config: serde_json::Value) -> PanelNode {
        PanelNode {
            id: PanelId::new(id).unwrap(),
            panel_type: panel_type.to_owned(),
            config_version: 1,
            title: None,
            config,
        }
    }

    fn layout(nodes: Vec<PanelNode>) -> LayoutDocument {
        LayoutDocument {
            schema_version: CURRENT_LAYOUT_SCHEMA_VERSION,
            root: LayoutNode::Split {
                direction: SplitDirection::Row,
                children: nodes
                    .into_iter()
                    .map(|panel| SplitChild {
                        weight: 1.0,
                        node: LayoutNode::Panel(panel),
                    })
                    .collect(),
            },
        }
    }

    #[test]
    fn parses_camera_and_plot_configs() {
        let camera =
            create_native_panel(&node("camera", "camera", json!({"showThumbnails": true})));
        assert_eq!(camera.kind_name(), "camera");
        let plot = create_native_panel(&node("plot", "plot", json!({"signal": "vehicle-speed"})));
        assert_eq!(plot.kind_name(), "plot");
        let inspector = create_native_panel(&node(
            "inspector",
            "inspector",
            json!({"topic": "/diagnostics"}),
        ));
        assert_eq!(inspector.kind_name(), "inspector");
        let status = create_native_panel(&node("status", "status", json!({})));
        assert_eq!(status.kind_name(), "status");
    }

    #[test]
    fn unknown_invalid_and_unsupported_panels_preserve_original_config() {
        let original = json!({"future": 42});
        let unknown_node = node("unknown", "future-panel", original.clone());
        let NativePanel::Placeholder(unknown) = create_native_panel(&unknown_node) else {
            panic!("unknown type must become a placeholder")
        };
        assert_eq!(unknown.original_config, original);
        assert!(unknown.error.contains("Unknown panel type"));

        let invalid = create_native_panel(&node("invalid", "camera", json!({"fit": "stretch-x"})));
        let NativePanel::Placeholder(invalid) = invalid else {
            panic!("invalid config must become a placeholder")
        };
        assert!(invalid.error.contains("Invalid camera config"));

        let mut unsupported_node = node("unsupported", "camera", json!({}));
        unsupported_node.config_version = 2;
        let unsupported = create_native_panel(&unsupported_node);
        let NativePanel::Placeholder(unsupported) = unsupported else {
            panic!("unsupported version must become a placeholder")
        };
        assert_eq!(unsupported.config_version, 2);
        assert!(
            unsupported
                .error
                .contains("Unsupported camera config version")
        );
    }

    #[test]
    fn enforces_only_bev_and_scene_singletons() {
        let result = PanelRuntimeStore::from_layout(&layout(vec![
            node("bev-a", "bev", json!({})),
            node("bev-b", "bev", json!({})),
            node("scene-a", "scene-3d", json!({})),
            node("scene-b", "scene-3d", json!({})),
        ]));
        assert_eq!(result.store.len(), 4);
        assert_eq!(result.store.placeholder_count(), 2);
        assert_eq!(result.warnings.len(), 2);
    }

    #[test]
    fn permits_multiple_camera_and_plot_panels() {
        let result = PanelRuntimeStore::from_layout(&layout(vec![
            node("camera-a", "camera", json!({})),
            node("camera-b", "camera", json!({})),
            node("plot-a", "plot", json!({"signal": "vehicle-speed"})),
            node("plot-b", "plot", json!({"signal": "yaw-rate"})),
        ]));
        assert_eq!(result.store.len(), 4);
        assert_eq!(result.store.placeholder_count(), 0);
        assert!(result.warnings.is_empty());
        assert_eq!(
            result.store.data_requirements().signals,
            std::collections::BTreeSet::from([SignalId::Speed, SignalId::YawRate])
        );
    }

    #[test]
    fn status_requires_speed_without_a_plot_specific_dependency() {
        let result =
            PanelRuntimeStore::from_layout(&layout(vec![node("status", "status", json!({}))]));
        assert_eq!(
            result.store.data_requirements().signals,
            std::collections::BTreeSet::from([SignalId::Speed])
        );
    }

    #[test]
    fn health_and_diagnostics_requirements_are_deduplicated_without_playback_routes() {
        let result = PanelRuntimeStore::from_layout(&layout(vec![
            node("health-a", "topic-health", json!({})),
            node("health-b", "topic-health", json!({"windowSeconds": 10})),
            node("diag-a", "diagnostics", json!({})),
            node(
                "diag-b",
                "diagnostics",
                json!({"topic": "/diagnostics", "windowSeconds": 10}),
            ),
            node(
                "diag-custom",
                "diagnostics",
                json!({"topic": "/system/diagnostics", "windowSeconds": 5}),
            ),
        ]));
        assert_eq!(result.store.placeholder_count(), 0);
        let requirements = result.store.data_requirements();
        assert_eq!(
            requirements.playback,
            viewer_core::PlaybackRequirements::empty()
        );
        assert!(requirements.signals.is_empty());
        assert!(requirements.inspections.is_empty());
        assert_eq!(requirements.topic_health.len(), 1);
        assert_eq!(requirements.diagnostics.len(), 2);
        let preset = crate::workspace::NativeWorkspace::load_bundled_or_fallback(
            crate::workspace::WorkspaceLayout::Health,
        );
        assert!(preset.startup_warning().is_none());
        assert_eq!(preset.panels.placeholder_count(), 0);
        assert_eq!(preset.panels.len(), 2);
    }

    #[test]
    fn window_panels_reject_invalid_config_and_versions() {
        for kind in ["topic-health", "diagnostics"] {
            for seconds in [0, 61, u32::MAX] {
                assert!(matches!(
                    create_native_panel(&node("bad", kind, json!({"windowSeconds": seconds}))),
                    NativePanel::Placeholder(_)
                ));
            }
            let mut unsupported = node("bad", kind, json!({}));
            unsupported.config_version = 2;
            assert!(matches!(
                create_native_panel(&unsupported),
                NativePanel::Placeholder(_)
            ));
        }
        assert!(matches!(
            create_native_panel(&node("empty", "diagnostics", json!({"topic":" "}))),
            NativePanel::Placeholder(_)
        ));
    }

    #[test]
    fn inspector_contributes_a_bounded_topic_requirement() {
        let result = PanelRuntimeStore::from_layout(&layout(vec![node(
            "inspector",
            "inspector",
            json!({"topic": "/diagnostics"}),
        )]));
        assert_eq!(
            result.store.data_requirements().inspections,
            vec![InspectorRequirement {
                topic: "/diagnostics".into(),
            }]
        );
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    #[test]
    fn builtin_plot_only_requires_a_query() {
        let node = PanelNode {
            id: PanelId::new("speed").unwrap(),
            panel_type: "plot".into(),
            config_version: 1,
            title: None,
            config: serde_json::json!({"signal":"vehicle-speed"}),
        };
        let mut requirements = PanelDataRequirements::default();
        create_native_panel(&node).contribute_data_requirements(&mut requirements);
        assert_eq!(
            requirements.playback,
            viewer_core::PlaybackRequirements::empty()
        );
        assert_eq!(
            requirements.signals,
            std::collections::BTreeSet::from([viewer_core::SignalId::Speed])
        );
    }
}
