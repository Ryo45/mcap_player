//! Scene controls and presentation. GPU resources remain host-owned.
use crate::{
    action::ViewerAction,
    panel::{PanelOutput, PanelRenderRequests},
    views::scene::{SceneViewInput, show_scene_view},
};
use scene_renderer::SceneCameraMode;
use serde::{Deserialize, Serialize};
use viewer_core::{PointCloudColorMode, PointCloudColoring, SceneDiagnostics};
use viewer_layout::{PanelId, PanelNode};

pub const SCENE_CONFIG_VERSION: u32 = 1;

#[derive(Clone, Copy)]
pub struct ScenePanelInput<'a> {
    pub texture_id: egui::TextureId,
    pub scan_points: usize,
    pub visible_scan_points: usize,
    pub camera_distance: f32,
    pub camera_mode: SceneCameraMode,
    pub diagnostics: &'a SceneDiagnostics,
    pub static_transform_count: usize,
    pub dynamic_transform_count: usize,
    pub playback: Option<viewer_core::PlaybackView>,
    pub telemetry: Option<&'a viewer_core::TelemetryPresentation>,
    pub camera: Option<&'a viewer_core::CameraPresentation>,
    pub source_error: Option<&'a str>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScenePanelConfig {
    #[serde(default)]
    show_telemetry: bool,
    #[serde(default)]
    optional_data: bool,
    #[serde(default)]
    coloring: PointCloudColoring,
}

pub struct ScenePanel {
    id: PanelId,
    title: Option<String>,
    config: ScenePanelConfig,
    pub state: SceneViewState,
}

impl ScenePanel {
    pub fn create(node: &PanelNode) -> Result<Self, String> {
        if node.config_version != SCENE_CONFIG_VERSION {
            return Err(format!(
                "Unsupported {} config version {}; expected {}",
                node.panel_type, node.config_version, SCENE_CONFIG_VERSION
            ));
        }
        match serde_json::from_value::<ScenePanelConfig>(node.config.clone()) {
            Ok(config) if !config.coloring.is_valid() => {
                Err("Invalid point cloud intensity range".into())
            }
            Ok(config) => Ok(Self {
                id: node.id.clone(),
                title: node.title.clone(),
                state: SceneViewState {
                    coloring: config.coloring,
                    ..Default::default()
                },
                config,
            }),
            Err(error) => Err(format!("Invalid scene config: {error}")),
        }
    }

    pub fn contribute_data_requirements(
        &self,
        requirements: &mut viewer_core::PlaybackRequirements,
    ) {
        requirements.optional_path();
        requirements.optional_odometry();
        if self.config.optional_data {
            requirements.optional_point_cloud();
            requirements.optional_transforms();
        } else {
            requirements.require_point_cloud();
            requirements.require_transforms();
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, input: ScenePanelInput<'_>) -> PanelOutput {
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
                render_requests: PanelRenderRequests {
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneViewState {
    pub coloring: viewer_core::PointCloudColoring,
    pub accumulate_points: bool,
}
