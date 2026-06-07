// vertex shader
struct Uniforms {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) intensity: f32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) intensity:f32,
};

fn get_heatmap_color(val: f32) -> vec3f {
    let t = clamp(val, 0.0, 1.0);
    let low = vec3f(0.0, 0.0, 1.0);
    let mid = vec3f(0.0, 1.0, 0.0);
    let high = vec3f(1.0, 0.0, 0.0);
    
    // 0.5を境にブレンドを切り替える（step使用でif文回避）
    return mix(
        mix(low, mid, t * 2.0),
        mix(mid, high, (t - 0.5) * 2.0),
        step(0.5, t)
    );
}

@vertex
fn vs_main(
        @builtin(vertex_index) v_index: u32,
        model:VertexInput
) -> VertexOutput {
    var out: VertexOutput;

    let size = 0.1;
    var offsets = array<vec2<f32>,4>(
        vec2<f32>(-0.5,-0.5),
        vec2<f32>(0.5,-0.5),
        vec2<f32>(-0.5,0.5),
        vec2<f32>(0.5,0.5),
    );
    
    let local_pos = offsets[v_index] *size;

    let view_pos = uniforms.view * vec4<f32>(model.position, 1.0);

    var offset_view_pos = view_pos;
    offset_view_pos.x += local_pos.x;
    offset_view_pos.y += local_pos.y;

    out.clip_position = uniforms.proj * offset_view_pos;

    out.intensity = model.intensity;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // 受け取った色をそのまま塗る (Alphaは1.0)
    let color = get_heatmap_color(in.intensity);
    return vec4<f32>(color, 1.0);
}
