use std::sync::Arc;
use winit::window::Window;
use wgpu::util::DeviceExt;
use crate::camera::{self, Camera};
use rand::prelude::*;
use crate::ui::Gui;
use winit::event::WindowEvent;
use egui::{scroll_area::State, text};
use crate::transform::pcd::{PointCloud2, parse_lidar_packet, LidarPointVertex}; // 追加


// 頂点データの構造体 (Rust -> GPU にバイト列として送る)
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 3],
    color: [f32; 3],
}

// Uniformバッファ (行列) の構造体
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct LidarPoint {
    position: [f32; 3],
    intensity: f32,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pub size: winit::dpi::PhysicalSize<u32>,

    // パイプライン関連
    grid_render_pipeline: wgpu::RenderPipeline,
    grid_vertex_buffer: wgpu::Buffer,
    num_vertices: u32,

    camera_render_pipeline: wgpu::RenderPipeline,
    camera_bind_group: wgpu::BindGroup, 
    camera_texture: wgpu::Texture, // 更新時に書き込むために保持

    camera_bind_group_layout: wgpu::BindGroupLayout, 
    

    lidar_render_pipeline: wgpu::RenderPipeline,
    lidar_vertex_buffer: wgpu::Buffer,
    num_point: u32,
    
    // Uniform関連
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,

    // GUI関係
    pub gui: Gui,
    last_frame_time: std::time::Instant,

}

impl Renderer {
    pub async fn new(window: Arc<Window>) -> Self {

        let size = window.inner_size();

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone()).unwrap();
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }).await.expect("No adapter found");
        
        let (device, queue) = adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("McapPlayer Device"),
                required_features: wgpu::Features::empty(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                required_limits: wgpu::Limits::default(),
                memory_hints: Default::default(),
                trace: wgpu::Trace::Off, // Trace path
            },
        ).await.unwrap();

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(surface_caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let gui = Gui::new(&window, &device, config.format);

        let grid_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Grid Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/grid.wgsl").into()),
        });

        let mut vertices = Vec::new();
        let grid_size = 20; // 20x20グリッド
        let step = 1.0;     // 1m間隔
        let color = [0.5, 0.5, 0.5]; // 灰色

        for i in -grid_size..=grid_size {
            let pos = i as f32 * step;
            let limit = grid_size as f32 * step;
            
            // X軸平行 (Y=pos)
            vertices.push(Vertex { position: [-limit, pos, 0.0], color });
            vertices.push(Vertex { position: [ limit, pos, 0.0], color });

            // Y軸平行 (X=pos)
            vertices.push(Vertex { position: [pos, -limit, 0.0], color });
            vertices.push(Vertex { position: [pos,  limit, 0.0], color });
        }
        
        // 軸 (X=赤, Y=緑) を追加
        vertices.push(Vertex { position: [0.0, 0.0, 0.0], color: [1.0, 0.0, 0.0] }); // X start
        vertices.push(Vertex { position: [5.0, 0.0, 0.0], color: [1.0, 0.0, 0.0] }); // X end
        vertices.push(Vertex { position: [0.0, 0.0, 0.0], color: [0.0, 1.0, 0.0] }); // Y start
        vertices.push(Vertex { position: [0.0, 5.0, 0.0], color: [0.0, 1.0, 0.0] }); // Y end

        let grid_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Grid Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // --- 4. Uniform (行列) バッファ ---
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Camera Uniform Buffer"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Camera Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Camera Bind Group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let render_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Render Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let grid_render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Grid Render Pipeline"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &grid_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 }, // position
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 }, // color
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &grid_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList, // 線を描く設定
                ..Default::default()
            },
            depth_stencil: None, // まだ深度バッファなし
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // lidar描画用のpipeline作成
        let lidar_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lidar Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/lidar.wgsl").into()),
        });

        
        let num_point=10000 as u32;
        let max_points = 300_000;
        let mut lidar_vertices = generate_random_points(num_point);
        // let lidar_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        //     label: Some("Lidar Vertex Buffer"),
        //     contents: bytemuck::cast_slice(&lidar_vertices),
        //     usage: wgpu::BufferUsages::VERTEX,
        // });
        let lidar_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lidar Vertex Buffer"),
            size: (max_points * std::mem::size_of::<LidarPoint>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, 
            mapped_at_creation: false,
        });



        let lidar_render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("lidar Render Pipeline"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &lidar_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<LidarPoint>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 }, // position
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 12, shader_location: 1 }, // intensity
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &lidar_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip, // 線を描く設定
                ..Default::default()
            },
            depth_stencil: None, // まだ深度バッファなし
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });


        // Camear関係のセットアップ

        let initial_widht = 1280;
        let initial_height = 720;
        let texture_size = wgpu::Extent3d {
            width: initial_widht,
            height: initial_height,
            depth_or_array_layers: 1,
        };

        let camera_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Camera Texture"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let camera_texture_view = camera_texture.create_view(&wgpu::TextureViewDescriptor::default());


        let camera_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let camera_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Camera Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true},
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
            },
            ],
        });
        
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Camera Bind Group"),
            layout: &camera_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding : 0, resource: wgpu::BindingResource::TextureView(&camera_texture_view)},
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&camera_sampler),}
            ]
        });

        let camera_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Camera Pipeline Layout"),
            bind_group_layouts: &[&camera_bind_group_layout],
            push_constant_ranges: &[],
        });

        // Camera描画用のpipeline
        let camera_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Camera Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/camera.wgsl").into()),
        });

        let camera_render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Camera Render Pipeline"),
            layout: Some(&camera_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &camera_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &camera_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });


        Self {
            surface,
            device,
            queue,
            config,
            size,
            grid_render_pipeline,
            grid_vertex_buffer,
            num_vertices: vertices.len() as u32,
            lidar_render_pipeline,
            lidar_vertex_buffer,
            num_point: num_point as u32,
            camera_render_pipeline,
            camera_bind_group,
            camera_texture,
            camera_bind_group_layout,
            uniform_buffer,
            bind_group,
            gui,
            last_frame_time: std::time::Instant::now(),
        }
    }

    pub fn resize(&mut self, new_width: u32, new_height: u32) {
        if new_width > 0 && new_height > 0 {
            self.size.width = new_width;
            self.size.height = new_height;
            self.config.width = new_width;
            self.config.height = new_height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    // カメラを受け取って描画
    pub fn render(&mut self, window: &Window, camera: &Camera) -> Result<(), wgpu::SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

        // FPS計算
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.last_frame_time).as_secs_f32();
        self.last_frame_time = now;
        let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };

        // 行列計算してGPUに送る
        let (viewport_mat, projection_mat) = camera.build_view_proj_matrix();
        let uniform = CameraUniform {
            view: viewport_mat.to_cols_array_2d(),
            proj: projection_mat.to_cols_array_2d(),
        };
        self.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniform]));

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.1, g: 0.1, b: 0.1, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            // パイプライン・BindGroup・頂点バッファをセットして描画
            if self.gui.show_grid {
                render_pass.set_bind_group(0, &self.bind_group, &[]);
                render_pass.set_pipeline(&self.grid_render_pipeline);
                render_pass.set_vertex_buffer(0, self.grid_vertex_buffer.slice(..));
                render_pass.draw(0..self.num_vertices, 0..1);
            }
            render_pass.set_pipeline(&self.lidar_render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.lidar_vertex_buffer.slice(..));
            render_pass.draw(0..4, 0..self.num_point);

            // Camera描画
            render_pass.set_pipeline(&self.camera_render_pipeline);
            render_pass.set_bind_group(0, &self.camera_bind_group, &[]);
            render_pass.draw(0..4, 0..1); // 4頂点で四角形を描く


        }

        // 4. UI描画パス (Overlay)
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: window.scale_factor() as f32,
        };

        let full_output = self.gui.prepare(window, fps);
        
        self.gui.render(
            &self.device, 
            &self.queue, 
            &mut encoder, 
            &view, 
            &screen_descriptor,
            full_output,
            &window,
        );

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        Ok(())
    }

    pub fn handle_event(&mut self, window: &Window, event: &WindowEvent) -> egui_winit::EventResponse {
        self.gui.handle_event(window, event)
    }

    pub fn update_lidar(&mut self, packet: &crate::input::LoadedPacket) {
        // 1. デコード
        // PointCloud2以外（画像など）が来るとエラーになるので無視する
        if let Ok(vertices) = parse_lidar_packet(packet) {
            if vertices.is_empty() { return; }

        let max_points = (self.lidar_vertex_buffer.size() / std::mem::size_of::<LidarPoint>() as u64) as usize;
        
        // 最大容量を超えないようにカットする
        let write_len = vertices.len().min(max_points);
        self.num_point = write_len as u32;

            // GPUのバッファへゼロコピーに近い形で直接転送
            self.queue.write_buffer(
                &self.lidar_vertex_buffer, 
                0, 
                bytemuck::cast_slice(&vertices)
            );
            // // バッファ更新処理
            // self.lidar_vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            //     label: Some("Lidar Vertex Buffer"),
            //     contents: bytemuck::cast_slice(&vertices),
            //     usage: wgpu::BufferUsages::VERTEX,
            // });
            // self.num_point = vertices.len() as u32;
        }
    }


    // 今はカメラ画像を更新できないので常に黒が
    pub fn update_camera(&mut self, width: u32, height:u32, image: &[u8]) {
        // let size: wgpu::Extent3d = wgpu::Extent3d {
        //     width,
        //     height,
        //     depth_or_array_layers: 1,
        // };
        // // TODO: textureのサイズが変わった場合の対応, 今はサイズは固定
        // // imageをGPUにアップロード
        // self.queue.write_texture(
        //     wgpu::ImageCopyTexture {
        //         texture: &self.camera_texture,
        //         mip_level: 0,
        //         origin: wgpu::Origin3d::ZERO,
        //         aspect: wgpu::TextureAspect::All,
        //     },
        //     image,
        //     wgpu::ImageDataLayout {
        //         offset: 0,
        //         bytes_per_row: Some(4 * width), // RGBA8なので4バイト
        //         rows_per_image: Some(height),
        //     },
        //     size,
        // );
        
    }

}

fn generate_random_points(count: u32) -> Vec<LidarPoint> {
    let mut rng = rand::thread_rng();
    let mut points = Vec::with_capacity(count as usize); // 重要：メモリ確保を一度で済ませる

    for _ in 0..count {
        points.push(LidarPoint {
            position: [
                rng.gen_range(-10.0..10.0), // X: -1.0 ~ 1.0 の範囲
                rng.gen_range(-10.0..10.0), // Y
                rng.gen_range(-10.0..10.0), // Z
            ],
            intensity: rng.gen_range(0.0..1.0), // 0.0 ~ 1.0
        });
    }
    points
}
