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
    // Keystone terms, vertical then horizontal; z is 1 when the whole photo is kept;
    // w is the post-crop vignette.
    perspective: vec4<f32>,
    // Part of the framed image on the canvas: first s and t, then their spans.
    view: vec4<f32>,
    // Where the full-resolution layer sits in source UV: minimum then maximum.
    detail_rect: vec4<f32>,
    // x: the full-resolution layer is valid. y and z: warn about clipped highlights, shadows.
    flags: vec4<f32>,
}

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var<uniform> blit: Blit;
// Liquify offsets in UV.
@group(0) @binding(3) var warp: texture_2d<f32>;
// The developed image with no edits, for comparison.
@group(0) @binding(4) var before: texture_2d<f32>;
// Part of the image developed at full resolution, for viewing up close.
@group(0) @binding(5) var detail: texture_2d<f32>;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) across: f32,
    @location(2) framed: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    // One triangle strip quad.
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    var out: VertexOut;
    out.position = vec4<f32>((corner * 2.0 - 1.0) * blit.scale, 0.0, 1.0);
    let s = blit.view.x + corner.x * blit.view.z;
    let t = blit.view.y + (1.0 - corner.y) * blit.view.w;
    out.uv = blit.uv_origin + s * blit.uv_axis_s + t * blit.uv_axis_t;
    out.across = s;
    out.framed = vec2<f32>(s, t);
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

// The developed image at a source position, at the best resolution available there.
fn developed_at(position: vec2<f32>) -> vec4<f32> {
    let coarse = textureSample(image, image_sampler, position);
    let within = (position - blit.detail_rect.xy) / (blit.detail_rect.zw - blit.detail_rect.xy);
    let fine = textureSample(detail, image_sampler, within);
    let inside = blit.flags.x > 0.0 && all(within >= vec2<f32>(0.0)) && all(within <= vec2<f32>(1.0));
    return select(coarse, fine, inside);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let offset = textureSample(warp, image_sampler, in.uv).rg;
    let at = in.uv + offset;
    let unedited = textureSample(before, image_sampler, in.uv);
    var colour = vec4<f32>(
        developed_at(lens_source(at, blit.chroma.x, blit.chroma.y)).r,
        developed_at(lens_source(at, 0.0, 0.0)).g,
        developed_at(lens_source(at, blit.chroma.z, blit.chroma.w)).b,
        1.0,
    );
    // Keep in step with `post_crop_vignette` in op-core.
    if blit.perspective.w != 0.0 {
        let falloff = smoothstep(0.4, 1.4, length((in.framed - 0.5) * 2.0));
        colour = vec4<f32>(clamp(colour.rgb * (1.0 + blit.perspective.w * falloff), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    }
    let brightest = max(colour.r, max(colour.g, colour.b));
    if blit.flags.y > 0.0 && brightest >= 0.998 {
        colour = vec4<f32>(1.0, 0.0, 0.0, 1.0);
    } else if blit.flags.z > 0.0 && brightest <= 0.004 {
        colour = vec4<f32>(0.0, 0.35, 1.0, 1.0);
    }
    // Rotation can show areas beyond the image; the export leaves those black too.
    let read = lens_source(at, 0.0, 0.0);
    let beyond_photo = blit.perspective.z > 0.0 && (any(read < vec2<f32>(0.0)) || any(read > vec2<f32>(1.0)));
    if beyond_photo || any(in.uv < vec2<f32>(0.0)) || any(in.uv > vec2<f32>(1.0)) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return select(colour, unedited, in.across < blit.compare);
}
