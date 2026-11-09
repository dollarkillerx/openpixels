// Global develop pass. Reads scene-linear RGB (sRGB primaries), writes display sRGB.
//
// Evaluated per pixel with no neighbourhood access to the source (the mask is sampled
// by whole-image position), so a tile needs no overlap and renders identically to
// the same area of the full image. Anything spatial added
// later (clarity, sharpening) will need tile overlap in `Renderer::export_rgb8`.

struct Adjustments {
    temperature: f32,
    tint: f32,
    exposure: f32,
    contrast: f32,
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
    vibrance: f32,
    saturation: f32,
    vignette: f32,
    background_exposure: f32,
    mask_overlay: f32,
}

struct Region {
    // Dimensions of the whole image, of which this dispatch renders a part.
    full_size: vec2<f32>,
    // Position of this dispatch's output (0, 0) within the whole image.
    offset: vec2<f32>,
    // Pixels to render. Can be smaller than the bound textures for edge tiles.
    extent: vec2<f32>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var target_image: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> adj: Adjustments;
@group(0) @binding(3) var<uniform> region: Region;
// Subject mask spanning the whole image; 1 is subject, 0 is background.
@group(0) @binding(4) var subject_mask: texture_2d<f32>;
@group(0) @binding(5) var mask_sampler: sampler;

const LUMA = vec3<f32>(0.2126, 0.7152, 0.0722);
const TONE_GAMMA = 2.2;
const MID_GRAY_TONE = 0.4586; // 0.18 ^ (1 / 2.2)

fn white_balance(rgb: vec3<f32>) -> vec3<f32> {
    let gain = pow(vec3<f32>(2.0), vec3<f32>(
        adj.temperature * 0.5 + adj.tint * 0.25,
        -adj.tint * 0.5,
        -adj.temperature * 0.5 + adj.tint * 0.25,
    ));
    // Keep neutral luminance fixed so temperature does not read as exposure.
    return rgb * gain / dot(gain, LUMA);
}

// Weight that peaks at `center` and fades to 0 at `center +- width`.
fn tone_band(tone: f32, center: f32, width: f32) -> f32 {
    let distance = clamp(abs(tone - center) / width, 0.0, 1.0);
    return 1.0 - smoothstep(0.0, 1.0, distance);
}

// Works on a display-referred tone value, because that is where these sliders are
// perceptually even.
fn shape_tone(input: f32) -> f32 {
    var tone = input;
    tone += adj.blacks * 0.12 * tone_band(tone, 0.0, 0.35);
    tone += adj.shadows * 0.20 * tone_band(tone, 0.25, 0.30);
    tone += adj.highlights * 0.20 * tone_band(tone, 0.75, 0.30);
    tone += adj.whites * 0.12 * tone_band(tone, 1.0, 0.35);

    // Exponential contrast around mid gray; continuous at 0 and monotonic.
    let slope = pow(2.0, adj.contrast * 1.2);
    let centered = max(tone, 0.0) / MID_GRAY_TONE;
    return MID_GRAY_TONE * pow(centered, slope);
}

fn apply_tone(rgb: vec3<f32>) -> vec3<f32> {
    let luma = max(dot(rgb, LUMA), 1e-5);
    let shaped = max(shape_tone(pow(luma, 1.0 / TONE_GAMMA)), 0.0);
    // Scale RGB together so hue survives tone edits.
    return rgb * pow(shaped, TONE_GAMMA) / luma;
}

fn apply_color(rgb: vec3<f32>) -> vec3<f32> {
    let luma = dot(rgb, LUMA);
    let peak = max(max(rgb.r, rgb.g), rgb.b);
    let trough = min(min(rgb.r, rgb.g), rgb.b);
    let chroma = clamp((peak - trough) / max(peak, 1e-5), 0.0, 1.0);
    // Vibrance acts mostly on muted colours, saturation on all of them equally.
    let amount = (1.0 + adj.saturation) * (1.0 + adj.vibrance * (1.0 - chroma));
    return max(mix(vec3<f32>(luma), rgb, amount), vec3<f32>(0.0));
}

fn apply_vignette(rgb: vec3<f32>, pixel: vec2<f32>) -> vec3<f32> {
    let centered = (pixel + 0.5 - region.full_size * 0.5) / (region.full_size * 0.5);
    let falloff = smoothstep(0.35, 1.45, length(centered));
    return rgb * pow(2.0, adj.vignette * 2.0 * falloff);
}

fn linear_to_srgb(linear: vec3<f32>) -> vec3<f32> {
    let c = clamp(linear, vec3<f32>(0.0), vec3<f32>(1.0));
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, c <= vec3<f32>(0.0031308));
}

// 16 * 16 = 256 invocations, the WebGPU default limit per workgroup.
@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if f32(id.x) >= region.extent.x || f32(id.y) >= region.extent.y {
        return;
    }

    let pixel = vec2<f32>(id.xy) + region.offset;
    let uv = (pixel + 0.5) / region.full_size;
    let background = 1.0 - textureSampleLevel(subject_mask, mask_sampler, uv, 0.0).r;

    var rgb = textureLoad(source, id.xy, 0).rgb;
    rgb = white_balance(rgb);
    rgb *= pow(2.0, adj.exposure + adj.background_exposure * background);
    rgb = apply_tone(rgb);
    rgb = apply_color(rgb);
    rgb = apply_vignette(rgb, pixel);

    let tint = vec3<f32>(0.95, 0.2, 0.35);
    let display = mix(linear_to_srgb(rgb), tint, adj.mask_overlay * background * 0.65);
    textureStore(target_image, id.xy, vec4<f32>(display, 1.0));
}
