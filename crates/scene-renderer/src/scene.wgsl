struct SceneUniform {
    view_projection: mat4x4<f32>,
    viewport: vec2<f32>,
    _padding: vec2<f32>,
}

@group(0) @binding(0) var<uniform> scene: SceneUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = scene.view_projection * vec4(input.position, 1.0);
    output.color = input.color;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4(input.color, 1.0);
}

struct PointOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) offset: vec2<f32>,
}

@vertex
fn vs_point(
    @location(0) point: vec3<f32>,
    @builtin(vertex_index) index: u32,
) -> PointOutput {
    let corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    var output: PointOutput;
    let center = scene.view_projection * vec4(point, 1.0);
    let offset = corners[index];
    let clip_offset = vec2(
        offset.x * 5.0 / scene.viewport.x,
        -offset.y * 5.0 / scene.viewport.y,
    ) * center.w;
    output.position = center + vec4(clip_offset, 0.0, 0.0);
    output.offset = offset;
    return output;
}

@fragment
fn fs_point(input: PointOutput) -> @location(0) vec4<f32> {
    if length(input.offset) > 1.0 {
        discard;
    }
    return vec4(0.20, 0.82, 1.0, 0.88);
}
