//! Minimal offscreen BEV renderer: a fixed metric grid and ego marker.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BevFrame<'a> {
    pub revision: u64,
    pub path: &'a [[f32; 2]],
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BevMetrics {
    pub target_creations: u64,
    pub layer_uploads: u64,
    pub renders: u64,
}

pub struct BevRenderer {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    path_pipeline: wgpu::RenderPipeline,
    path_buffer: wgpu::Buffer,
    path_capacity: usize,
    path_segment_count: u32,
    viewport_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
    size: (u32, u32),
    last_revision: Option<u64>,
    metrics: BevMetrics,
}

impl BevRenderer {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("minimal BEV shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("bev.wgsl").into()),
        });
        let viewport_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("BEV viewport layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let viewport_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BEV viewport uniform"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let viewport_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("BEV viewport bind group"),
            layout: &viewport_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("BEV pipeline layout"),
            bind_group_layouts: &[&viewport_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("minimal BEV pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let path_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("BEV path pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_path"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_path"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let path_buffer = make_path_buffer(device, 1);
        let (texture, view) = make_target(device, width, height);
        Self {
            texture,
            view,
            pipeline,
            path_pipeline,
            path_buffer,
            path_capacity: 1,
            path_segment_count: 0,
            viewport_buffer,
            viewport_bind_group,
            size: (width, height),
            last_revision: None,
            metrics: BevMetrics {
                target_creations: 1,
                layer_uploads: 0,
                renders: 0,
            },
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) -> bool {
        let size = (width.max(1), height.max(1));
        if size == self.size {
            return false;
        }
        (self.texture, self.view) = make_target(device, size.0, size.1);
        self.size = size;
        self.metrics.target_creations += 1;
        true
    }

    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: BevFrame<'_>) {
        if self.last_revision != Some(frame.revision) {
            let segments: Vec<[f32; 4]> = frame
                .path
                .windows(2)
                .map(|points| [points[0][0], points[0][1], points[1][0], points[1][1]])
                .collect();
            if segments.len() > self.path_capacity {
                self.path_capacity = segments.len().next_power_of_two();
                self.path_buffer = make_path_buffer(device, self.path_capacity);
            }
            if !segments.is_empty() {
                queue.write_buffer(&self.path_buffer, 0, bytemuck::cast_slice(&segments));
            }
            self.path_segment_count = u32::try_from(segments.len()).unwrap_or(u32::MAX);
            self.metrics.layer_uploads += 1;
        }
        // Keep the visible metric extent predictable across differently-shaped panels.
        // The shorter axis shows at least 36 metres, with a small lower bound for tiny windows.
        let pixels_per_meter = (self.size.0.min(self.size.1) as f32 / 36.0).max(4.0);
        let viewport = [
            self.size.0 as f32,
            self.size.1 as f32,
            pixels_per_meter,
            0.0,
        ];
        queue.write_buffer(&self.viewport_buffer, 0, bytemuck::cast_slice(&viewport));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("BEV encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("BEV pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.viewport_bind_group, &[]);
            pass.draw(0..3, 0..1);
            if self.path_segment_count > 0 {
                pass.set_pipeline(&self.path_pipeline);
                pass.set_vertex_buffer(0, self.path_buffer.slice(..));
                pass.draw(0..6, 0..self.path_segment_count);
            }
        }
        queue.submit([encoder.finish()]);
        self.last_revision = Some(frame.revision);
        self.metrics.renders += 1;
    }

    pub fn needs_render(&self, frame: BevFrame<'_>) -> bool {
        self.last_revision != Some(frame.revision)
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
    pub fn metrics(&self) -> BevMetrics {
        self.metrics
    }
}

fn make_path_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("BEV path segments"),
        size: (capacity.max(1) * std::mem::size_of::<[f32; 4]>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn make_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("BEV offscreen target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revisions_are_stable() {
        let points = [[0.0, 0.0], [1.0, 2.0]];
        let frame = BevFrame {
            revision: 4,
            path: &points,
        };
        assert_eq!(frame.revision, 4);
        assert_eq!(frame.path.len(), 2);
    }
}
