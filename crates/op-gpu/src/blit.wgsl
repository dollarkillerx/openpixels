// Draws the developed preview into the canvas, cropped and rotated, and letterboxed
// to preserve its aspect ratio.

struct Blit {
    scale: vec2<f32>,
    // Source UV of a screen position (s, t): uv_origin + s * uv_axis_s + t * uv_axis_t.
    uv_origin: vec2<f32>,
    uv_axis_s: vec2<f32>,
    uv_axis_t: vec2<f32>,
    // Lens corrections; see `LensMap` in op-core.
    lens_aspect: vec2<f32>,
    lens_scale: f32,
    // Screen fraction, from the left, showing the unedited image. 0 is off.
    compare: f32,
    distortion: vec4<f32>,
    chroma: vec4<f32>,
    // Keystone terms, vertical then horizontal.
    perspective: vec4<f32>,
}

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var<uniform> blit: Blit;
// Liquify offsets in UV.
@group(0) @binding(3) var warp: texture_2d<f32>;
// The developed image with no edits, for comparison.
@group(0) @binding(4) var before: texture_2d<f32>;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) across: f32,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    // One triangle strip quad.
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    var out: VertexOut;
    out.position = vec4<f32>((corner * 2.0 - 1.0) * blit.scale, 0.0, 1.0);
    out.uv = blit.uv_origin + corner.x * blit.uv_axis_s + (1.0 - corner.y) * blit.uv_axis_t;
    out.across = corner.x;
    return out;
}

// Where a corrected position reads from, given a channel's fringe scale terms.
fn lens_source(uv: vec2<f32>, constant: f32, slope: f32) -> vec2<f32> {
    let centred = (uv - 0.5) * 2.0 * blit.lens_aspect;
    let depth = 1.0 - blit.perspective.x * centred.y / blit.lens_aspect.y - blit.perspective.y * centred.x / blit.lens_aspect.x;
    let position = centred / depth;
    let r2 = dot(position, position);
    let d = blit.distortion;
    let stretch = 1.0 + r2 * (d.x + r2 * (d.y + r2 * (d.z + r2 * d.w)));
    let fringe = 1.0 + constant + slope * r2;
    return 0.5 + position * stretch * blit.lens_scale * fringe / (2.0 * blit.lens_aspect);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let offset = textureSample(warp, image_sampler, in.uv).rg;
    let at = in.uv + offset;
    let unedited = textureSample(before, image_sampler, in.uv);
    let colour = vec4<f32>(
        textureSample(image, image_sampler, lens_source(at, blit.chroma.x, blit.chroma.y)).r,
        textureSample(image, image_sampler, lens_source(at, 0.0, 0.0)).g,
        textureSample(image, image_sampler, lens_source(at, blit.chroma.z, blit.chroma.w)).b,
        1.0,
    );
    // Rotation can show areas beyond the image; the export leaves those black too.
    if any(in.uv < vec2<f32>(0.0)) || any(in.uv > vec2<f32>(1.0)) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return select(colour, unedited, in.across < blit.compare);
}
