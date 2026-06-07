// vertex shader
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@group(0) @binding(0)
var t_camera: texture_2d<f32>;
@group(0) @binding(1)
var s_camera: sampler;

@vertex
fn vs_main(@builtin(vertex_index) v_index: u32) -> VertexOutput{
    var out: VertexOutput;

    var pos = array<vec2<f32>, 4>(
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>( 1.0, -1.0)
    );

        var uvs = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0), // 0番に対応: 画像の左上
        vec2<f32>(0.0, 1.0), // 1番に対応: 画像の左下
        vec2<f32>(1.0, 0.0), // 2番に対応: 画像の右上
        vec2<f32>(1.0, 1.0)  // 3番に対応: 画像の右下
    );

    out.clip_position = vec4<f32>(pos[v_index], 0.0, 1.0);
    out.uv = uvs[v_index];
    
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(t_camera,s_camera, in.uv);
}