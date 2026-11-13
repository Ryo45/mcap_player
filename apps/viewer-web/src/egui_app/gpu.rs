//! Browser image import and offscreen scene textures; egui owns compositing and input.
use eframe::egui_wgpu::RenderState;
use scene_renderer::{SceneFrame, SceneRenderer};
use std::collections::BTreeMap;
use viewer_core::{CameraFrameMetadata, CameraId};

struct CameraTexture {
    texture: wgpu::Texture,
    id: egui::TextureId,
    size: (u32, u32),
}

pub(super) struct GpuViews {
    pub state: RenderState,
    pub scene: SceneRenderer,
    pub scene_id: egui::TextureId,
    cameras: BTreeMap<CameraId, CameraTexture>,
    pub uploads: u64,
    pub software: bool,
    pub device_type: String,
}
impl GpuViews {
    pub fn new(state: RenderState) -> Self {
        let info = state.adapter.get_info();
        let software = info.device_type == wgpu::DeviceType::Cpu;
        let device_type = format!("{:?}", info.device_type);
        let scene = SceneRenderer::new(&state.device, 768, 384);
        let scene_id = state.renderer.write().register_native_texture(
            &state.device,
            &scene.encoded_view(),
            wgpu::FilterMode::Linear,
        );
        Self {
            state,
            scene,
            scene_id,
            cameras: BTreeMap::new(),
            uploads: 0,
            software,
            device_type,
        }
    }
    pub fn reset(&mut self) {
        for image in self.cameras.values() {
            self.state.renderer.write().free_texture(&image.id);
        }
        self.cameras.clear();
        self.scene.invalidate();
    }
    pub fn import(
        &mut self,
        frame: CameraFrameMetadata,
        size: (u32, u32),
        bitmap: &web_sys::ImageBitmap,
    ) -> Result<(), String> {
        // SwiftShader WebGPU rejects Worker ImageBitmap copies in this environment.
        // Keep the UI and Inspection usable rather than entering wgpu's panic path.
        if self.software {
            return Err(
                "Camera GPU upload is not supported by this experimental software adapter".into(),
            );
        }
        let recreate = self
            .cameras
            .get(&frame.camera_id)
            .is_none_or(|old| old.size != size);
        if recreate {
            if let Some(old) = self.cameras.remove(&frame.camera_id) {
                self.state.renderer.write().free_texture(&old.id);
            }
            let texture = self.state.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("egui browser camera"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let id = self.state.renderer.write().register_native_texture(
                &self.state.device,
                &texture.create_view(&Default::default()),
                wgpu::FilterMode::Linear,
            );
            self.cameras
                .insert(frame.camera_id, CameraTexture { texture, id, size });
        }
        let image = self
            .cameras
            .get_mut(&frame.camera_id)
            .expect("created camera texture");
        self.state.queue.copy_external_image_to_texture(
            &wgpu::CopyExternalImageSourceInfo {
                source: wgpu::ExternalImageSource::ImageBitmap(bitmap.clone()),
                origin: wgpu::Origin2d::ZERO,
                flip_y: false,
            },
            wgpu::CopyExternalImageDestInfo {
                texture: &image.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
                color_space: wgpu::PredefinedColorSpace::Srgb,
                premultiplied_alpha: true,
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        self.uploads += 1;
        Ok(())
    }
    pub fn camera(&self, id: CameraId) -> Option<(egui::TextureId, (u32, u32))> {
        self.cameras.get(&id).map(|image| (image.id, image.size))
    }
    pub fn render_scene(&mut self, size: (u32, u32), frame: SceneFrame<'_>) -> egui::TextureId {
        if self.scene.resize(&self.state.device, size.0, size.1) {
            self.state.renderer.write().free_texture(&self.scene_id);
            self.scene_id = self.state.renderer.write().register_native_texture(
                &self.state.device,
                &self.scene.encoded_view(),
                wgpu::FilterMode::Linear,
            );
        }
        if self.scene.needs_render(frame) {
            self.scene
                .render(&self.state.device, &self.state.queue, frame);
        }
        self.scene_id
    }
}
