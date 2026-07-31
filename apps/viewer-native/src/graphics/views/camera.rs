use crate::interaction::ViewerAction;
use viewer_core::{CameraId, CameraPresentation};

#[derive(Clone, Copy)]
pub(crate) struct CameraTextureView {
    pub(crate) camera_id: CameraId,
    pub(crate) texture_id: egui::TextureId,
    pub(crate) size: (u32, u32),
}

pub(crate) struct CameraViewInput<'a> {
    pub(crate) cameras: &'a [CameraPresentation],
    pub(crate) textures: &'a [CameraTextureView],
    pub(crate) focused_camera: Option<CameraId>,
}

#[derive(Default)]
pub(crate) struct CameraViewOutput {
    pub(crate) actions: Vec<ViewerAction>,
}

pub(crate) fn show_camera_view(ui: &mut egui::Ui, input: CameraViewInput<'_>) -> CameraViewOutput {
    let mut output = CameraViewOutput::default();
    let focused = input
        .focused_camera
        .and_then(|camera_id| input.cameras.iter().find(|camera| camera.id == camera_id));
    let focused_texture = input.focused_camera.and_then(|camera_id| {
        input
            .textures
            .iter()
            .find(|texture| texture.camera_id == camera_id)
    });
    let focused_label = focused.map_or("waiting", |camera| camera.topic.as_str());
    let focused_overlay = focused.map_or_else(
        || "overlay waiting".to_owned(),
        |camera| camera.overlay.to_string(),
    );

    ui.heading(format!(
        "CAMERA FOCUS · {focused_label} · {focused_overlay}"
    ));
    ui.separator();
    if let Some(texture) = focused_texture {
        let (width, height) = texture.size;
        let available = ui.available_size();
        let scale = (available.x / width as f32)
            .min((available.y - 70.0).max(1.0) / height as f32)
            .max(0.0);
        let size = egui::vec2(width as f32 * scale, height as f32 * scale);
        let focus_area = egui::vec2(available.x, (available.y - 70.0).max(1.0));
        ui.allocate_ui(focus_area, |ui| {
            ui.centered_and_justified(|ui| {
                ui.add(egui::Image::new((texture.texture_id, size)))
                    .on_hover_text("Focused camera");
            });
        });
    } else {
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.label("Waiting for camera frame");
            });
        });
    }
    ui.horizontal_wrapped(|ui| {
        for texture in input.textures {
            let (width, height) = texture.size;
            let scale = (96.0 / width as f32).min(72.0 / height as f32);
            let size = egui::vec2(
                width as f32 * scale.max(0.01),
                height as f32 * scale.max(0.01),
            );
            let response = ui
                .add(egui::Image::new((texture.texture_id, size)).sense(egui::Sense::click()))
                .on_hover_text(format!("Focus camera {}", texture.camera_id.0));
            if response.clicked() {
                output
                    .actions
                    .push(ViewerAction::SetFocusedCamera(Some(texture.camera_id)));
            }
        }
        if input.textures.is_empty() && !input.cameras.is_empty() {
            ui.label("Waiting for camera frames…");
        }
    });
    output
}
