mod camera;
mod layout_host;
mod surface;
mod ui;
pub(crate) mod views;

use crate::{
    inspection::TopicInspection, interaction::ViewerAction, presentation::PresentationTransition,
    workspace::NativeWorkspace,
};
use bev_renderer::{BevFrame, BevRenderer};
use egui_wgpu::Renderer as EguiRenderer;
use scene_renderer::{SceneCameraMode, SceneFrame, SceneRenderer};
use std::collections::BTreeMap;
use viewer_core::{
    BevSnapshot, Bookmark, CameraId, LoadedSignal, PlaybackView, PreviewSnapshot, SceneSnapshot,
    SignalOverview, ViewerPresentation,
};
use viewer_renderer::{CameraBaseImageTracker, CameraTextureSlot};
use winit::window::Window;

pub(crate) struct RenderInput<'a> {
    pub(crate) presentation: &'a ViewerPresentation,
    pub(crate) camera_overlays: &'a viewer_renderer::CameraOverlayState,
    pub(crate) playback: Option<PlaybackView>,
    pub(crate) speed_signal: Option<&'a LoadedSignal>,
    pub(crate) plot_loading: bool,
    pub(crate) plot_error: Option<&'a str>,
    pub(crate) inspections: &'a [TopicInspection],
    pub(crate) preview: Option<&'a PreviewSnapshot>,
    pub(crate) preview_speed: Option<&'a SignalOverview>,
    pub(crate) bookmarks: &'a [Bookmark],
    pub(crate) bev: BevSnapshot<'a>,
    pub(crate) scene: &'a SceneSnapshot<'a>,
    pub(crate) static_transform_count: usize,
    pub(crate) dynamic_transform_count: usize,
}

pub(crate) struct RenderOutput {
    pub(crate) actions: Vec<ViewerAction>,
}

#[derive(Clone, Copy)]
pub(crate) struct ViewRenderRequests {
    pub(crate) bev_size: Option<egui::Vec2>,
    pub(crate) scene: Option<SceneRenderRequest>,
}

#[derive(Clone, Copy)]
pub(crate) struct SceneRenderRequest {
    pub(crate) logical_size: egui::Vec2,
    pub(crate) wheel_delta: f32,
    pub(crate) orbit_delta: egui::Vec2,
    pub(crate) reset_camera: bool,
    pub(crate) camera_mode: SceneCameraMode,
}

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
    camera_base_images: CameraBaseImageTracker,
    preview_camera_slots: BTreeMap<CameraId, CameraTextureSlot>,
    preview_camera_texture_ids: BTreeMap<CameraId, egui::TextureId>,
    preview_camera_keys: BTreeMap<CameraId, (viewer_core::ArrivalTime, u32, u32)>,
    bev_renderer: BevRenderer,
    bev_texture_id: egui::TextureId,
    scene_renderer: SceneRenderer,
    scene_texture_id: egui::TextureId,
}

impl Graphics {
    pub(crate) fn apply_transition(&mut self, transition: PresentationTransition) {
        self.hide_camera();
        self.clear_scene_history();
        if transition == PresentationTransition::SourceChanged {
            self.clear_preview();
        }
    }

    pub(crate) fn render(
        &mut self,
        window: &Window,
        input: RenderInput<'_>,
        workspace: &mut NativeWorkspace,
    ) -> Result<RenderOutput, wgpu::SurfaceError> {
        let ui = self.build_ui(window, &input, workspace);
        let view_requests = ViewRenderRequests {
            bev_size: ui.render_requests.bev_size,
            scene: ui.render_requests.scene.map(|scene| SceneRenderRequest {
                logical_size: scene.logical_size,
                wheel_delta: scene.wheel_delta,
                orbit_delta: scene.orbit_delta,
                reset_camera: scene.reset_camera,
                camera_mode: scene.camera_mode,
            }),
        };
        if let Some(scene) = view_requests.scene {
            self.apply_scene_request(scene);
        }
        let pixels_per_point = ui.egui.pixels_per_point;
        if let Some(bev_size) = view_requests.bev_size {
            self.sync_bev(bev_size, input.bev, pixels_per_point);
        }
        if let Some(scene) = view_requests.scene {
            self.sync_scene(scene.logical_size, input.scene, pixels_per_point);
        }
        self.paint_egui(window, ui.egui)?;
        Ok(RenderOutput {
            actions: ui.actions,
        })
    }

    fn apply_scene_request(&mut self, request: SceneRenderRequest) {
        self.scene_renderer.set_camera_mode(request.camera_mode);
        if request.wheel_delta != 0.0 {
            self.scene_renderer.zoom(request.wheel_delta);
        }
        if request.orbit_delta != egui::Vec2::ZERO {
            self.scene_renderer
                .orbit(request.orbit_delta.x, request.orbit_delta.y);
        }
        if request.reset_camera {
            self.scene_renderer.reset_camera();
        }
    }

    fn sync_bev(
        &mut self,
        logical_size: egui::Vec2,
        snapshot: BevSnapshot<'_>,
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
        snapshot: &SceneSnapshot<'_>,
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
