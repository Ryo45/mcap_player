//! Camera selection, view state and presentation shared by both hosts.
use crate::{
    action::ViewerAction,
    panel::PanelOutput,
    views::camera::{CameraTextureView, CameraViewInput, show_camera_view},
};
use serde::{Deserialize, Serialize};
use viewer_layout::{PanelId, PanelNode};

pub const CAMERA_CONFIG_VERSION: u32 = 1;

#[derive(Clone, Copy)]
pub struct CameraPanelInput<'a> {
    pub cameras: &'a [viewer_core::CameraPresentation],
    pub textures: &'a [CameraTextureView],
    pub preview_textures: &'a [CameraTextureView],
    pub preview_active: bool,
    pub overlays: &'a viewer_renderer::CameraOverlayState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CameraPanelConfig {
    #[serde(default = "default_true")]
    pub show_thumbnails: bool,
    #[serde(default)]
    pub camera_topic: Option<String>,
    #[serde(default = "default_true")]
    pub show_overlay: bool,
    #[serde(default)]
    pub scheduler_priority: bool,
}

fn default_true() -> bool {
    true
}

pub struct CameraPanel {
    id: PanelId,
    title: Option<String>,
    config: CameraPanelConfig,
    pub state: CameraViewState,
    available_topics: Vec<String>,
}

impl CameraPanel {
    pub fn create(node: &PanelNode) -> Result<Self, String> {
        if node.config_version != CAMERA_CONFIG_VERSION {
            return Err(format!(
                "Unsupported {} config version {}; expected {}",
                node.panel_type, node.config_version, CAMERA_CONFIG_VERSION
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
                Ok(Self {
                    id: node.id.clone(),
                    title: node.title.clone(),
                    config,
                    state: CameraViewState::default(),
                    available_topics: Vec::new(),
                })
            }
            Ok(_) => Err(
                "Invalid camera config: cameraTopic must be non-empty and is required when schedulerPriority is true".to_owned(),
            ),
            Err(error) => Err(
                format!("Invalid camera config: {error}"),
            ),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, input: CameraPanelInput<'_>) -> PanelOutput {
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

    pub fn scheduler_priority_topic(&self) -> Option<&str> {
        if self.config.scheduler_priority {
            self.state
                .topic
                .as_deref()
                .or(self.config.camera_topic.as_deref())
        } else {
            None
        }
    }

    pub fn preferred_topic(&self, reset: bool) -> Option<&str> {
        if reset {
            self.config.camera_topic.as_deref()
        } else {
            self.state
                .topic
                .as_deref()
                .or(self.config.camera_topic.as_deref())
        }
    }

    pub fn contribute_camera_requirements(
        &self,
        topic: Option<&str>,
        requirements: &mut viewer_core::PlaybackRequirements,
    ) {
        if let Some(topic) = topic {
            if self.config.camera_topic.is_none() {
                requirements.require_all_cameras();
            } else {
                requirements.require_camera_topic(topic);
            }
        }
        if self.config.show_overlay {
            requirements.optional_path();
            requirements.optional_transforms();
        }
    }

    pub fn bind_topics(
        &mut self,
        topics: &[String],
        topic: Option<&str>,
        routes: &[(viewer_core::CameraId, String)],
    ) {
        self.available_topics = topics.to_vec();
        self.state.topic = topic.map(str::to_owned);
        self.state.focused_camera = routes
            .iter()
            .find(|(_, candidate)| Some(candidate.as_str()) == topic)
            .map(|(id, _)| *id);
    }

    pub fn contribute_data_requirements(
        &self,
        requirements: &mut viewer_core::PlaybackRequirements,
    ) {
        if self.config.show_thumbnails && self.config.camera_topic.is_none() {
            requirements.require_all_cameras();
        } else if let Some(topic) = self
            .state
            .topic
            .as_deref()
            .or(self.config.camera_topic.as_deref())
        {
            requirements.require_camera_topic(topic);
        } else {
            requirements.require_all_cameras();
        }
        if self.config.show_overlay {
            requirements.optional_path();
            requirements.optional_transforms();
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CameraViewState {
    pub focused_camera: Option<viewer_core::CameraId>,
    pub topic: Option<String>,
}

use std::collections::BTreeMap;
/// Small prospective bindings, staged separately from the active decoded runtime.
pub struct CameraSelection {
    pub available_topics: Vec<String>,
    pub panels: BTreeMap<PanelId, Option<String>>,
}

impl CameraSelection {
    pub fn priority_topic<'a>(&'a self, preferred: &'a str) -> &'a str {
        self.panels
            .values()
            .flatten()
            .find(|topic| topic.as_str() == preferred)
            .or_else(|| self.panels.values().flatten().next())
            .map_or(preferred, String::as_str)
    }
}

pub fn select_topics<'a>(
    catalog: &viewer_core::SourceCatalog,
    preferred: &str,
    panels: impl Iterator<Item = (&'a PanelId, &'a CameraPanel)>,
    change: Option<(&PanelId, &str)>,
    reset: bool,
) -> Result<CameraSelection, String> {
    let cameras = panels.collect::<std::collections::BTreeMap<_, _>>();
    let available_topics = viewer_core::camera_streams(catalog)
        .map(|stream| stream.topic.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if let Some((id, topic)) = change {
        if !cameras.contains_key(id) {
            return Err(format!("Camera panel {id} is unavailable"));
        }
        if !available_topics.iter().any(|candidate| candidate == topic) {
            return Err(format!("Unsupported image topic {topic:?}"));
        }
    }
    let fallback = available_topics
        .iter()
        .find(|topic| topic.as_str() == preferred)
        .or_else(|| available_topics.first());
    let panels = cameras
        .into_iter()
        .map(|(id, panel)| {
            let topic = change
                .filter(|(changed, _)| *changed == id)
                .map(|(_, topic)| topic)
                .or_else(|| panel.preferred_topic(reset))
                .and_then(|topic| {
                    available_topics
                        .iter()
                        .find(|candidate| candidate.as_str() == topic)
                })
                .or(fallback)
                .cloned();
            (id.clone(), topic)
        })
        .collect();
    Ok(CameraSelection {
        available_topics,
        panels,
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
        let mut panel = CameraPanel::create(&node).unwrap();
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
    fn topic_resolution_is_scoped_and_rejects_invalid_choices_without_mutation() {
        let catalog = viewer_core::SourceCatalog {
            streams: vec![viewer_core::StreamDescriptor {
                id: viewer_core::StreamId(1),
                topic: "/robot/image".into(),
                schema: "sensor_msgs/msg/Image".into(),
                message_encoding: "cdr".into(),
                schema_definition: None,
                timing: Default::default(),
            }],
            ..Default::default()
        };
        let node = viewer_layout::PanelNode {
            id: PanelId::new("camera").unwrap(),
            panel_type: "camera".into(),
            config_version: 1,
            title: None,
            config: serde_json::json!({}),
        };
        let panel = CameraPanel::create(&node).unwrap();
        let selection = select_topics(
            &catalog,
            "/old/topic",
            std::iter::once((&node.id, &panel)),
            None,
            true,
        )
        .unwrap();
        assert_eq!(selection.panels[&node.id].as_deref(), Some("/robot/image"));
        assert!(
            panel.state.topic.is_none(),
            "resolution must not commit panel state"
        );
        assert!(
            select_topics(
                &catalog,
                "",
                std::iter::once((&node.id, &panel)),
                Some((&node.id, "/missing")),
                false
            )
            .is_err()
        );
        assert!(
            select_topics(
                &catalog,
                "",
                std::iter::once((&node.id, &panel)),
                Some((&PanelId::new("other").unwrap(), "/robot/image")),
                false
            )
            .is_err()
        );
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
