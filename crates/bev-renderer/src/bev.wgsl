struct Viewport {
    size: vec2<f32>,
    pixels_per_meter: f32,
    _padding: f32,
}

@group(0) @binding(0) var<uniform> viewport: Viewport;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2(-1.0, -3.0), vec2(-1.0, 1.0), vec2(3.0, 1.0));
    return vec4(p[index], 0.0, 1.0);
}

@vertex
fn vs_path(
    @location(0) segment: vec4<f32>,
    @builtin(vertex_index) index: u32,
) -> @builtin(position) vec4<f32> {
    let ego_origin = vec2(viewport.size.x * 0.5, viewport.size.y * 0.70);
    let start = ego_origin + vec2(segment.x, -segment.y) * viewport.pixels_per_meter;
    let end = ego_origin + vec2(segment.z, -segment.w) * viewport.pixels_per_meter;
    let delta = end - start;
    let direction = delta / max(length(delta), 0.0001);
    let normal = vec2(-direction.y, direction.x);
    let along = array<f32, 6>(0.0, 1.0, 1.0, 0.0, 1.0, 0.0);
    let side = array<f32, 6>(-1.0, -1.0, 1.0, -1.0, 1.0, 1.0);
    let screen = mix(start, end, along[index]) + normal * side[index] * 2.2;
    let clip = vec2(
        screen.x / viewport.size.x * 2.0 - 1.0,
        1.0 - screen.y / viewport.size.y * 2.0,
    );
    return vec4(clip, 0.0, 1.0);
}

@fragment
fn fs_path() -> @location(0) vec4<f32> {
    return vec4(0.96, 0.72, 0.16, 0.96);
}

fn grid_distance_pixels(world: vec2<f32>, spacing: f32) -> f32 {
    let cell = abs(fract(world / spacing + vec2(0.5)) - vec2(0.5)) * spacing;
    return min(cell.x, cell.y) * viewport.pixels_per_meter;
}

fn rounded_box_distance(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2(radius);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0))) - radius;
}

@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    // Forward is up. Keeping ego below centre reserves more room for the road ahead.
    let ego_origin = vec2(viewport.size.x * 0.5, viewport.size.y * 0.70);
    let world = vec2(
        (p.x - ego_origin.x) / viewport.pixels_per_meter,
        (ego_origin.y - p.y) / viewport.pixels_per_meter,
    );

    let minor = 1.0 - smoothstep(0.55, 1.15, grid_distance_pixels(world, 1.0));
    let major = 1.0 - smoothstep(0.75, 1.55, grid_distance_pixels(world, 5.0));
    let x_axis = 1.0 - smoothstep(0.7, 1.6, abs(world.x) * viewport.pixels_per_meter);
    let y_axis = 1.0 - smoothstep(0.7, 1.6, abs(world.y) * viewport.pixels_per_meter);

    var color = vec3(0.025, 0.043, 0.057);
    color = mix(color, vec3(0.075, 0.13, 0.16), minor * 0.52);
    color = mix(color, vec3(0.12, 0.25, 0.29), major * 0.82);
    color = mix(color, vec3(0.18, 0.39, 0.34), y_axis * 0.52);
    color = mix(color, vec3(0.39, 0.22, 0.20), x_axis * 0.42);

    let local_pixels = vec2(
        world.x * viewport.pixels_per_meter,
        -world.y * viewport.pixels_per_meter,
    );
    let car_half_size = vec2(0.92, 2.10) * viewport.pixels_per_meter;
    let car_distance = rounded_box_distance(
        local_pixels,
        car_half_size,
        0.28 * viewport.pixels_per_meter,
    );
    let car = 1.0 - smoothstep(-0.8, 0.8, car_distance);
    color = mix(color, vec3(0.10, 0.72, 0.82), car);

    // Windshield and nose mark the vehicle heading without adding synthetic scene data.
    let windshield = select(
        0.0,
        1.0,
        abs(world.x) < 0.62 && world.y > 0.35 && world.y < 1.28,
    );
    color = mix(color, vec3(0.035, 0.13, 0.17), windshield * car * 0.92);
    let nose = select(
        0.0,
        1.0,
        abs(world.x) < (1.78 - world.y) * 0.62 && world.y > 1.42 && world.y < 1.78,
    );
    color = mix(color, vec3(0.73, 0.96, 0.99), nose * car);

    return vec4(color, 1.0);
}
