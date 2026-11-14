pub struct BevViewInput {
    pub texture_id: egui::TextureId,
    pub path_points: usize,
}

pub struct BevViewOutput {
    pub logical_size: egui::Vec2,
}

pub fn show_bev_view(ui: &mut egui::Ui, input: BevViewInput) -> BevViewOutput {
    ui.heading(format!("BEV · PATH {} pts", input.path_points));
    ui.separator();
    let logical_size = ui.available_size().max(egui::vec2(1.0, 1.0));
    ui.add(egui::Image::new((input.texture_id, logical_size)));
    BevViewOutput { logical_size }
}
