// Adaptive glass over an arbitrary silhouette, appended to the renderer's
// material shader (`iced_wgpu::shader::MATERIAL`) for `u`, `filtered` and the
// colour helpers.
//
// The silhouette texture holds, per texel, the signed distance to the outline
// in texels (negative inside) and the centre and width of the connected
// component the texel belongs to. Each component is its own flat radial lens:
// its rim folds the backdrop towards the component's centre, its body bends
// gently, and colour separates along the same direction.

struct Silhouette { source_width: f32, alpha: f32, unused: vec2<f32> };
@group(0) @binding(5) var silhouette_texture: texture_2d<f32>;
@group(0) @binding(6) var<uniform> silhouette: Silhouette;

fn silhouette_texel(p: vec2<i32>) -> vec4<f32> {
    let last = vec2<i32>(textureDimensions(silhouette_texture)) - vec2<i32>(1);
    return textureLoad(silhouette_texture, clamp(p, vec2<i32>(0), last), 0);
}

// Bilinear by hand: float32 filtering is an optional feature.
fn silhouette_field(p: vec2<f32>) -> vec4<f32> {
    let uv = (p - u.region.xy) / (2.0 * u.region.zw) + vec2<f32>(0.5);
    let texel = uv * vec2<f32>(textureDimensions(silhouette_texture)) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(texel));
    let part = fract(texel);
    return mix(
        mix(silhouette_texel(base), silhouette_texel(base + vec2<i32>(1, 0)), part.x),
        mix(silhouette_texel(base + vec2<i32>(0, 1)), silhouette_texel(base + vec2<i32>(1, 1)), part.x),
        part.y,
    );
}

fn backdrop_luma(p: vec2<f32>) -> f32 {
    return dot(srgb_encode(straight(filtered(p))), vec3<f32>(0.299, 0.587, 0.114));
}

// One tone for the whole region, from a 4 x 3 grid over its inner 85 %, so
// separate shapes never band against each other.
fn ambient_luma() -> f32 {
    var sum = 0.0;
    for (var y = 0; y < 3; y++) {
        for (var x = 0; x < 4; x++) {
            let at = (vec2<f32>(f32(x), f32(y)) + 0.5) / vec2<f32>(4.0, 3.0) * 2.0 - 1.0;
            sum += backdrop_luma(u.region.xy + u.region.zw * at * 0.85);
        }
    }
    return sum / 12.0;
}

@fragment
fn fs_silhouette(input: Varying) -> @location(0) vec4<f32> {
    let p = input.position.xy;
    let field = silhouette_field(p);
    let signed = field.r * (u.region.z * 2.0 / silhouette.source_width);
    let coverage = 1.0 - smoothstep(-0.75, 0.75, signed);
    if coverage <= 0.0 {
        return vec4<f32>(0.0);
    }

    let scale = max(u.surface.z, 0.0001);
    let component = field.a / silhouette.source_width;
    let centre = u.region.xy
        + (field.gb / vec2<f32>(textureDimensions(silhouette_texture)) - vec2<f32>(0.5)) * 2.0 * u.region.zw;
    let towards = centre - p;
    let direction = towards / max(length(towards), scale);
    // Smooth tails: no hard line between rim and body.
    let reach = max(2.0 * u.shape.y * component, scale);
    let depth = max(-signed, 0.0) / reach;
    let fold_weight = exp(-4.0 * depth * depth);
    let chroma_weight = exp(-1.5 * depth * depth);
    let strength = 2.0 * u.shape.z * u.optics.x * component;
    let body_bend = towards * (0.045 * u.optics.x);
    let base = p + mix(body_bend, direction * strength, fold_weight);
    let chroma = u.optics.z * CHROMA_PER_DISPERSION * component * scale;
    let half_width = max(u.region.z * component, scale);
    let body_fringe = -towards / half_width * (0.06 * chroma);
    let fringe = mix(body_fringe, -direction * chroma, chroma_weight);
    let sample = filtered(base);
    var color = vec3<f32>(
        straight(filtered(base + fringe)).r,
        straight(sample).g,
        straight(filtered(base - fringe)).b,
    );
    // Dark backdrops lift towards white so the shape stays legible; bright
    // ones keep their translucency.
    var tint = u.color.x;
    if tint > 0.0 {
        let dark = 1.0 - smoothstep(0.10, 0.55, ambient_luma());
        let maximum = select(0.30, 0.45, u.optics.w > 0.0);
        tint = mix(tint, maximum, dark);
    }
    color = mix(color, decode(u.tint.rgb), tint);
    // A soft lighting core avoids an abrupt normal turn at a component's centre.
    let rim_distance = (max(-signed, 0.0) / scale - 0.25) / 1.65;
    let rim = exp(-0.5 * rim_distance * rim_distance);
    let light_core = max(half_width * 0.5, scale);
    let light_direction = -towards / sqrt(dot(towards, towards) + light_core * light_core);
    let facing = dot(light_direction, vec2<f32>(-0.6, -0.8));
    let reflection = u.finish.x * rim * (0.65 + 0.35 * facing);
    let lit = srgb_encode(color) + vec3<f32>(reflection);
    let alpha = coverage * clamp(sample.a, 0.0, 1.0) * silhouette.alpha;
    return vec4<f32>(encode(srgb_decode(lit)) * alpha, alpha);
}
