use super::{
    Graphics, RenderInput,
    views::{
        BevViewInput, CameraTextureView, CameraViewInput, PlotViewInput, SceneViewInput,
        SceneViewOutput, show_bev_view, show_camera_view, show_plot_view, show_scene_view,
    },
};
use crate::{interaction::ViewerAction, workspace::WorkspaceState};
use scene_renderer::SceneCameraMode;
use viewer_ui::{playback_controls, source_status};
use winit::window::Window;

pub(super) struct UiOutput {
    pub(super) egui: egui::FullOutput,
    pub(super) actions: Vec<ViewerAction>,
    pub(super) bev_size: egui::Vec2,
    pub(super) scene: SceneViewOutput,
}

impl Graphics {
    pub(super) fn build_ui(
        &mut self,
        window: &Window,
        render_input: &RenderInput<'_>,
        workspace: &mut WorkspaceState,
    ) -> UiOutput {
        let input = self.egui_state.take_egui_input(window);
        let presentation = render_input.presentation;
        let playback = render_input.playback;
        let mut camera_textures = presentation
            .cameras
            .iter()
            .filter_map(|camera| {
                self.camera_texture(camera.id)
                    .map(|(texture_id, size)| CameraTextureView {
                        camera_id: camera.id,
                        texture_id,
                        size,
                    })
            })
            .collect::<Vec<_>>();
        camera_textures.sort_by_key(|texture| texture.camera_id);

        let bev_texture_id = self.bev_texture_id;
        let scene_texture_id = self.scene_texture_id;
        let visible_scan_points = self.scene_renderer.visible_points();
        let scene_camera_distance = self.scene_renderer.camera().distance;
        let scene_camera_mode = self.scene_renderer.camera_mode();
        let mut actions = Vec::new();
        let mut bev_size = egui::Vec2::ZERO;
        let mut scene_output = empty_scene_output(scene_camera_mode);

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
                let top_size = egui::vec2(ui.available_width(), ui.available_height() * 0.36);
                ui.allocate_ui(top_size, |ui| {
                    ui.columns(2, |columns| {
                        let camera = show_camera_view(
                            &mut columns[0],
                            CameraViewInput {
                                cameras: &presentation.cameras,
                                textures: &camera_textures,
                                focused_camera: workspace.camera.focused_camera,
                            },
                        );
                        actions.extend(camera.actions);
                        bev_size = show_bev_view(
                            &mut columns[1],
                            BevViewInput {
                                texture_id: bev_texture_id,
                                path_points: presentation.diagnostics.path_points,
                            },
                        )
                        .logical_size;
                    });
                });
                ui.separator();
                if let Some(playback) = playback {
                    let plot = show_plot_view(
                        ui,
                        PlotViewInput {
                            signal: render_input.speed_signal,
                            loading: render_input.plot_loading,
                            error: render_input.plot_error,
                            playback,
                            display_time: workspace.interaction.display_time(playback),
                        },
                        &mut workspace.plot,
                    );
                    actions.extend(plot.actions);
                    ui.separator();
                }
                scene_output = show_scene_view(
                    ui,
                    SceneViewInput {
                        texture_id: scene_texture_id,
                        scan_points: presentation.diagnostics.scan_points,
                        visible_scan_points,
                        camera_distance: scene_camera_distance,
                        camera_mode: scene_camera_mode,
                        accumulate_points: workspace.scene.accumulate_points,
                        diagnostics: &render_input.scene.diagnostics,
                        static_transform_count: render_input.static_transform_count,
                        dynamic_transform_count: render_input.dynamic_transform_count,
                    },
                );
                actions.append(&mut scene_output.actions);
            });
        });
        UiOutput {
            egui,
            actions,
            bev_size,
            scene: scene_output,
        }
    }
}

fn empty_scene_output(camera_mode: SceneCameraMode) -> SceneViewOutput {
    SceneViewOutput {
        actions: Vec::new(),
        logical_size: egui::Vec2::ZERO,
        wheel_delta: 0.0,
        orbit_delta: egui::Vec2::ZERO,
        reset_camera: false,
        camera_mode,
    }
}
