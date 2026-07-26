mod camera;
mod surface;
mod ui;

use crate::session::PlaybackSession;
use bev_renderer::BevFrame;
use bev_renderer::BevRenderer;
use egui_wgpu::Renderer as EguiRenderer;
use scene_renderer::{SceneFrame, SceneRenderer};
use std::{collections::BTreeMap, sync::Arc};
use viewer_core::{CameraCalibrationSet, CameraId, OverlayStatus, PresentationMetrics};
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
        let bev_frame = BevFrame {
            revision: session.state().bev.revision(),
            path: self.bev_path_points(session).unwrap_or(&[]),
        };
        if bev_resized || self.bev_renderer.needs_render(bev_frame) {
            self.bev_renderer
                .render(&self.device, &self.queue, bev_frame);
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
        let telemetry = session.state().telemetry.latest();
        let telemetry_revision = telemetry.map_or(0, |frame| frame.arrival_time.0 as u64);
        let scene_frame = SceneFrame {
            revision: session.state().bev.revision().rotate_left(17) ^ telemetry_revision,
            cloud_revision: session.state().point_cloud.revision(),
            ego_position: telemetry.map_or([0.0, 0.0], |frame| {
                [frame.position_x as f32, frame.position_y as f32]
            }),
            ego_yaw: telemetry.map_or(0.0, |frame| frame.yaw_radians as f32),
            path: self.bev_path_points(session).unwrap_or(&[]),
            cloud: session
                .state()
                .point_cloud
                .latest()
                .map_or(&[], |frame| frame.points.as_slice()),
            accumulate: self.accumulate_points,
        };
        if scene_resized || self.scene_renderer.needs_render(scene_frame) {
            self.scene_renderer
                .render(&self.device, &self.queue, scene_frame);
        }
    }

    fn bev_path_points<'a>(&self, session: &'a PlaybackSession) -> Option<&'a [[f32; 2]]> {
        session
            .state()
            .bev
            .latest()
            .map(|frame| frame.points.as_slice())
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
