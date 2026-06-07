// 路面に格子を書く

// vertex shader
struct Uniforms {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color:vec3<f32>,
};

@vertex
fn vs_main(model:VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.proj *(uniforms.view * vec4<f32>(model.position,1.0));
    out.color = model.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // 受け取った色をそのまま塗る (Alphaは1.0)
    return vec4<f32>(in.color, 1.0);
}
