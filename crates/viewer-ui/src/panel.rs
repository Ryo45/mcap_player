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

/// UI-declared demand. Hosts execute queries independently from continuous playback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanelRequirements {
    pub playback: viewer_core::PlaybackRequirements,
    pub signals: std::collections::BTreeSet<viewer_core::SignalId>,
    pub inspections: std::collections::BTreeSet<String>,
}
impl Default for PanelRequirements {
    fn default() -> Self {
        Self {
            playback: viewer_core::PlaybackRequirements::empty(),
            signals: Default::default(),
            inspections: Default::default(),
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
    pub fn contribute_requirements(&self, requirements: &mut PanelRequirements) {
        match self {
            Self::Camera(p) => p.contribute_data_requirements(&mut requirements.playback),
            Self::Scene(p) => p.contribute_data_requirements(&mut requirements.playback),
            Self::Bev(p) => p.contribute_data_requirements(&mut requirements.playback),
            Self::Status(p) => {
                p.contribute_data_requirements(&mut requirements.playback);
            }
            Self::Plot(p) => {
                requirements.signals.insert(p.signal_id());
            }
            Self::Inspector(p) => {
                requirements.inspections.insert(p.topic().to_owned());
            }
            Self::Topics(_) | Self::Placeholder(_) => {}
        }
    }

    /// Clear source-bound UI identities/caches; retain display preferences and configured topics.
    /// Hosts cancel their own query/I/O work and bind the new source after this reset.
    pub fn reset_for_source(&mut self, focused_camera: Option<viewer_core::CameraId>) {
        match self {
            Self::Camera(p) => {
                p.state = crate::camera::CameraViewState {
                    focused_camera,
                    topic: None,
                }
            }
            Self::Plot(p) => p.reset_for_source(),
            Self::Inspector(p) => p.reset_for_source(),
            Self::Topics(p) => p.reset_for_source(),
            Self::Scene(_) | Self::Bev(_) | Self::Status(_) | Self::Placeholder(_) => {}
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        input: &crate::panel_input::PanelInput<'_>,
    ) -> PanelOutput {
        use crate::{
            bev::BevPanelInput, camera::CameraPanelInput, inspector::InspectorPanelInput,
            plot::PlotPanelInput, scene::ScenePanelInput, status::StatusPanelInput,
            topics::TopicsPanelInput,
        };
        let presentation = input.presentation;
        match self {
            Self::Camera(p) => p.show(
                ui,
                CameraPanelInput {
                    cameras: &presentation.cameras,
                    textures: input.resources.camera_textures,
                    preview_textures: input.resources.preview_camera_textures,
                    preview_active: input.preview.active,
                    overlays: input.camera_overlays,
                },
            ),
            Self::Scene(p) => {
                let Some(scene) = input.scene else {
                    return PanelOutput::default();
                };
                p.show(
                    ui,
                    ScenePanelInput {
                        texture_id: input.resources.scene_texture,
                        scan_points: presentation.diagnostics.scan_points,
                        visible_scan_points: scene.visible_scan_points,
                        camera_distance: scene.camera_distance,
                        camera_mode: scene.camera_mode,
                        diagnostics: scene.diagnostics,
                        static_transform_count: scene.static_transform_count,
                        dynamic_transform_count: scene.dynamic_transform_count,
                        playback: input.playback,
                        telemetry: presentation.telemetry.as_ref(),
                        camera: presentation.focused_camera(),
                        source_error: presentation.diagnostics.error.as_deref(),
                    },
                )
            }
            Self::Bev(p) => p.show(
                ui,
                BevPanelInput {
                    texture_id: input.resources.bev_texture,
                    path_points: presentation.diagnostics.path_points,
                },
            ),
            Self::Plot(p) => {
                let id = p.signal_id();
                let signal = input.signals.get(id);
                p.show(
                    ui,
                    PlotPanelInput {
                        playback: input.playback,
                        signal: signal.signal,
                        current: signal.current,
                        loading: signal.loading,
                        error: signal.error,
                        display_time: input.display_time,
                        preview_signal: match id {
                            viewer_core::SignalId::Speed => input.preview.speed_overview,
                            viewer_core::SignalId::YawRate => None,
                        },
                        bookmarks: input.preview.bookmarks,
                    },
                )
            }
            Self::Inspector(p) => p.show(
                ui,
                InspectorPanelInput {
                    inspections: input.inspections,
                    cursor: input.playback.map(|p| p.cursor),
                },
            ),
            Self::Topics(p) => p.show(
                ui,
                TopicsPanelInput {
                    tree: input.topic_tree,
                    series: input.numeric_series,
                    selected_topic: input.inspector_topic,
                    inspector: InspectorPanelInput {
                        inspections: input.topic_inspections,
                        cursor: input.playback.map(|p| p.cursor),
                    },
                    presentation,
                },
            ),
            Self::Status(p) => p.show(
                ui,
                StatusPanelInput {
                    playback: input.playback,
                    main_camera: presentation.focused_camera(),
                    speed: input.display_time.and_then(|time| {
                        input
                            .signals
                            .speed
                            .current
                            .filter(|s| s.arrival_time <= time)
                    }),
                },
            ),
            Self::Placeholder(p) => p.show(ui),
        }
    }
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

    fn panel(kind: &str, config: serde_json::Value) -> Panel {
        Panel::create(&PanelNode {
            id: PanelId::new("test").unwrap(),
            panel_type: kind.into(),
            config_version: 1,
            title: None,
            config,
        })
        .unwrap()
        .unwrap()
    }

    #[test]
    fn independent_query_demands_are_deduplicated_and_never_become_playback_routes() {
        let mut requirements = PanelRequirements::default();
        for p in [
            panel("plot", json!({"signal":"vehicle-speed"})),
            panel("plot", json!({"signal":"yaw-rate"})),
            panel("inspector", json!({"topic":"/custom"})),
            panel("inspector", json!({"topic":"/custom"})),
        ] {
            p.contribute_requirements(&mut requirements);
        }
        assert_eq!(
            requirements.playback,
            viewer_core::PlaybackRequirements::empty()
        );
        assert_eq!(
            requirements.signals,
            std::collections::BTreeSet::from([
                viewer_core::SignalId::Speed,
                viewer_core::SignalId::YawRate
            ])
        );
        assert_eq!(
            requirements.inspections,
            std::collections::BTreeSet::from(["/custom".to_owned()])
        );
    }

    #[test]
    fn source_reset_discards_old_camera_identity_and_retains_configured_binding() {
        let mut p = panel("camera", json!({"cameraTopic":"/configured/image"}));
        let Panel::Camera(camera) = &mut p else {
            unreachable!()
        };
        camera.state.topic = Some("/old/source/image".into());
        camera.state.focused_camera = Some(viewer_core::CameraId(10));
        p.reset_for_source(Some(viewer_core::CameraId(0)));
        let Panel::Camera(camera) = p else {
            unreachable!()
        };
        assert_eq!(camera.state.focused_camera, Some(viewer_core::CameraId(0)));
        assert!(camera.state.topic.is_none());
        assert_eq!(camera.preferred_topic(false), Some("/configured/image"));
    }

    #[test]
    fn status_displays_current_speed_without_a_history_result_and_hides_future_samples() {
        use crate::panel_input::{
            PanelInput, PanelResourceView, PreviewDataView, SignalInput, SignalInputs,
        };
        use viewer_core::{ArrivalTime, SignalSample, TopicTree, ViewerPresentation};
        let mut panel = panel("status", json!({}));
        let mut demand = PanelRequirements::default();
        panel.contribute_requirements(&mut demand);
        let mut expected = viewer_core::PlaybackRequirements::empty();
        expected.require_odometry();
        assert_eq!(demand.playback, expected);
        assert!(demand.signals.is_empty());
        let presentation = ViewerPresentation::default();
        let tree = TopicTree::default();
        let overlays = viewer_renderer::CameraOverlayState::default();
        let mut input = PanelInput {
            playback: None,
            display_time: Some(ArrivalTime(10)),
            presentation: &presentation,
            camera_overlays: &overlays,
            signals: SignalInputs {
                speed: SignalInput {
                    current: Some(SignalSample {
                        arrival_time: ArrivalTime(10),
                        measurement_time: None,
                        value: 4.5,
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            preview: PreviewDataView {
                active: false,
                speed_overview: None,
                bookmarks: &[],
            },
            resources: PanelResourceView {
                camera_textures: &[],
                preview_camera_textures: &[],
                bev_texture: egui::TextureId::Managed(1),
                scene_texture: egui::TextureId::Managed(2),
            },
            scene: None,
            inspections: &[],
            topic_inspections: &[],
            inspector_topic: None,
            topic_tree: &tree,
            numeric_series: None,
        };
        let context = egui::Context::default();
        let mut labels = |input: &PanelInput<'_>| {
            context
                .run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        panel.show(ui, input);
                    });
                })
                .shapes
                .into_iter()
                .filter_map(|s| match s.shape {
                    egui::epaint::Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(labels(&input).iter().any(|s| s == "4.50 m/s"));
        input.display_time = Some(ArrivalTime(9));
        let labels = labels(&input);
        assert!(!labels.iter().any(|s| s == "4.50 m/s"));
        assert!(labels.iter().any(|s| s == "waiting"));
    }

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
