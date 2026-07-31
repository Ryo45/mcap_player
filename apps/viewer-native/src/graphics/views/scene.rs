use crate::interaction::ViewerAction;
use scene_renderer::SceneCameraMode;
use viewer_core::SceneDiagnostics;

pub(crate) struct SceneViewInput<'a> {
    pub(crate) texture_id: egui::TextureId,
    pub(crate) scan_points: usize,
    pub(crate) visible_scan_points: usize,
    pub(crate) camera_distance: f32,
    pub(crate) camera_mode: SceneCameraMode,
    pub(crate) accumulate_points: bool,
    pub(crate) diagnostics: &'a SceneDiagnostics,
    pub(crate) static_transform_count: usize,
    pub(crate) dynamic_transform_count: usize,
}

pub(crate) struct SceneViewOutput {
    pub(crate) actions: Vec<ViewerAction>,
    pub(crate) logical_size: egui::Vec2,
    pub(crate) wheel_delta: f32,
    pub(crate) orbit_delta: egui::Vec2,
    pub(crate) reset_camera: bool,
    pub(crate) camera_mode: SceneCameraMode,
}

pub(crate) fn show_scene_view(ui: &mut egui::Ui, input: SceneViewInput<'_>) -> SceneViewOutput {
    let mut actions = Vec::new();
    let mut camera_mode = input.camera_mode;
    let tf_status = if let Some(error) = &input.diagnostics.current_tf_error {
        format!(
            "TF missing {} → {} · misses {}",
            error.source_frame, error.target_frame, input.diagnostics.tf_misses
        )
    } else {
        input.diagnostics.last_tf_route.as_ref().map_or_else(
            || format!("TF waiting · misses {}", input.diagnostics.tf_misses),
            |route| {
                format!(
                    "TF {route} · static {} dynamic {} · misses {}",
                    input.static_transform_count,
                    input.dynamic_transform_count,
                    input.diagnostics.tf_misses
                )
            },
        )
    };

    ui.horizontal(|ui| {
        ui.heading(format!("3D VIEW · SCAN {} pts", input.scan_points));
        ui.separator();
        egui::ComboBox::from_id_salt("scene-camera-mode")
            .selected_text(camera_mode.label())
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut camera_mode,
                    SceneCameraMode::Chase,
                    SceneCameraMode::Chase.label(),
                );
                ui.selectable_value(
                    &mut camera_mode,
                    SceneCameraMode::Free,
                    SceneCameraMode::Free.label(),
                );
                ui.selectable_value(
                    &mut camera_mode,
                    SceneCameraMode::VehicleEye,
                    SceneCameraMode::VehicleEye.label(),
                );
            });
        let mut selected_accumulation = input.accumulate_points;
        if ui
            .checkbox(&mut selected_accumulation, "Accumulate scans")
            .changed()
        {
            actions.push(ViewerAction::SetAccumulatePoints(selected_accumulation));
        }
        if input.accumulate_points {
            ui.label(format!("visible {}", input.visible_scan_points));
        }
        ui.label(format!("camera {:.1} m", input.camera_distance));
        ui.label(tf_status);
    });

    let logical_size = ui.available_size().max(egui::vec2(1.0, 1.0));
    let response = ui
        .add(egui::Image::new((input.texture_id, logical_size)).sense(egui::Sense::drag()))
        .on_hover_text(match camera_mode {
            SceneCameraMode::Chase => {
                "Vehicle-following chase view · Wheel: zoom · Double-click: reset"
            }
            SceneCameraMode::Free => "Free view · Wheel: zoom · Drag: orbit · Double-click: reset",
            SceneCameraMode::VehicleEye => "Forward view from the vehicle",
        });
    let wheel_delta = if response.hovered() && camera_mode != SceneCameraMode::VehicleEye {
        ui.input(|input| input.smooth_scroll_delta.y)
    } else {
        0.0
    };
    let orbit_delta = if camera_mode == SceneCameraMode::Free
        && response.dragged_by(egui::PointerButton::Primary)
    {
        ui.input(|input| input.pointer.delta())
    } else {
        egui::Vec2::ZERO
    };

    SceneViewOutput {
        actions,
        logical_size,
        wheel_delta,
        orbit_delta,
        reset_camera: response.double_clicked(),
        camera_mode,
    }
}
