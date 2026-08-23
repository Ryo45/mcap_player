use super::{
    BevPanel, CameraPanel, InspectorPanel, NativePanel, PanelDataRequirements, PlaceholderPanel,
    PlotPanel, ScenePanel, StatusPanel,
};
use std::collections::BTreeMap;
use viewer_core::CameraId;
#[cfg(test)]
use viewer_core::SignalId;
use viewer_layout::{LayoutDocument, LayoutNode, PanelId, PanelNode};

pub(crate) struct PanelRuntimeStore {
    panels: BTreeMap<PanelId, NativePanel>,
}

pub(crate) struct PanelRuntimeBuildResult {
    pub(crate) store: PanelRuntimeStore,
    pub(crate) warnings: Vec<String>,
}

impl PanelRuntimeStore {
    pub(crate) fn from_layout(document: &LayoutDocument) -> PanelRuntimeBuildResult {
        let mut panels = BTreeMap::new();
        let mut warnings = Vec::new();
        let mut has_bev = false;
        let mut has_scene = false;
        visit_panels(&document.root, &mut |node| {
            let mut panel = create_native_panel(node);
            match &panel {
                NativePanel::Bev(_) if has_bev => {
                    warnings.push(format!(
                        "panel {} is a duplicate BEV instance and was replaced with a placeholder",
                        node.id
                    ));
                    panel = NativePanel::Placeholder(PlaceholderPanel::duplicate_singleton(node));
                }
                NativePanel::Bev(_) => has_bev = true,
                NativePanel::Scene(_) if has_scene => {
                    warnings.push(format!(
                        "panel {} is a duplicate Scene instance and was replaced with a placeholder",
                        node.id
                    ));
                    panel = NativePanel::Placeholder(PlaceholderPanel::duplicate_singleton(node));
                }
                NativePanel::Scene(_) => has_scene = true,
                NativePanel::Camera(_)
                | NativePanel::Plot(_)
                | NativePanel::Inspector(_)
                | NativePanel::Status(_)
                | NativePanel::Placeholder(_) => {}
            }
            if panels.insert(node.id.clone(), panel).is_some() {
                warnings.push(format!(
                    "duplicate runtime panel id {} was ignored by layout validation",
                    node.id
                ));
            }
        });
        PanelRuntimeBuildResult {
            store: Self { panels },
            warnings,
        }
    }

    pub(crate) fn get_mut(&mut self, id: &PanelId) -> Option<&mut NativePanel> {
        self.panels.get_mut(id)
    }

    pub(crate) fn data_requirements(&self) -> PanelDataRequirements {
        let mut requirements = PanelDataRequirements::default();
        for panel in self.panels.values() {
            panel.contribute_data_requirements(&mut requirements);
        }
        requirements
    }

    pub(crate) fn reset_for_source(&mut self, focused_camera: Option<CameraId>) {
        for panel in self.panels.values_mut() {
            panel.reset_for_source(focused_camera);
        }
    }

    pub(crate) fn has_scene(&self) -> bool {
        self.panels
            .values()
            .any(|panel| matches!(panel, NativePanel::Scene(_)))
    }

    pub(crate) fn set_focused_camera(
        &mut self,
        panel_id: &PanelId,
        camera_id: Option<CameraId>,
    ) -> bool {
        self.panels
            .get_mut(panel_id)
            .is_some_and(|panel| panel.set_focused_camera(camera_id))
    }

    pub(crate) fn set_accumulate_points(&mut self, panel_id: &PanelId, accumulate: bool) -> bool {
        self.panels
            .get_mut(panel_id)
            .is_some_and(|panel| panel.set_accumulate_points(accumulate))
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
    match node.panel_type.as_str() {
        "camera" => CameraPanel::create(node),
        "bev" => BevPanel::create(node),
        "plot" => PlotPanel::create(node),
        "inspector" => InspectorPanel::create(node),
        "scene-3d" => ScenePanel::create(node),
        "status" => StatusPanel::create(node),
        _ => NativePanel::Placeholder(PlaceholderPanel::unknown_type(node)),
    }
}

fn visit_panels(node: &LayoutNode, visitor: &mut impl FnMut(&PanelNode)) {
    match node {
        LayoutNode::Split {
            direction: _,
            children,
        } => {
            for child in children {
                visit_panels(&child.node, visitor);
            }
        }
        LayoutNode::Panel(panel) => visitor(panel),
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
            json!({"topic": "/diagnostics", "maxMessages": 8}),
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
    fn inspector_contributes_a_bounded_topic_requirement() {
        let result = PanelRuntimeStore::from_layout(&layout(vec![node(
            "inspector",
            "inspector",
            json!({"topic": "/diagnostics", "maxMessages": 8}),
        )]));
        assert_eq!(
            result.store.data_requirements().inspections,
            vec![crate::inspection::InspectorRequirement {
                topic: "/diagnostics".into(),
                max_messages: 8,
            }]
        );
    }
}
