use super::{Graphics, RenderInput, layout_host::show_layout_host, views::CameraTextureView};
use crate::{
    interaction::ViewerAction,
    panels::{
        PanelFrameContext, PanelRenderRequests, PanelResourceView, PlotDataView, SceneDataView,
    },
    workspace::NativeWorkspace,
};
use viewer_ui::{playback_controls, source_status};
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
        workspace: &mut NativeWorkspace,
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

        let resources = PanelResourceView {
            camera_textures: &camera_textures,
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
        let plot = PlotDataView {
            signal: render_input.speed_signal,
            loading: render_input.plot_loading,
            error: render_input.plot_error,
        };
        let layout = &workspace.layout;
        let interaction = &workspace.interaction;
        let panels = &mut workspace.panels;
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
                    PanelFrameContext {
                        playback,
                        presentation,
                        camera_overlays: render_input.camera_overlays,
                        interaction,
                        plot,
                        resources,
                        scene,
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
