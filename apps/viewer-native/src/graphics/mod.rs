mod camera;
mod surface;
mod ui;

use crate::session::PlaybackSession;
use bev_renderer::{BevFrame, BevRenderer};
use egui_wgpu::Renderer as EguiRenderer;
use scene_renderer::{SceneFrame, SceneRenderer};
use std::{collections::BTreeMap, sync::Arc};
use viewer_core::{
    BevFrameBuilder, CameraCalibrationSet, CameraId, OverlayStatus, PresentationMetrics,
    SceneFrameBuilder,
};
use viewer_renderer::CameraTextureSlot;
use winit::window::Window;

pub(crate) struct Graphics {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub(crate) config: wgpu::SurfaceConfiguration,
    egui_context: egui::Context,
    pub(crate) egui_state: egui_winit::State,
    egui_renderer: EguiRenderer,
    camera_slots: BTreeMap<CameraId, CameraTextureSlot>,
    camera_texture_ids: BTreeMap<CameraId, egui::TextureId>,
    uploaded_arrivals: BTreeMap<CameraId, i64>,
    camera_topics: Arc<Vec<(CameraId, String)>>,
    pub(crate) focused_camera: Option<CameraId>,
    calibrations: CameraCalibrationSet,
    overlay_status: BTreeMap<CameraId, OverlayStatus>,
    pub(crate) presentation_metrics: PresentationMetrics,
    bev_renderer: BevRenderer,
    bev_texture_id: egui::TextureId,
    scene_builder: SceneFrameBuilder,
    scene_renderer: SceneRenderer,
    scene_texture_id: egui::TextureId,
    accumulate_points: bool,
}

impl Graphics {
    pub(crate) fn render(
        &mut self,
        window: &Window,
        session: &mut PlaybackSession,
        error: Option<String>,
    ) -> Result<bool, wgpu::SurfaceError> {
        let ui = self.build_ui(window, session, error);
        self.accumulate_points = ui.accumulate_points;
        self.scene_renderer.set_camera_mode(ui.scene_camera_mode);
        if ui.scene_wheel_delta != 0.0 {
            self.scene_renderer.zoom(ui.scene_wheel_delta);
        }
        if ui.scene_orbit_delta != egui::Vec2::ZERO {
            self.scene_renderer
                .orbit(ui.scene_orbit_delta.x, ui.scene_orbit_delta.y);
        }
        if ui.reset_scene_camera {
            self.scene_renderer.reset_camera();
        }
        let pixels_per_point = ui.egui.pixels_per_point;
        self.sync_bev(ui.bev_size, session, pixels_per_point);
        self.sync_scene(ui.scene_size, session, pixels_per_point);
        self.paint_egui(window, ui.egui)?;
        Ok(ui.seeked)
    }

    fn sync_bev(
        &mut self,
        logical_size: egui::Vec2,
        session: &PlaybackSession,
        pixels_per_point: f32,
    ) {
        let (bev_width, bev_height) = texture_size(logical_size, pixels_per_point);
        let bev_resized = self
            .bev_renderer
            .resize(&self.device, bev_width, bev_height);
        if bev_resized {
            self.egui_renderer.update_egui_texture_from_wgpu_texture(
                &self.device,
                self.bev_renderer.view(),
                wgpu::FilterMode::Linear,
                self.bev_texture_id,
            );
        }
        let snapshot = BevFrameBuilder::new(session.state()).build();
        let frame = BevFrame {
            revision: snapshot.revision,
            path: snapshot.path,
        };
        if bev_resized || self.bev_renderer.needs_render(frame) {
            self.bev_renderer.render(&self.device, &self.queue, frame);
        }
    }

    fn sync_scene(
        &mut self,
        logical_size: egui::Vec2,
        session: &PlaybackSession,
        pixels_per_point: f32,
    ) {
        let (scene_width, scene_height) = texture_size(logical_size, pixels_per_point);
        let scene_resized = self
            .scene_renderer
            .resize(&self.device, scene_width, scene_height);
        if scene_resized {
            self.egui_renderer.update_egui_texture_from_wgpu_texture(
                &self.device,
                self.scene_renderer.view(),
                wgpu::FilterMode::Linear,
                self.scene_texture_id,
            );
        }
        let snapshot = self
            .scene_builder
            .build(session.state(), self.accumulate_points);
        let frame = SceneFrame {
            revision: snapshot.revision,
            cloud_revision: snapshot.cloud_revision,
            ego_position: snapshot.ego_position,
            ego_yaw: snapshot.ego_yaw,
            path: snapshot.path,
            cloud: snapshot.cloud,
            accumulate: snapshot.accumulate,
        };
        if scene_resized || self.scene_renderer.needs_render(frame) {
            self.scene_renderer.render(&self.device, &self.queue, frame);
        }
    }

    pub(crate) fn clear_scene_history(&mut self) {
        self.scene_builder.reset();
        self.scene_renderer.clear_cloud_history();
    }
}

fn texture_size(logical_size: egui::Vec2, pixels_per_point: f32) -> (u32, u32) {
    (
        (logical_size.x * pixels_per_point)
            .round()
            .clamp(1.0, 4096.0) as u32,
        (logical_size.y * pixels_per_point)
            .round()
            .clamp(1.0, 4096.0) as u32,
    )
}
