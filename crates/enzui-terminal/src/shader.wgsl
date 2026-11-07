// Terminal grid: one background pass reading a per-cell colour buffer, and
// one instanced pass for glyphs, decorations and the cursor. Positions are
// computed in framebuffer pixels and mapped onto the render pass viewport,
// which iced sets to the primitive's (possibly fractional) bounds.
//
// Colours are sRGB-encoded and blended as such, like the rest of the UI
// under iced's `web-colors`.

struct Uniforms {
    // The render pass viewport, in framebuffer pixels.
    viewport_origin: vec2<f32>,
    viewport_size: vec2<f32>,
    // The grid's top-left, in whole framebuffer pixels.
    grid_origin: vec2<f32>,
    cell: vec2<f32>,
    grid: vec2<u32>,
    default_bg: u32,
    // Bit 0: extend edge cells' backgrounds right and down to the edge.
    flags: u32,
    // The fade over the rows above the view: top and bottom y in framebuffer
    // pixels, and the strength at the top (0: none).
    fade: vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<storage, read> bg_cells: array<u32>;
@group(1) @binding(0) var mask_atlas: texture_2d<f32>;
@group(1) @binding(1) var color_atlas: texture_2d<f32>;

fn unpack(c: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(c & 0xffu),
        f32((c >> 8u) & 0xffu),
        f32((c >> 16u) & 0xffu),
        f32((c >> 24u) & 0xffu),
    ) / 255.0;
}

fn to_clip(p: vec2<f32>) -> vec4<f32> {
    let n = (p - u.viewport_origin) / u.viewport_size * 2.0 - 1.0;
    return vec4<f32>(n.x, -n.y, 0.0, 1.0);
}

// How far a pixel at framebuffer `y` fades into the default background: the
// full strength over most of the rows above the view, easing to none at their
// bottom, where the view's own rows begin.
fn fade_at(y: f32) -> f32 {
    // Most of the view is below the fade: it costs those pixels one compare.
    if (y >= u.fade.y || u.fade.z <= 0.0 || u.fade.y <= u.fade.x) {
        return 0.0;
    }
    let t = clamp((y - u.fade.x) / (u.fade.y - u.fade.x), 0.0, 1.0);
    let held = mix(1.0, 0.84, t / 0.7);
    let easing = mix(0.84, 0.0, (t - 0.7) / 0.3);
    return u.fade.z * select(easing, held, t < 0.7);
}

fn corner(vertex: u32) -> vec2<f32> {
    return vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u));
}

// --- Backgrounds -----------------------------------------------------------

@vertex
fn bg_vs(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    return to_clip(u.viewport_origin + corner(vertex) * u.viewport_size);
}

@fragment
fn bg_fs(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    var cell = floor((frag.xy - u.grid_origin) / u.cell);
    // Past the grid's edge, the edge cell's own colour: not the cursor or a
    // selection drawn on it.
    var past_edge = 0u;
    if ((u.flags & 1u) != 0u && cell.x >= 0.0 && cell.y >= 0.0) {
        let edge = min(cell, vec2<f32>(f32(u.grid.x) - 1.0, f32(u.grid.y) - 1.0));
        past_edge = select(0u, 1u, any(edge != cell));
        cell = edge;
    }
    var color = u.default_bg;
    if (cell.x >= 0.0 && cell.y >= 0.0 && cell.x < f32(u.grid.x) && cell.y < f32(u.grid.y)) {
        // Two words per cell: the colour drawn, then the cell's own.
        let c = bg_cells[(u32(cell.y) * u.grid.x + u32(cell.x)) * 2u + past_edge];
        if ((c >> 24u) != 0u) {
            color = c;
        }
    }
    let fade = fade_at(frag.y);
    if (fade > 0.0) {
        return mix(unpack(color), unpack(u.default_bg), fade);
    }
    return unpack(color);
}

// --- Glyphs, decorations and the cursor --------------------------------------

struct Instance {
    // Column and row, 16 bits each.
    @location(0) cell: u32,
    // Offset from the cell's top-left, signed 16 bits each.
    @location(1) offset: u32,
    // Size in pixels, 16 bits each.
    @location(2) size: u32,
    // Atlas position in pixels, 16 bits each.
    @location(3) uv: u32,
    @location(4) color: u32,
    // 0: mask glyph, 1: colour glyph, 2: solid rectangle.
    @location(5) kind: u32,
}

struct GlyphOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) kind: u32,
}

fn signed16(v: u32) -> f32 {
    return f32(i32(v << 16u) >> 16u);
}

@vertex
fn glyph_vs(@builtin(vertex_index) vertex: u32, i: Instance) -> GlyphOut {
    let c = corner(vertex);
    let cell = vec2<f32>(f32(i.cell & 0xffffu), f32(i.cell >> 16u));
    let offset = vec2<f32>(signed16(i.offset & 0xffffu), signed16(i.offset >> 16u));
    let size = vec2<f32>(f32(i.size & 0xffffu), f32(i.size >> 16u));
    let uv = vec2<f32>(f32(i.uv & 0xffffu), f32(i.uv >> 16u));
    var out: GlyphOut;
    out.position = to_clip(u.grid_origin + cell * u.cell + offset + c * size);
    out.color = unpack(i.color);
    out.uv = uv + c * size;
    out.kind = i.kind;
    return out;
}

@fragment
fn glyph_fs(in: GlyphOut) -> @location(0) vec4<f32> {
    let texel = vec2<i32>(floor(in.uv));
    // Over the faded background, a faded glyph is the glyph drawn fainter.
    let keep = 1.0 - fade_at(in.position.y);
    switch in.kind {
        case 0u: {
            let coverage = textureLoad(mask_atlas, texel, 0).r;
            return vec4<f32>(in.color.rgb, in.color.a * coverage * keep);
        }
        case 1u: {
            let texel_color = textureLoad(color_atlas, texel, 0);
            return vec4<f32>(texel_color.rgb, texel_color.a * keep);
        }
        default: {
            return vec4<f32>(in.color.rgb, in.color.a * keep);
        }
    }
}
