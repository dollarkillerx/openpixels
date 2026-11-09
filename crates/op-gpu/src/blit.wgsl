// Draws the developed preview into the canvas, cropped and rotated, and letterboxed
// to preserve its aspect ratio.

struct Blit {
    scale: vec2<f32>,
    // Source UV of a screen position (s, t): uv_origin + s * uv_axis_s + t * uv_axis_t.
    uv_origin: vec2<f32>,
    uv_axis_s: vec2<f32>,
    uv_axis_t: vec2<f32>,
}

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var<uniform> blit: Blit;
// Liquify offsets in UV.
@group(0) @binding(3) var warp: texture_2d<f32>;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    // One triangle strip quad.
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    var out: VertexOut;
    out.position = vec4<f32>((corner * 2.0 - 1.0) * blit.scale, 0.0, 1.0);
    out.uv = blit.uv_origin + corner.x * blit.uv_axis_s + (1.0 - corner.y) * blit.uv_axis_t;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let offset = textureSample(warp, image_sampler, in.uv).rg;
    let colour = textureSample(image, image_sampler, in.uv + offset);
    // Rotation can show areas beyond the image; the export leaves those black too.
    if any(in.uv < vec2<f32>(0.0)) || any(in.uv > vec2<f32>(1.0)) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return colour;
}
