//! Minimal offscreen 3D scene renderer for a world grid, ego wireframe and planned path.

use bytemuck::{Pod, Zeroable};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneFrame<'a> {
    pub revision: u64,
    pub cloud_revision: u64,
    /// ROS odometry coordinates: +x forward, +y left.
    pub ego_position: [f32; 2],
    pub ego_yaw: f32,
    /// Ego-relative BEV coordinates: +x right, +y forward.
    pub path: &'a [[f32; 2]],
    /// ROS world-frame points: +x forward, +y left, +z up.
    /// The acquisition-time pose has already been applied.
    pub cloud: &'a [[f32; 3]],
    pub accumulate: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SceneMetrics {
    pub target_creations: u64,
    pub layer_uploads: u64,
    pub point_uploads: u64,
    pub renders: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneCamera {
    pub distance: f32,
    pub azimuth: f32,
    pub elevation: f32,
}

impl Default for SceneCamera {
    fn default() -> Self {
        Self {
            distance: 18.4,
            azimuth: 0.699,
            elevation: 0.48,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    color: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SceneUniform {
    view_projection: [[f32; 4]; 4],
    viewport: [f32; 2],
    padding: [f32; 2],
}

pub struct SceneRenderer {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    point_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    grid_buffer: wgpu::Buffer,
    grid_vertices: u32,
    dynamic_buffer: wgpu::Buffer,
    dynamic_capacity: usize,
    dynamic_vertices: u32,
    point_buffer: wgpu::Buffer,
    point_capacity: usize,
    point_count: u32,
    accumulated_points: Vec<[f32; 3]>,
    size: (u32, u32),
    last_revision: Option<u64>,
    last_cloud_revision: Option<u64>,
    last_accumulate: bool,
    camera: SceneCamera,
    camera_dirty: bool,
    metrics: SceneMetrics,
}

impl SceneRenderer {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("3D scene shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3D scene uniform layout"),
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
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("3D scene uniform"),
            size: std::mem::size_of::<SceneUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3D scene uniform bind group"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3D scene pipeline layout"),
            bind_group_layouts: &[&uniform_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("3D scene line pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                    ],
                }],
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
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let point_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("3D point billboard pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_point"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<[f32; 3]>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_point"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let grid = make_grid();
        let grid_buffer = make_vertex_buffer(device, "3D world grid", &grid);
        let dynamic_buffer = make_empty_vertex_buffer(device, "3D dynamic lines", 1);
        let point_buffer = make_empty_point_buffer(device, 1);
        let (texture, view, depth, depth_view) = make_targets(device, width, height);
        Self {
            texture,
            view,
            depth,
            depth_view,
            pipeline,
            point_pipeline,
            uniform_buffer,
            uniform_bind_group,
            grid_buffer,
            grid_vertices: u32::try_from(grid.len()).unwrap_or(u32::MAX),
            dynamic_buffer,
            dynamic_capacity: 1,
            dynamic_vertices: 0,
            point_buffer,
            point_capacity: 1,
            point_count: 0,
            accumulated_points: Vec::new(),
            size: (width, height),
            last_revision: None,
            last_cloud_revision: None,
            last_accumulate: false,
            camera: SceneCamera::default(),
            camera_dirty: true,
            metrics: SceneMetrics {
                target_creations: 1,
                layer_uploads: 0,
                point_uploads: 0,
                renders: 0,
            },
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) -> bool {
        let size = (width.max(1), height.max(1));
        if size == self.size {
            return false;
        }
        (self.texture, self.view, self.depth, self.depth_view) =
            make_targets(device, size.0, size.1);
        self.size = size;
        self.metrics.target_creations += 1;
        true
    }

    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: SceneFrame<'_>) {
        if self.last_revision != Some(frame.revision) {
            let vertices = make_dynamic_lines(frame);
            if vertices.len() > self.dynamic_capacity {
                self.dynamic_capacity = vertices.len().next_power_of_two();
                self.dynamic_buffer =
                    make_empty_vertex_buffer(device, "3D dynamic lines", self.dynamic_capacity);
            }
            if !vertices.is_empty() {
                queue.write_buffer(&self.dynamic_buffer, 0, bytemuck::cast_slice(&vertices));
            }
            self.dynamic_vertices = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
            self.metrics.layer_uploads += 1;
        }
        let cloud_changed = self.last_cloud_revision != Some(frame.cloud_revision);
        let mode_changed = self.last_accumulate != frame.accumulate;
        if cloud_changed || mode_changed {
            update_accumulated_cloud(
                &mut self.accumulated_points,
                frame,
                cloud_changed,
                mode_changed,
            );
            if self.accumulated_points.len() > self.point_capacity {
                self.point_capacity = self.accumulated_points.len().next_power_of_two();
                self.point_buffer = make_empty_point_buffer(device, self.point_capacity);
            }
            if !self.accumulated_points.is_empty() {
                queue.write_buffer(
                    &self.point_buffer,
                    0,
                    bytemuck::cast_slice(&self.accumulated_points),
                );
            }
            self.point_count = u32::try_from(self.accumulated_points.len()).unwrap_or(u32::MAX);
            self.metrics.point_uploads += 1;
        }

        let ego = odom_to_world(frame.ego_position);
        let horizontal_distance = self.camera.distance * self.camera.elevation.cos();
        let eye = [
            ego[0] + horizontal_distance * self.camera.azimuth.sin(),
            self.camera.distance * self.camera.elevation.sin(),
            ego[1] + horizontal_distance * self.camera.azimuth.cos(),
        ];
        let target = [ego[0], 0.6, ego[1]];
        let view = look_at_rh(eye, target, [0.0, 1.0, 0.0]);
        let aspect = self.size.0 as f32 / self.size.1 as f32;
        let projection = perspective_rh(48.0_f32.to_radians(), aspect, 0.1, 160.0);
        let view_projection = multiply(projection, view);
        let uniform = SceneUniform {
            view_projection,
            viewport: [self.size.0 as f32, self.size.1 as f32],
            padding: [0.0; 2],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("3D scene encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("3D scene pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.012,
                            g: 0.019,
                            b: 0.026,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, self.grid_buffer.slice(..));
            pass.draw(0..self.grid_vertices, 0..1);
            if self.dynamic_vertices > 0 {
                pass.set_vertex_buffer(0, self.dynamic_buffer.slice(..));
                pass.draw(0..self.dynamic_vertices, 0..1);
            }
            if self.point_count > 0 {
                pass.set_pipeline(&self.point_pipeline);
                pass.set_vertex_buffer(0, self.point_buffer.slice(..));
                pass.draw(0..6, 0..self.point_count);
            }
        }
        queue.submit([encoder.finish()]);
        self.last_revision = Some(frame.revision);
        self.last_cloud_revision = Some(frame.cloud_revision);
        self.last_accumulate = frame.accumulate;
        self.camera_dirty = false;
        self.metrics.renders += 1;
    }

    pub fn needs_render(&self, frame: SceneFrame<'_>) -> bool {
        self.last_revision != Some(frame.revision)
            || self.last_cloud_revision != Some(frame.cloud_revision)
            || self.last_accumulate != frame.accumulate
            || self.camera_dirty
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn metrics(&self) -> SceneMetrics {
        self.metrics
    }

    pub fn visible_points(&self) -> usize {
        self.accumulated_points.len()
    }

    pub fn camera(&self) -> SceneCamera {
        self.camera
    }

    /// Positive wheel delta zooms in, negative delta zooms out.
    pub fn zoom(&mut self, wheel_delta: f32) {
        let distance = (self.camera.distance * (-wheel_delta * 0.002).exp()).clamp(4.0, 80.0);
        if (distance - self.camera.distance).abs() > f32::EPSILON {
            self.camera.distance = distance;
            self.camera_dirty = true;
        }
    }

    pub fn orbit(&mut self, delta_x: f32, delta_y: f32) {
        if delta_x == 0.0 && delta_y == 0.0 {
            return;
        }
        self.camera.azimuth += delta_x * 0.008;
        self.camera.elevation = (self.camera.elevation + delta_y * 0.008).clamp(0.12, 1.35);
        self.camera_dirty = true;
    }

    pub fn reset_camera(&mut self) {
        self.camera = SceneCamera::default();
        self.camera_dirty = true;
    }
}

const MAX_ACCUMULATED_POINTS: usize = 65_536;

fn update_accumulated_cloud(
    accumulated: &mut Vec<[f32; 3]>,
    frame: SceneFrame<'_>,
    cloud_changed: bool,
    mode_changed: bool,
) {
    if frame.cloud.is_empty() {
        accumulated.clear();
        return;
    }
    if cloud_changed {
        if !frame.accumulate {
            accumulated.clear();
        }
        accumulated.extend(world_cloud(frame));
    } else if mode_changed && !frame.accumulate {
        accumulated.clear();
        accumulated.extend(world_cloud(frame));
    }
    if accumulated.len() > MAX_ACCUMULATED_POINTS {
        let excess = accumulated.len() - MAX_ACCUMULATED_POINTS;
        accumulated.drain(..excess);
    }
}

fn world_cloud(frame: SceneFrame<'_>) -> impl Iterator<Item = [f32; 3]> + '_ {
    frame
        .cloud
        .iter()
        .map(|[forward, left, up]| [-left, *up, -*forward])
}

fn make_grid() -> Vec<Vertex> {
    let mut vertices = Vec::new();
    for meter in -50_i32..=50 {
        let coordinate = meter as f32;
        let color = if meter == 0 {
            [0.22, 0.48, 0.42]
        } else if meter % 5 == 0 {
            [0.12, 0.26, 0.30]
        } else {
            [0.055, 0.10, 0.12]
        };
        push_line(
            &mut vertices,
            [-50.0, 0.0, coordinate],
            [50.0, 0.0, coordinate],
            color,
        );
        push_line(
            &mut vertices,
            [coordinate, 0.0, -50.0],
            [coordinate, 0.0, 50.0],
            color,
        );
    }
    vertices
}

fn make_dynamic_lines(frame: SceneFrame<'_>) -> Vec<Vertex> {
    let mut vertices = Vec::with_capacity(30 + frame.path.len().saturating_mul(2));
    let ego = odom_to_world(frame.ego_position);
    let (sin_yaw, cos_yaw) = frame.ego_yaw.sin_cos();
    let right = [cos_yaw, -sin_yaw];
    let forward = [-sin_yaw, -cos_yaw];
    let to_world = |right_offset: f32, forward_offset: f32, height: f32| {
        [
            ego[0] + right[0] * right_offset + forward[0] * forward_offset,
            height,
            ego[1] + right[1] * right_offset + forward[1] * forward_offset,
        ]
    };

    let corners = [
        to_world(-0.92, -2.1, 0.05),
        to_world(0.92, -2.1, 0.05),
        to_world(0.92, 2.1, 0.05),
        to_world(-0.92, 2.1, 0.05),
        to_world(-0.92, -2.1, 1.5),
        to_world(0.92, -2.1, 1.5),
        to_world(0.92, 2.1, 1.5),
        to_world(-0.92, 2.1, 1.5),
    ];
    let ego_color = [0.10, 0.78, 0.88];
    for [a, b] in [
        [0, 1],
        [1, 2],
        [2, 3],
        [3, 0],
        [4, 5],
        [5, 6],
        [6, 7],
        [7, 4],
        [0, 4],
        [1, 5],
        [2, 6],
        [3, 7],
    ] {
        push_line(&mut vertices, corners[a], corners[b], ego_color);
    }
    push_line(
        &mut vertices,
        to_world(0.0, 2.1, 1.5),
        to_world(0.0, 3.3, 1.5),
        [0.72, 0.97, 1.0],
    );

    let path_color = [0.96, 0.72, 0.16];
    for segment in frame.path.windows(2) {
        let start = to_world(segment[0][0], segment[0][1], 0.08);
        let end = to_world(segment[1][0], segment[1][1], 0.08);
        push_line(&mut vertices, start, end, path_color);
    }
    vertices
}

fn push_line(vertices: &mut Vec<Vertex>, start: [f32; 3], end: [f32; 3], color: [f32; 3]) {
    vertices.push(Vertex {
        position: start,
        color,
    });
    vertices.push(Vertex {
        position: end,
        color,
    });
}

fn make_vertex_buffer(device: &wgpu::Device, label: &str, vertices: &[Vertex]) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(vertices),
        usage: wgpu::BufferUsages::VERTEX,
    })
}

fn make_empty_vertex_buffer(device: &wgpu::Device, label: &str, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (capacity.max(1) * std::mem::size_of::<Vertex>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn make_empty_point_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("3D point cloud"),
        size: (capacity.max(1) * std::mem::size_of::<[f32; 3]>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn make_targets(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (
    wgpu::Texture,
    wgpu::TextureView,
    wgpu::Texture,
    wgpu::TextureView,
) {
    let extent = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("3D scene color target"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("3D scene depth target"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view, depth, depth_view)
}

fn odom_to_world(position: [f32; 2]) -> [f32; 2] {
    [-position[1], -position[0]]
}

type Matrix = [[f32; 4]; 4];

fn perspective_rh(fovy: f32, aspect: f32, near: f32, far: f32) -> Matrix {
    let f = 1.0 / (fovy * 0.5).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / (near - far), -1.0],
        [0.0, 0.0, near * far / (near - far), 0.0],
    ]
}

fn look_at_rh(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Matrix {
    let forward = normalize(subtract(target, eye));
    let side = normalize(cross(forward, up));
    let corrected_up = cross(side, forward);
    [
        [side[0], corrected_up[0], -forward[0], 0.0],
        [side[1], corrected_up[1], -forward[1], 0.0],
        [side[2], corrected_up[2], -forward[2], 0.0],
        [
            -dot(side, eye),
            -dot(corrected_up, eye),
            dot(forward, eye),
            1.0,
        ],
    ]
}

fn multiply(left: Matrix, right: Matrix) -> Matrix {
    let mut output = [[0.0; 4]; 4];
    for column in 0..4 {
        for row in 0..4 {
            output[column][row] = (0..4)
                .map(|index| left[index][row] * right[column][index])
                .sum();
        }
    }
    output
}

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn normalize(value: [f32; 3]) -> [f32; 3] {
    let length = dot(value, value).sqrt().max(f32::EPSILON);
    [value[0] / length, value[1] / length, value[2] / length]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_scene_contains_ego_and_path_lines() {
        let path = [[0.0, 0.0], [0.5, 1.0], [1.0, 2.0]];
        let vertices = make_dynamic_lines(SceneFrame {
            revision: 1,
            cloud_revision: 0,
            ego_position: [2.0, 3.0],
            ego_yaw: 0.25,
            path: &path,
            cloud: &[],
            accumulate: false,
        });
        assert_eq!(vertices.len(), 24 + 2 + 4);
        assert!(vertices.iter().all(|vertex| {
            vertex
                .position
                .iter()
                .all(|component| component.is_finite())
        }));
    }

    #[test]
    fn accumulation_switches_between_history_and_latest() {
        let first = [[0.0, 0.1, 1.0], [1.0, 0.1, 1.0]];
        let second = [[0.0, 0.1, 2.0]];
        let mut accumulated = Vec::new();
        let first_frame = SceneFrame {
            cloud_revision: 1,
            cloud: &first,
            accumulate: true,
            ..SceneFrame::default()
        };
        update_accumulated_cloud(&mut accumulated, first_frame, true, true);
        let second_frame = SceneFrame {
            cloud_revision: 2,
            cloud: &second,
            accumulate: true,
            ..SceneFrame::default()
        };
        update_accumulated_cloud(&mut accumulated, second_frame, true, false);
        assert_eq!(
            accumulated,
            vec![[-0.1, 1.0, -0.0], [-0.1, 1.0, -1.0], [-0.1, 2.0, -0.0]]
        );

        let snapshot = accumulated.clone();
        update_accumulated_cloud(
            &mut accumulated,
            SceneFrame {
                ego_position: [100.0, -50.0],
                ego_yaw: 2.0,
                ..second_frame
            },
            false,
            false,
        );
        assert_eq!(
            accumulated, snapshot,
            "pose-only updates must not move history"
        );

        let latest_only = SceneFrame {
            accumulate: false,
            ..second_frame
        };
        update_accumulated_cloud(&mut accumulated, latest_only, false, true);
        assert_eq!(accumulated.len(), 1);
    }

    #[test]
    fn projection_matrix_is_finite() {
        let projection = perspective_rh(48.0_f32.to_radians(), 16.0 / 9.0, 0.1, 160.0);
        let view = look_at_rh([10.0, 8.0, 12.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(
            multiply(projection, view)
                .iter()
                .flatten()
                .all(|value| value.is_finite())
        );
    }

    #[test]
    fn camera_zoom_orbit_and_reset_are_bounded() {
        let mut camera = SceneCamera::default();
        camera.distance = (camera.distance * (-10_000.0_f32 * 0.002).exp()).clamp(4.0, 80.0);
        assert_eq!(camera.distance, 4.0);
        camera.elevation = (camera.elevation + 10_000.0 * 0.008).clamp(0.12, 1.35);
        assert_eq!(camera.elevation, 1.35);
        assert_eq!(SceneCamera::default().distance, 18.4);
    }
}
