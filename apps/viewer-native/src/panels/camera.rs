use super::{
    CAMERA_CONFIG_VERSION, NativePanel, PanelDataRequirements, PanelOutput, PlaceholderPanel,
};
use crate::{
    graphics::views::{CameraViewInput, show_camera_view},
    interaction::ViewerAction,
    workspace::CameraViewState,
};
use serde::{Deserialize, Serialize};
use viewer_layout::{PanelId, PanelNode};

#[derive(Clone, Copy)]
pub(crate) struct CameraPanelInput<'a> {
    pub(crate) cameras: &'a [viewer_core::CameraPresentation],
    pub(crate) textures: &'a [crate::graphics::views::CameraTextureView],
    pub(crate) preview_textures: &'a [crate::graphics::views::CameraTextureView],
    pub(crate) preview_active: bool,
    pub(crate) overlays: &'a viewer_renderer::CameraOverlayState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CameraPanelConfig {
    #[serde(default = "default_true")]
    pub(crate) show_thumbnails: bool,
    #[serde(default)]
    pub(crate) camera_topic: Option<String>,
    #[serde(default = "default_true")]
    pub(crate) show_overlay: bool,
    #[serde(default)]
    pub(crate) scheduler_priority: bool,
}

fn default_true() -> bool {
    true
}

pub(crate) struct CameraPanel {
    id: PanelId,
    title: Option<String>,
    config: CameraPanelConfig,
    pub(crate) state: CameraViewState,
}

impl CameraPanel {
    pub(crate) fn create(node: &PanelNode) -> NativePanel {
        if node.config_version != CAMERA_CONFIG_VERSION {
            return NativePanel::Placeholder(PlaceholderPanel::unsupported_version(
                node,
                CAMERA_CONFIG_VERSION,
            ));
        }
        match serde_json::from_value::<CameraPanelConfig>(node.config.clone()) {
            Ok(config)
                if config
                    .camera_topic
                    .as_deref()
                    .is_none_or(|topic| !topic.trim().is_empty())
                    && (!config.scheduler_priority || config.camera_topic.is_some()) =>
            {
                NativePanel::Camera(Self {
                    id: node.id.clone(),
                    title: node.title.clone(),
                    config,
                    state: CameraViewState::default(),
                })
            }
            Ok(_) => NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                "Invalid camera config: cameraTopic must be non-empty and is required when schedulerPriority is true".to_owned(),
            )),
            Err(error) => NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                format!("Invalid camera config: {error}"),
            )),
        }
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, input: CameraPanelInput<'_>) -> PanelOutput {
        ui.push_id((self.id.as_str(), self.title.as_deref()), |ui| {
            let camera_id = self.selected_camera(input.cameras);
            let output = show_camera_view(
                ui,
                CameraViewInput {
                    cameras: input.cameras,
                    textures: input.textures,
                    preview_textures: input.preview_textures,
                    preview_active: input.preview_active,
                    focused_camera: camera_id,
                    show_thumbnails: self.config.show_thumbnails,
                    overlays: input.overlays,
                    show_overlay: self.config.show_overlay,
                    heading: self.title.as_deref().unwrap_or("CAMERA"),
                },
            );
            let mut panel_output = PanelOutput::default();
            if self.config.camera_topic.is_none()
                && let Some(camera_id) = output.selected_camera
            {
                panel_output.actions.push(ViewerAction::SetFocusedCamera {
                    panel_id: self.id.clone(),
                    camera_id: Some(camera_id),
                });
            }
            panel_output
        })
        .inner
    }

    fn selected_camera(
        &self,
        cameras: &[viewer_core::CameraPresentation],
    ) -> Option<viewer_core::CameraId> {
        select_camera_id(
            self.config.camera_topic.as_deref(),
            self.state.focused_camera,
            cameras,
        )
    }

    pub(crate) fn scheduler_priority_topic(&self) -> Option<&str> {
        self.config
            .scheduler_priority
            .then_some(self.config.camera_topic.as_deref())
            .flatten()
    }

    pub(crate) fn contribute_data_requirements(&self, requirements: &mut PanelDataRequirements) {
        if self.config.show_thumbnails && self.config.camera_topic.is_none() {
            requirements.playback.require_all_cameras();
        } else if let Some(topic) = &self.config.camera_topic {
            requirements.playback.require_camera_topic(topic.clone());
        } else {
            requirements.playback.require_all_cameras();
        }
        if self.config.show_overlay {
            requirements.playback.optional_path();
            requirements.playback.optional_transforms();
        }
    }

    pub(crate) fn set_focused_camera(&mut self, camera_id: Option<viewer_core::CameraId>) -> bool {
        if self.config.camera_topic.is_some() {
            return false;
        }
        self.state.focused_camera = camera_id;
        true
    }
}

fn select_camera_id(
    configured_topic: Option<&str>,
    interactive_focus: Option<viewer_core::CameraId>,
    cameras: &[viewer_core::CameraPresentation],
) -> Option<viewer_core::CameraId> {
    configured_topic.map_or(interactive_focus, |topic| {
        cameras
            .iter()
            .find(|camera| camera.topic == topic)
            .map(|camera| camera.id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{CameraId, CameraPresentation, CameraStatus, OverlayStatus};

    #[test]
    fn fixed_camera_selector_config_is_typed() {
        let default: CameraPanelConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(default.camera_topic, None);
        assert!(default.show_overlay);

        let selected: CameraPanelConfig = serde_json::from_value(serde_json::json!({
            "cameraTopic": "/camera/front_left/image/compressed",
            "showThumbnails": false,
            "showOverlay": false,
            "schedulerPriority": true
        }))
        .unwrap();
        assert_eq!(
            selected.camera_topic.as_deref(),
            Some("/camera/front_left/image/compressed")
        );
        assert!(!selected.show_thumbnails);
        assert!(!selected.show_overlay);
        assert!(selected.scheduler_priority);
    }

    #[test]
    fn configured_topic_selects_exactly_without_semantic_role_heuristics() {
        let camera = |id, topic: &str| CameraPresentation {
            id: CameraId(id),
            topic: topic.to_owned(),
            status: CameraStatus::WaitingForCameraFrame,
            fps: 0.0,
            overlay: OverlayStatus::Waiting,
            focused: false,
        };
        let cameras = vec![
            camera(0, "/camera/rear_left/image/compressed"),
            camera(1, "/camera/front_left/image/compressed"),
        ];
        assert_eq!(
            select_camera_id(
                Some("/camera/front_left/image/compressed"),
                Some(CameraId(0)),
                &cameras,
            ),
            Some(CameraId(1))
        );
        assert_eq!(
            select_camera_id(Some("left"), Some(CameraId(0)), &cameras),
            None
        );
        assert_eq!(
            select_camera_id(None, Some(CameraId(0)), &cameras),
            Some(CameraId(0))
        );
    }
}
