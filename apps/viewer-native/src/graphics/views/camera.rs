use viewer_core::{ArrivalTime, CameraId, CameraPresentation};
use viewer_renderer::{CameraOverlaySnapshot, CameraOverlayState};

#[derive(Clone, Copy)]
pub(crate) struct CameraTextureView {
    pub(crate) camera_id: CameraId,
    pub(crate) texture_id: egui::TextureId,
    pub(crate) arrival_time: ArrivalTime,
    pub(crate) size: (u32, u32),
}

pub(crate) struct CameraViewInput<'a> {
    pub(crate) cameras: &'a [CameraPresentation],
    pub(crate) textures: &'a [CameraTextureView],
    pub(crate) preview_textures: &'a [CameraTextureView],
    pub(crate) preview_active: bool,
    pub(crate) focused_camera: Option<CameraId>,
    pub(crate) show_thumbnails: bool,
    pub(crate) overlays: &'a CameraOverlayState,
    pub(crate) show_overlay: bool,
    pub(crate) heading: &'a str,
}

pub(crate) struct CameraImageInput<'a> {
    pub(crate) camera_id: Option<CameraId>,
    pub(crate) textures: &'a [CameraTextureView],
    pub(crate) preview_textures: &'a [CameraTextureView],
    pub(crate) preview_active: bool,
    pub(crate) overlays: &'a CameraOverlayState,
    pub(crate) show_overlay: bool,
    pub(crate) empty_label: &'a str,
}

#[derive(Default)]
pub(crate) struct CameraViewOutput {
    pub(crate) selected_camera: Option<CameraId>,
}

pub(crate) fn show_camera_view(ui: &mut egui::Ui, input: CameraViewInput<'_>) -> CameraViewOutput {
    let mut output = CameraViewOutput::default();
    let visible_textures = if input.preview_active && !input.preview_textures.is_empty() {
        input.preview_textures
    } else {
        input.textures
    };
    let showing_preview = input.preview_active && !input.preview_textures.is_empty();
    let focused = input
        .focused_camera
        .and_then(|camera_id| input.cameras.iter().find(|camera| camera.id == camera_id));
    let focused_label = focused.map_or("waiting", |camera| camera.topic.as_str());
    let focused_overlay = if input.show_overlay {
        focused.map_or_else(
            || "overlay waiting".to_owned(),
            |camera| camera.overlay.to_string(),
        )
    } else {
        "overlay off".to_owned()
    };

    let source_label = if showing_preview { "PREVIEW" } else { "EXACT" };
    ui.heading(format!(
        "{} · {source_label} · {focused_label} · {focused_overlay}",
        input.heading
    ));
    ui.separator();
    let available = ui.available_size();
    let thumbnail_reserve = if input.show_thumbnails { 70.0 } else { 0.0 };
    let focus_area = egui::vec2(available.x, (available.y - thumbnail_reserve).max(1.0));
    show_camera_image(
        ui,
        focus_area,
        CameraImageInput {
            camera_id: input.focused_camera,
            textures: input.textures,
            preview_textures: input.preview_textures,
            preview_active: input.preview_active,
            overlays: input.overlays,
            show_overlay: input.show_overlay,
            empty_label: "Waiting for camera frame",
        },
    );
    if input.show_thumbnails {
        ui.horizontal_wrapped(|ui| {
            for texture in visible_textures {
                let (width, height) = texture.size;
                let scale = (96.0 / width as f32).min(72.0 / height as f32);
                let size = egui::vec2(
                    width as f32 * scale.max(0.01),
                    height as f32 * scale.max(0.01),
                );
                let response = ui
                    .add(egui::Image::new((texture.texture_id, size)).sense(egui::Sense::click()))
                    .on_hover_text(format!("Focus camera {}", texture.camera_id.0));
                if input.show_overlay && !showing_preview {
                    paint_camera_overlay(ui, response.rect, texture, input.overlays);
                }
                if response.clicked() {
                    output.selected_camera = Some(texture.camera_id);
                }
            }
            if visible_textures.is_empty() && !input.cameras.is_empty() {
                ui.label("Waiting for camera frames…");
            }
        });
    }
    output
}

/// Draws one selected camera into a contained image rect without owning camera state.
///
/// The exact and preview texture caches remain Graphics-owned. Path overlay is applied only to an
/// exact frame whose arrival and dimensions match the CPU-side overlay snapshot.
pub(crate) fn show_camera_image(
    ui: &mut egui::Ui,
    desired_size: egui::Vec2,
    input: CameraImageInput<'_>,
) {
    let preview_texture = input.camera_id.and_then(|camera_id| {
        input
            .preview_textures
            .iter()
            .find(|texture| texture.camera_id == camera_id)
    });
    let showing_preview = input.preview_active && preview_texture.is_some();
    let texture = if showing_preview {
        preview_texture
    } else {
        input.camera_id.and_then(|camera_id| {
            input
                .textures
                .iter()
                .find(|texture| texture.camera_id == camera_id)
        })
    };
    let (container, _) = ui.allocate_exact_size(
        egui::vec2(desired_size.x.max(1.0), desired_size.y.max(1.0)),
        egui::Sense::hover(),
    );
    let Some(texture) = texture else {
        ui.painter()
            .rect_filled(container, 4.0, egui::Color32::from_gray(18));
        ui.painter().text(
            container.center(),
            egui::Align2::CENTER_CENTER,
            input.empty_label,
            egui::FontId::proportional(14.0),
            egui::Color32::GRAY,
        );
        return;
    };
    let image_rect = contain_image_rect(container, texture.size);
    ui.put(
        image_rect,
        egui::Image::new((texture.texture_id, image_rect.size())),
    );
    if input.show_overlay && !showing_preview {
        paint_camera_overlay(ui, image_rect, texture, input.overlays);
    }
}

pub(crate) fn contain_image_rect(container: egui::Rect, image_size: (u32, u32)) -> egui::Rect {
    if image_size.0 == 0
        || image_size.1 == 0
        || container.width() <= 0.0
        || container.height() <= 0.0
    {
        return egui::Rect::from_center_size(container.center(), egui::Vec2::ZERO);
    }
    let scale = (container.width() / image_size.0 as f32)
        .min(container.height() / image_size.1 as f32)
        .max(0.0);
    egui::Rect::from_center_size(
        container.center(),
        egui::vec2(image_size.0 as f32 * scale, image_size.1 as f32 * scale),
    )
}

pub(crate) fn projected_pixel_to_screen(
    pixel: [f32; 2],
    image_size: (u32, u32),
    image_rect: egui::Rect,
) -> egui::Pos2 {
    let scale_x = image_rect.width() / image_size.0.max(1) as f32;
    let scale_y = image_rect.height() / image_size.1.max(1) as f32;
    egui::pos2(
        image_rect.min.x + pixel[0] * scale_x,
        image_rect.min.y + pixel[1] * scale_y,
    )
}

fn paint_camera_overlay(
    ui: &egui::Ui,
    image_rect: egui::Rect,
    texture: &CameraTextureView,
    overlays: &CameraOverlayState,
) {
    let Some(overlay) = overlays.snapshot(texture.camera_id) else {
        return;
    };
    if overlay.camera_arrival != texture.arrival_time || overlay.image_size != texture.size {
        return;
    }
    paint_projected_path(ui, image_rect, overlay);
}

fn paint_projected_path(ui: &egui::Ui, image_rect: egui::Rect, overlay: &CameraOverlaySnapshot) {
    let painter = ui.painter().with_clip_rect(image_rect);
    let display_scale = (image_rect.width() / overlay.image_size.0.max(1) as f32)
        .min(image_rect.height() / overlay.image_size.1.max(1) as f32);
    for pair in overlay.projected_path.windows(2) {
        let [Some(start), Some(end)] = pair else {
            continue;
        };
        let segment = [
            projected_pixel_to_screen(*start, overlay.image_size, image_rect),
            projected_pixel_to_screen(*end, overlay.image_size, image_rect),
        ];
        painter.line_segment(
            segment,
            egui::Stroke::new(
                (5.0 * display_scale).max(1.0),
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180),
            ),
        );
        painter.line_segment(
            segment,
            egui::Stroke::new(
                (2.0 * display_scale).max(0.75),
                egui::Color32::from_rgb(45, 235, 165),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_mapping_accounts_for_letterboxing() {
        let container = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(200.0, 200.0));
        let image_rect = contain_image_rect(container, (400, 200));
        assert_eq!(image_rect.min, egui::pos2(10.0, 70.0));
        assert_eq!(image_rect.max, egui::pos2(210.0, 170.0));
        assert_eq!(
            projected_pixel_to_screen([200.0, 100.0], (400, 200), image_rect),
            egui::pos2(110.0, 120.0)
        );
    }
}
