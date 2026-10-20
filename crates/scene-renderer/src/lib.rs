//! Minimal offscreen 3D scene renderer for a world grid, ego wireframe and planned path.

use bytemuck::{Pod, Zeroable};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneFrame<'a> {
    pub revision: u64,
    pub cloud_revision: u64,
    /// World ROS metre positions, already placed by presentation.
    pub focus: [f32; 3],
    pub forward: [f32; 3],
    pub view_radius: f32,
    pub platform: Option<[[f32; 3]; 8]>,
    pub path: &'a [[f32; 3]],
    /// ROS world-frame points: +x forward, +y left, +z up.
    /// The acquisition-time pose has already been applied.
    pub cloud: &'a [[f32; 3]],
    /// Linear RGBA supplied by presentation, aligned with cloud.
    pub cloud_colors: &'a [[f32; 4]],
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SceneCameraMode {
    /// Follow the ego pose from above and behind, with a slight right offset.
    #[default]
    Chase,
    /// Orbit the ego in world coordinates using mouse input.
    Free,
    /// Look forward from the vehicle.
    VehicleEye,
}

impl SceneCameraMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Chase => "Chase",
            Self::Free => "Free",
            Self::VehicleEye => "Vehicle eye",
        }
    }
}

impl Default for SceneCamera {
    fn default() -> Self {
        Self {
            distance: 6.0,
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
struct PointVertex {
    position: [f32; 3],
    color: [f32; 4],
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
    opaque_point_pipeline: wgpu::RenderPipeline,
    opaque_points: bool,
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
    size: (u32, u32),
    last_revision: Option<u64>,
    last_cloud_revision: Option<u64>,
    camera: SceneCamera,
    camera_mode: SceneCameraMode,
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
        let mut point_descriptor = wgpu::RenderPipelineDescriptor {
            label: Some("3D point billboard pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_point"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<PointVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 12,
                            shader_location: 1,
                        },
                    ],
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
        };
        let point_pipeline = device.create_render_pipeline(&point_descriptor);
        point_descriptor.label = Some("3D opaque point billboard pipeline");
        point_descriptor
            .depth_stencil
            .as_mut()
            .unwrap()
            .depth_write_enabled = true;
        let opaque_point_pipeline = device.create_render_pipeline(&point_descriptor);
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
            opaque_point_pipeline,
            opaque_points: false,
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
            size: (width, height),
            last_revision: None,
            last_cloud_revision: None,
            camera: SceneCamera::default(),
            camera_mode: SceneCameraMode::default(),
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
        if cloud_changed {
            let points = world_cloud(frame).collect::<Vec<_>>();
            // Opaque clouds must occlude by distance, independently of point order.
            // Keep the existing non-writing path for clouds containing transparency.
            self.opaque_points = points.iter().all(|point| point.color[3] >= 1.0);
            if points.len() > self.point_capacity {
                self.point_capacity = points.len().next_power_of_two();
                self.point_buffer = make_empty_point_buffer(device, self.point_capacity);
            }
            if !points.is_empty() {
                queue.write_buffer(&self.point_buffer, 0, bytemuck::cast_slice(&points));
            }
            self.point_count = u32::try_from(points.len()).unwrap_or(u32::MAX);
            self.metrics.point_uploads += 1;
        }

        let (eye, target) = camera_pose(self.camera_mode, self.camera, frame);
        let view = look_at_rh(eye, target, [0.0, 1.0, 0.0]);
        let aspect = self.size.0 as f32 / self.size.1 as f32;
        let projection = perspective_rh(48.0_f32.to_radians(), aspect, 0.01, 160.0);
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
                pass.set_pipeline(if self.opaque_points {
                    &self.opaque_point_pipeline
                } else {
                    &self.point_pipeline
                });
                pass.set_vertex_buffer(0, self.point_buffer.slice(..));
                pass.draw(0..6, 0..self.point_count);
            }
        }
        queue.submit([encoder.finish()]);
        self.last_revision = Some(frame.revision);
        self.last_cloud_revision = Some(frame.cloud_revision);
        self.camera_dirty = false;
        self.metrics.renders += 1;
    }

    pub fn needs_render(&self, frame: SceneFrame<'_>) -> bool {
        self.last_revision != Some(frame.revision)
            || self.last_cloud_revision != Some(frame.cloud_revision)
            || self.camera_dirty
    }

    /// Source replacement or seek may reuse revisions for different content.
    pub fn invalidate(&mut self) {
        self.last_revision = None;
        self.last_cloud_revision = None;
    }

    /// Linear sampling view for color-managed presentation.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Encoded sRGB bytes without automatic decoding, for UI compositors such as egui
    /// whose texture contract expects gamma-encoded values.
    pub fn encoded_view(&self) -> wgpu::TextureView {
        self.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        })
    }

    pub fn metrics(&self) -> SceneMetrics {
        self.metrics
    }

    pub fn visible_points(&self) -> usize {
        self.point_count as usize
    }

    pub fn camera(&self) -> SceneCamera {
        self.camera
    }

    pub fn camera_mode(&self) -> SceneCameraMode {
        self.camera_mode
    }

    pub fn set_camera_mode(&mut self, mode: SceneCameraMode) {
        if self.camera_mode != mode {
            self.camera_mode = mode;
            self.camera_dirty = true;
        }
    }

    /// Positive wheel delta zooms in, negative delta zooms out.
    pub fn zoom(&mut self, wheel_delta: f32) {
        if self.camera_mode == SceneCameraMode::VehicleEye {
            return;
        }
        let distance = (self.camera.distance * (-wheel_delta * 0.002).exp()).clamp(0.3, 80.0);
        if (distance - self.camera.distance).abs() > f32::EPSILON {
            self.camera.distance = distance;
            self.camera_dirty = true;
        }
    }

    pub fn orbit(&mut self, delta_x: f32, delta_y: f32) {
        if self.camera_mode != SceneCameraMode::Free || (delta_x == 0.0 && delta_y == 0.0) {
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

fn camera_pose(
    mode: SceneCameraMode,
    camera: SceneCamera,
    frame: SceneFrame<'_>,
) -> ([f32; 3], [f32; 3]) {
    let focus = ros_to_render(frame.focus);
    let forward = normalize(ros_to_render(frame.forward));
    match mode {
        SceneCameraMode::Chase | SceneCameraMode::Free => {
            let yaw = if mode == SceneCameraMode::Chase {
                (-forward[0]).atan2(-forward[2])
            } else {
                0.0
            };
            let azimuth = camera.azimuth + yaw;
            let horizontal = camera.distance * camera.elevation.cos();
            (
                [
                    focus[0] + horizontal * azimuth.sin(),
                    focus[1] + camera.distance * camera.elevation.sin(),
                    focus[2] + horizontal * azimuth.cos(),
                ],
                focus,
            )
        }
        SceneCameraMode::VehicleEye => {
            // A virtual view policy derived from the supplied envelope, not sensor extrinsics.
            let r = frame.view_radius.max(0.01);
            (
                [
                    focus[0] + forward[0] * r * 0.25,
                    focus[1] + r,
                    focus[2] + forward[2] * r * 0.25,
                ],
                [
                    focus[0] + forward[0] * r * 20.0,
                    focus[1] + r + forward[1] * r * 20.0,
                    focus[2] + forward[2] * r * 20.0,
                ],
            )
        }
    }
}

fn world_cloud(frame: SceneFrame<'_>) -> impl Iterator<Item = PointVertex> + '_ {
    frame
        .cloud
        .iter()
        .enumerate()
        .map(move |(index, [forward, left, up])| PointVertex {
            position: [-left, *up, -*forward],
            color: frame
                .cloud_colors
                .get(index)
                .copied()
                .unwrap_or([1.0, 1.0, 1.0, 1.0]),
        })
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
        // Short metric segments remain stable when the near plane cuts a grid line.
        for step in -50_i32..50 {
            let start = step as f32;
            let end = start + 1.0;
            push_line(
                &mut vertices,
                [start, 0.0, coordinate],
                [end, 0.0, coordinate],
                color,
            );
            push_line(
                &mut vertices,
                [coordinate, 0.0, start],
                [coordinate, 0.0, end],
                color,
            );
        }
    }
    vertices
}

fn make_dynamic_lines(frame: SceneFrame<'_>) -> Vec<Vertex> {
    let mut vertices = Vec::with_capacity(24 + frame.path.len().saturating_mul(2));
    if let Some(corners) = frame.platform {
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
            push_line(
                &mut vertices,
                ros_to_render(corners[a]),
                ros_to_render(corners[b]),
                [0.10, 0.78, 0.88],
            );
        }
    }
    for segment in frame.path.windows(2) {
        push_line(
            &mut vertices,
            ros_to_render(segment[0]),
            ros_to_render(segment[1]),
            [0.96, 0.72, 0.16],
        );
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
        size: (capacity.max(1) * std::mem::size_of::<PointVertex>()) as u64,
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | if cfg!(test) {
                wgpu::TextureUsages::COPY_SRC
            } else {
                wgpu::TextureUsages::empty()
            },
        view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
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

fn ros_to_render([forward, left, up]: [f32; 3]) -> [f32; 3] {
    [-left, up, -forward]
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
        let path = [[0.0, 0.0, 0.1], [1.0, -0.5, 0.2], [2.0, -1.0, 0.3]];
        let vertices = make_dynamic_lines(SceneFrame {
            revision: 1,
            cloud_revision: 0,
            focus: [2.0, 3.0, 0.0],
            forward: [1.0, 0.0, 0.0],
            view_radius: 0.2,
            platform: None,
            path: &path,
            cloud: &[],
            cloud_colors: &[],
        });
        assert_eq!(vertices.len(), 4);
        assert!(vertices.iter().all(|vertex| {
            vertex
                .position
                .iter()
                .all(|component| component.is_finite())
        }));
    }

    #[test]
    fn world_mapping_preserves_per_point_linear_color() {
        let colors = [[0.2, 0.4, 0.6, 0.5]];
        let points = [[1.0, 2.0, 3.0]];
        let frame = SceneFrame {
            cloud: &points,
            cloud_colors: &colors,
            ..Default::default()
        };
        let vertex = world_cloud(frame).next().unwrap();
        assert_eq!(vertex.position, [-2.0, 3.0, -1.0]);
        assert_eq!(vertex.color, colors[0]);
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
        camera.distance = (camera.distance * (-10_000.0_f32 * 0.002).exp()).clamp(0.3, 80.0);
        assert_eq!(camera.distance, 0.3);
        camera.elevation = (camera.elevation + 10_000.0 * 0.008).clamp(0.12, 1.35);
        assert_eq!(camera.elevation, 1.35);
        assert_eq!(SceneCamera::default().distance, 6.0);
    }

    #[test]
    fn chase_and_vehicle_eye_follow_ego_heading() {
        let frame = SceneFrame {
            focus: [2.0, 3.0, 0.0],
            forward: [0.0, 1.0, 0.0],
            view_radius: 0.2,
            ..SceneFrame::default()
        };
        let (chase_eye, chase_target) =
            camera_pose(SceneCameraMode::Chase, SceneCamera::default(), frame);
        assert!(chase_eye.iter().all(|value| value.is_finite()));
        assert_eq!(chase_target, [-3.0, 0.0, -2.0]);

        let (vehicle_eye, vehicle_target) =
            camera_pose(SceneCameraMode::VehicleEye, SceneCamera::default(), frame);
        let view_direction = subtract(vehicle_target, vehicle_eye);
        assert!(view_direction[0] < 0.0);
        assert!(view_direction[0].abs() > view_direction[2].abs());
    }
    #[test]
    fn grid_spacing_is_one_metre_and_path_axis_mapping_preserves_distance() {
        let grid = make_grid();
        assert_eq!(grid[0].position, [-50.0, 0.0, -50.0]);
        assert_eq!(grid[1].position, [-49.0, 0.0, -50.0]);
        assert!(grid.iter().all(|v| v.position[1] == 0.0
            && v.position[0].fract() == 0.0
            && v.position[2].fract() == 0.0));
        let path = [[1.0, 2.0, 0.3], [2.0, 2.0, 0.3]];
        let lines = make_dynamic_lines(SceneFrame {
            path: &path,
            ..Default::default()
        });
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].position, [-2.0, 0.3, -1.0]);
        assert_eq!(lines[1].position, [-2.0, 0.3, -2.0]);
        assert_eq!(
            subtract(lines[1].position, lines[0].position),
            [0.0, 0.0, -1.0]
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod depth_tests;
