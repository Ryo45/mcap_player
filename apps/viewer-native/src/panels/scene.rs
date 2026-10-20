use super::{
    NativePanel, PanelDataRequirements, PanelOutput, PlaceholderPanel, SCENE_CONFIG_VERSION,
};
use crate::{
    graphics::views::{SceneViewInput, show_scene_view},
    interaction::ViewerAction,
    workspace::SceneViewState,
};
use scene_renderer::SceneCameraMode;
use serde::{Deserialize, Serialize};
use viewer_core::{PointCloudColorMode, PointCloudColoring, SceneDiagnostics};
use viewer_layout::{PanelId, PanelNode};

#[derive(Clone, Copy)]
pub(crate) struct ScenePanelInput<'a> {
    pub(crate) texture_id: egui::TextureId,
    pub(crate) scan_points: usize,
    pub(crate) visible_scan_points: usize,
    pub(crate) camera_distance: f32,
    pub(crate) camera_mode: SceneCameraMode,
    pub(crate) diagnostics: &'a SceneDiagnostics,
    pub(crate) static_transform_count: usize,
    pub(crate) dynamic_transform_count: usize,
    pub(crate) playback: Option<viewer_core::PlaybackView>,
    pub(crate) telemetry: Option<&'a viewer_core::TelemetryPresentation>,
    pub(crate) camera: Option<&'a viewer_core::CameraPresentation>,
    pub(crate) source_error: Option<&'a str>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScenePanelConfig {
    #[serde(default)]
    show_telemetry: bool,
    #[serde(default)]
    optional_data: bool,
    #[serde(default)]
    coloring: PointCloudColoring,
}

pub(crate) struct ScenePanel {
    id: PanelId,
    title: Option<String>,
    config: ScenePanelConfig,
    pub(crate) state: SceneViewState,
}

impl ScenePanel {
    pub(crate) fn create(node: &PanelNode) -> NativePanel {
        if node.config_version != SCENE_CONFIG_VERSION {
            return NativePanel::Placeholder(PlaceholderPanel::unsupported_version(
                node,
                SCENE_CONFIG_VERSION,
            ));
        }
        match serde_json::from_value::<ScenePanelConfig>(node.config.clone()) {
            Ok(config) if !config.coloring.is_valid() => {
                NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                    node,
                    "Invalid point cloud intensity range".into(),
                ))
            }
            Ok(config) => NativePanel::Scene(Self {
                id: node.id.clone(),
                title: node.title.clone(),
                state: SceneViewState {
                    coloring: config.coloring,
                    ..Default::default()
                },
                config,
            }),
            Err(error) => NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                format!("Invalid scene config: {error}"),
            )),
        }
    }

    pub(crate) fn contribute_data_requirements(&self, requirements: &mut PanelDataRequirements) {
        requirements.playback.optional_path();
        requirements.playback.optional_odometry();
        if self.config.optional_data {
            requirements.playback.optional_point_cloud();
            requirements.playback.optional_transforms();
        } else {
            requirements.playback.require_point_cloud();
            requirements.playback.require_transforms();
        }
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, input: ScenePanelInput<'_>) -> PanelOutput {
        ui.push_id((self.id.as_str(), self.title.as_deref()), |ui| {
            if self.config.show_telemetry {
                show_monitor_telemetry(ui, &input);
            }
            ui.horizontal_wrapped(|ui| {
                let mut coloring = self.state.coloring;
                egui::ComboBox::from_id_salt("point-cloud-color")
                    .selected_text(coloring.mode.label())
                    .show_ui(ui, |ui| {
                        for mode in [
                            PointCloudColorMode::Auto,
                            PointCloudColorMode::Solid,
                            PointCloudColorMode::Rgb,
                            PointCloudColorMode::Intensity,
                        ] {
                            ui.selectable_value(&mut coloring.mode, mode, mode.label());
                        }
                    });
                if matches!(
                    coloring.mode,
                    PointCloudColorMode::Auto | PointCloudColorMode::Intensity
                ) {
                    ui.label("Intensity range");
                    ui.add(egui::DragValue::new(&mut coloring.intensity_range[0]).speed(0.1));
                    ui.add(egui::DragValue::new(&mut coloring.intensity_range[1]).speed(0.1));
                }
                if coloring.is_valid() {
                    self.state.coloring = coloring;
                }
            });
            let output = show_scene_view(
                ui,
                SceneViewInput {
                    texture_id: input.texture_id,
                    scan_points: input.scan_points,
                    visible_scan_points: input.visible_scan_points,
                    camera_distance: input.camera_distance,
                    camera_mode: input.camera_mode,
                    accumulate_points: self.state.accumulate_points,
                    diagnostics: input.diagnostics,
                    static_transform_count: input.static_transform_count,
                    dynamic_transform_count: input.dynamic_transform_count,
                },
            );
            let mut actions = Vec::new();
            if let Some(accumulate) = output.selected_accumulation {
                actions.push(ViewerAction::SetAccumulatePoints {
                    panel_id: self.id.clone(),
                    accumulate,
                });
            }
            PanelOutput {
                actions,
                render_requests: super::PanelRenderRequests {
                    bev_size: None,
                    scene: Some(output),
                    ..Default::default()
                },
            }
        })
        .inner
    }
}

fn show_monitor_telemetry(ui: &mut egui::Ui, input: &ScenePanelInput<'_>) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong(input.playback.map_or("LIVE", |view| {
                if view.playing { "PLAYING" } else { "PAUSED" }
            }));
            if let Some(view) = input.playback {
                ui.monospace(format!(
                    "{:.3} s · {}",
                    view.cursor.0.saturating_sub(view.start.0) as f64 / 1e9,
                    view.speed.label()
                ));
            }
            ui.separator();
            if let Some(telemetry) = input.telemetry {
                ui.monospace(format!(
                    "{:.2} m/s · {:.1} km/h",
                    telemetry.speed,
                    telemetry.speed * 3.6
                ));
                ui.monospace(format!(
                    "Yaw rate {:+.1} °/s",
                    telemetry.yaw_rate.to_degrees()
                ));
            } else {
                ui.label("Odometry unavailable");
            }
            ui.separator();
            if let Some(camera) = input.camera {
                ui.label(format!(
                    "Camera {:?} · {:.1} Hz · overlay {}",
                    camera.status, camera.fps, camera.overlay
                ));
            } else {
                ui.label("Camera unavailable");
            }
        });
        if let Some(error) = input.source_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    });
}
