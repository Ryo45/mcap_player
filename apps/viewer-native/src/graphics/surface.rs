use super::Graphics;
use anyhow::Result;
use bev_renderer::{BevFrame, BevRenderer};
use egui_wgpu::{Renderer as EguiRenderer, RendererOptions, ScreenDescriptor};
use scene_renderer::{SceneFrame, SceneRenderer};
use std::{collections::BTreeMap, sync::Arc};
use viewer_core::CameraCalibrationSet;
use winit::window::Window;

impl Graphics {
    pub(crate) async fn new(
        window: Arc<Window>,
        calibrations: CameraCalibrationSet,
    ) -> Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("viewer-native device"),
                required_features: wgpu::Features::empty(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                required_limits: wgpu::Limits::default(),
                memory_hints: Default::default(),
                trace: wgpu::Trace::Off,
            })
            .await?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(capabilities.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        let egui_context = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_context.clone(),
            egui_context.viewport_id(),
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let mut egui_renderer = EguiRenderer::new(&device, format, RendererOptions::default());
        let mut bev_renderer = BevRenderer::new(&device, 512, 512);
        bev_renderer.render(&device, &queue, BevFrame::default());
        let bev_texture_id = egui_renderer.register_native_texture(
            &device,
            bev_renderer.view(),
            wgpu::FilterMode::Linear,
        );
        let mut scene_renderer = SceneRenderer::new(&device, 768, 384);
        scene_renderer.render(&device, &queue, SceneFrame::default());
        let scene_texture_id = egui_renderer.register_native_texture(
            &device,
            scene_renderer.view(),
            wgpu::FilterMode::Linear,
        );
        Ok(Self {
            surface,
            device,
            queue,
            config,
            egui_context,
            egui_state,
            egui_renderer,
            camera_slots: BTreeMap::new(),
            camera_texture_ids: BTreeMap::new(),
            uploaded_arrivals: BTreeMap::new(),
            calibrations,
            bev_renderer,
            bev_texture_id,
            scene_renderer,
            scene_texture_id,
        })
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    pub(super) fn paint_egui(
        &mut self,
        window: &Window,
        output: egui::FullOutput,
    ) -> Result<(), wgpu::SurfaceError> {
        let egui::FullOutput {
            platform_output,
            textures_delta,
            shapes,
            pixels_per_point,
            ..
        } = output;
        self.egui_state
            .handle_platform_output(window, platform_output);
        for (id, delta) in &textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, delta);
        }
        let paint_jobs = self.egui_context.tessellate(shapes, pixels_per_point);
        let screen = ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: window.scale_factor() as f32,
        };
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("viewer-native encoder"),
            });
        let callbacks = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen,
        );
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("viewer-native egui pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.015,
                                g: 0.02,
                                b: 0.025,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            self.egui_renderer.render(&mut pass, &paint_jobs, &screen);
        }
        self.queue
            .submit(callbacks.into_iter().chain([encoder.finish()]));
        frame.present();
        for id in &textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        Ok(())
    }
}
