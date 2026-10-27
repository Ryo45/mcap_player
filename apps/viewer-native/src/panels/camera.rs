use super::{
    CAMERA_CONFIG_VERSION, NativePanel, PanelDataRequirements, PanelOutput, PlaceholderPanel,
};
use crate::{
    graphics::views::{CameraViewInput, show_camera_view},
    interaction::ViewerAction,
    workspace::CameraViewState,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use viewer_layout::{PanelId, PanelNode};

/// Small prospective bindings, staged separately from the active decoded runtime.
pub(crate) struct CameraSelection {
    pub(crate) available_topics: Vec<String>,
    pub(crate) panels: BTreeMap<PanelId, Option<String>>,
}

impl CameraSelection {
    pub(crate) fn priority_topic<'a>(&'a self, preferred: &'a str) -> &'a str {
        self.panels
            .values()
            .flatten()
            .find(|topic| topic.as_str() == preferred)
            .or_else(|| self.panels.values().flatten().next())
            .map_or(preferred, String::as_str)
    }
}

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
    available_topics: Vec<String>,
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
                    available_topics: Vec::new(),
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
            let mut selected_topic = self.state.topic.clone();
            egui::ComboBox::from_id_salt("image-topic")
                .selected_text(selected_topic.as_deref().unwrap_or("No image topics"))
                .width(ui.available_width().max(1.0))
                .show_ui(ui, |ui| {
                    for topic in &self.available_topics {
                        ui.selectable_value(&mut selected_topic, Some(topic.clone()), topic);
                    }
                });
            let mut panel_output = PanelOutput::default();
            if selected_topic != self.state.topic
                && let Some(topic) = selected_topic
            {
                panel_output.actions.push(ViewerAction::SelectCameraTopic {
                    panel_id: self.id.clone(),
                    topic,
                });
            }
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
            panel_output.render_requests.camera_sizes = output.decode_sizes;
            if let Some(camera_id) = output.selected_camera
                && let Some(camera) = input.cameras.iter().find(|camera| camera.id == camera_id)
            {
                panel_output.actions.push(ViewerAction::SelectCameraTopic {
                    panel_id: self.id.clone(),
                    topic: camera.topic.clone(),
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
            self.state
                .topic
                .as_deref()
                .or(self.config.camera_topic.as_deref()),
            self.state.focused_camera,
            cameras,
        )
    }

    pub(crate) fn scheduler_priority_topic(&self) -> Option<&str> {
        if self.config.scheduler_priority {
            self.state
                .topic
                .as_deref()
                .or(self.config.camera_topic.as_deref())
        } else {
            None
        }
    }

    pub(crate) fn preferred_topic(&self, reset: bool) -> Option<&str> {
        if reset {
            self.config.camera_topic.as_deref()
        } else {
            self.state
                .topic
                .as_deref()
                .or(self.config.camera_topic.as_deref())
        }
    }

    pub(crate) fn contribute_camera_requirements(
        &self,
        topic: Option<&str>,
        requirements: &mut PanelDataRequirements,
    ) {
        if let Some(topic) = topic {
            if self.config.camera_topic.is_none() {
                requirements.playback.require_all_cameras();
            } else {
                requirements.playback.require_camera_topic(topic);
            }
        }
        if self.config.show_overlay {
            requirements.playback.optional_path();
            requirements.playback.optional_transforms();
        }
    }

    pub(crate) fn bind_topics(
        &mut self,
        topics: &[String],
        topic: Option<&str>,
        plan: &viewer_core::SessionPlan,
    ) {
        self.available_topics = topics.to_vec();
        self.state.topic = topic.map(str::to_owned);
        self.state.focused_camera = plan
            .camera_routes()
            .iter()
            .find(|route| Some(route.stream.topic.as_str()) == topic)
            .map(|route| route.camera_id);
    }

    pub(crate) fn contribute_data_requirements(&self, requirements: &mut PanelDataRequirements) {
        if self.config.show_thumbnails && self.config.camera_topic.is_none() {
            requirements.playback.require_all_cameras();
        } else if let Some(topic) = self
            .state
            .topic
            .as_deref()
            .or(self.config.camera_topic.as_deref())
        {
            requirements.playback.require_camera_topic(topic);
        } else {
            requirements.playback.require_all_cameras();
        }
        if self.config.show_overlay {
            requirements.playback.optional_path();
            requirements.playback.optional_transforms();
        }
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
    fn dropdown_emits_a_scoped_topic_choice_without_committing_the_binding() {
        let node = PanelNode {
            id: PanelId::new("camera").unwrap(),
            panel_type: "camera".into(),
            config_version: CAMERA_CONFIG_VERSION,
            title: None,
            config: serde_json::json!({"cameraTopic":"/project/a", "showThumbnails":false}),
        };
        let NativePanel::Camera(mut panel) = CameraPanel::create(&node) else {
            panic!("valid camera panel")
        };
        panel.available_topics = vec!["/project/a".into(), "/project/b".into()];
        panel.state.topic = Some("/project/a".into());
        let context = egui::Context::default();
        let overlays = viewer_renderer::CameraOverlayState::default();
        let mut frame = |events| {
            let mut actions = Vec::new();
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        actions = panel
                            .show(
                                ui,
                                CameraPanelInput {
                                    cameras: &[],
                                    textures: &[],
                                    preview_textures: &[],
                                    preview_active: false,
                                    overlays: &overlays,
                                },
                            )
                            .actions;
                    });
                },
            );
            (output, actions)
        };
        let label_position = |output: &egui::FullOutput, topic: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) if text.galley.text() == topic => {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .expect("visible image topic label")
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let (output, _) = frame(vec![]);
        let pos = label_position(&output, "/project/a");
        frame(vec![egui::Event::PointerMoved(pos), button(pos, true)]);
        frame(vec![button(pos, false)]);
        let (output, _) = frame(vec![]);
        let pos = label_position(&output, "/project/b");
        frame(vec![egui::Event::PointerMoved(pos), button(pos, true)]);
        let (_, actions) = frame(vec![button(pos, false)]);
        assert_eq!(
            actions,
            [ViewerAction::SelectCameraTopic {
                panel_id: node.id,
                topic: "/project/b".into()
            }]
        );
        assert_eq!(panel.state.topic.as_deref(), Some("/project/a"));
    }

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
