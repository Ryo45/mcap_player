use crate::{action::ViewerAction, views::scene::SceneViewOutput};

#[derive(Default)]
pub struct PanelOutput {
    pub actions: Vec<ViewerAction>,
    pub render_requests: PanelRenderRequests,
}

#[derive(Default)]
pub struct PanelRenderRequests {
    pub camera_sizes: std::collections::BTreeMap<viewer_core::CameraId, (u32, u32)>,
    pub bev_size: Option<egui::Vec2>,
    pub scene: Option<SceneViewOutput>,
    pub plot_rect: Option<egui::Rect>,
}

impl PanelOutput {
    pub fn merge(&mut self, mut other: Self) {
        for (camera, size) in other.render_requests.camera_sizes {
            let current = self
                .render_requests
                .camera_sizes
                .entry(camera)
                .or_insert((0, 0));
            current.0 = current.0.max(size.0);
            current.1 = current.1.max(size.1);
        }
        self.actions.append(&mut other.actions);
        if other.render_requests.bev_size.is_some() {
            self.render_requests.bev_size = other.render_requests.bev_size;
        }
        if other.render_requests.plot_rect.is_some() {
            self.render_requests.plot_rect = other.render_requests.plot_rect;
        }
        if other.render_requests.scene.is_some() {
            self.render_requests.scene = other.render_requests.scene;
        }
    }
}

/// The shared layout factory is the single registration point for migrated panels.
pub enum Panel {
    Camera(crate::camera::CameraPanel),
    Scene(crate::scene::ScenePanel),
    Plot(crate::plot::PlotPanel),
    Bev(crate::bev::BevPanel),
    Status(crate::status::StatusPanel),
    Inspector(crate::inspector::InspectorPanel),
    Topics(crate::topics::TopicsPanel),
    Placeholder(crate::placeholder::PlaceholderPanel),
}
impl Panel {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Camera(_) => "camera",
            Self::Scene(_) => "scene-3d",
            Self::Plot(_) => "plot",
            Self::Bev(_) => "bev",
            Self::Status(_) => "status",
            Self::Inspector(_) => "inspector",
            Self::Topics(_) => "topics",
            Self::Placeholder(_) => "placeholder",
        }
    }
    pub fn create(node: &viewer_layout::PanelNode) -> Option<Result<Self, String>> {
        Some(match node.panel_type.as_str() {
            "camera" => crate::camera::CameraPanel::create(node).map(Self::Camera),
            "scene-3d" => crate::scene::ScenePanel::create(node).map(Self::Scene),
            "plot" => crate::plot::PlotPanel::create(node).map(Self::Plot),
            "bev" => crate::bev::BevPanel::create(node).map(Self::Bev),
            "status" => crate::status::StatusPanel::create(node).map(Self::Status),
            "inspector" => crate::inspector::InspectorPanel::create(node).map(Self::Inspector),
            "topics" if node.config_version == 1 && node.config == serde_json::json!({}) => {
                Ok(Self::Topics(Default::default()))
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use viewer_layout::{PanelId, PanelNode};

    #[test]
    fn shared_factory_validates_configuration_before_either_host_builds_a_session() {
        let mut node = PanelNode {
            id: PanelId::new("panel").unwrap(),
            panel_type: "camera".into(),
            config_version: 1,
            title: None,
            config: json!({}),
        };
        assert!(matches!(Panel::create(&node), Some(Ok(Panel::Camera(_)))));
        node.config = json!({"cameraTopic":" "});
        assert!(Panel::create(&node).unwrap().is_err());
        node.panel_type = "plot".into();
        node.config = json!({"signal":"vehicle-speed"});
        assert!(matches!(Panel::create(&node), Some(Ok(Panel::Plot(_)))));
        node.config_version = 100;
        assert!(matches!(Panel::create(&node), Some(Err(error)) if error.contains("version")));
        node.panel_type = "unknown".into();
        assert!(Panel::create(&node).is_none());
    }
}
