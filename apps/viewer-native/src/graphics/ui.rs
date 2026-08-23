use super::{Graphics, RenderInput, layout_host::show_layout_host, views::CameraTextureView};
use crate::{
    interaction::ViewerAction,
    panels::{
        PanelCompositionInput, PanelRenderRequests, PanelResourceView, PanelRuntimeStore,
        PreviewDataView, SceneDataView,
    },
    ui_components::{playback_controls, source_status},
    workspace::ViewerInteractionState,
};
use viewer_layout::LayoutDocument;
use winit::window::Window;

pub(super) struct UiOutput {
    pub(super) egui: egui::FullOutput,
    pub(super) actions: Vec<ViewerAction>,
    pub(super) render_requests: PanelRenderRequests,
}

impl Graphics {
    pub(super) fn build_ui(
        &mut self,
        window: &Window,
        render_input: &RenderInput<'_>,
        layout: &LayoutDocument,
        panels: &mut PanelRuntimeStore,
        interaction: &ViewerInteractionState,
    ) -> UiOutput {
        let input = self.egui_state.take_egui_input(window);
        let presentation = render_input.presentation;
        let playback = render_input.playback;
        let mut camera_textures = presentation
            .cameras
            .iter()
            .filter_map(|camera| {
                self.camera_texture(camera.id)
                    .map(|(texture_id, arrival_time, size)| CameraTextureView {
                        camera_id: camera.id,
                        texture_id,
                        arrival_time,
                        size,
                    })
            })
            .collect::<Vec<_>>();
        camera_textures.sort_by_key(|texture| texture.camera_id);
        let mut preview_camera_textures = render_input
            .preview
            .into_iter()
            .flat_map(|snapshot| snapshot.camera_frames())
            .filter_map(|frame| {
                self.preview_camera_texture(frame.camera_id()).map(
                    |(texture_id, arrival_time, size)| CameraTextureView {
                        camera_id: frame.camera_id(),
                        texture_id,
                        arrival_time,
                        size,
                    },
                )
            })
            .collect::<Vec<_>>();
        preview_camera_textures.sort_by_key(|texture| texture.camera_id);

        let resources = PanelResourceView {
            camera_textures: &camera_textures,
            preview_camera_textures: &preview_camera_textures,
            bev_texture: self.bev_texture_id,
            scene_texture: self.scene_texture_id,
        };
        let scene = SceneDataView {
            diagnostics: &render_input.scene.diagnostics,
            visible_scan_points: self.scene_renderer.visible_points(),
            camera_distance: self.scene_renderer.camera().distance,
            camera_mode: self.scene_renderer.camera_mode(),
            static_transform_count: render_input.static_transform_count,
            dynamic_transform_count: render_input.dynamic_transform_count,
        };
        let preview = PreviewDataView {
            active: interaction.preview_time.is_some(),
            speed_overview: render_input.preview_speed,
            bookmarks: render_input.bookmarks,
        };
        let mut actions = Vec::new();
        let mut render_requests = PanelRenderRequests::default();

        let egui = self.egui_context.run(input, |context| {
            if let Some(playback) = playback {
                egui::TopBottomPanel::bottom("playback-controls").show(context, |ui| {
                    actions.extend(
                        playback_controls(ui, playback)
                            .commands
                            .into_iter()
                            .map(ViewerAction::Playback),
                    );
                });
            } else {
                egui::TopBottomPanel::bottom("live-status").show(context, |ui| {
                    ui.label("Live mode · timeline and playback clock disabled");
                });
            }
            egui::SidePanel::left("source-status")
                .resizable(true)
                .default_width(260.0)
                .show(context, |ui| source_status(ui, presentation));
            egui::CentralPanel::default().show(context, |ui| {
                let output = show_layout_host(
                    ui,
                    layout,
                    panels,
                    PanelCompositionInput {
                        playback,
                        presentation,
                        camera_overlays: render_input.camera_overlays,
                        interaction,
                        signals: render_input.signals,
                        preview,
                        resources,
                        scene,
                        inspections: render_input.inspections,
                    },
                );
                actions.extend(output.actions);
                render_requests = output.render_requests;
            });
        });
        UiOutput {
            egui,
            actions,
            render_requests,
        }
    }
}
