// An imported image on a quad within the widget's bounds (the render pass's
// viewport), sampled as premultiplied colour.

struct Uniforms {
    // Clip-space left, top, right, bottom of the quad.
    rect: vec4<f32>,
    // x = opacity, y = 1 when the format's alpha is to be ignored.
    params: vec4<f32>,
};

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var<uniform> uniforms: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let corner = corners[index];
    let rect = uniforms.rect;
    var output: VertexOutput;
    output.position = vec4<f32>(
        mix(rect.x, rect.z, corner.x),
        mix(rect.y, rect.w, corner.y),
        0.0,
        1.0,
    );
    output.uv = corner;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    var color = textureSample(image, image_sampler, input.uv);
    if (uniforms.params.y > 0.5) {
        color.a = 1.0;
    }
    return color * uniforms.params.x;
}
