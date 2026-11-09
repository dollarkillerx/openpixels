// Global develop pass. Reads scene-linear RGB (sRGB primaries), writes display sRGB.
//
// Tiles render identically to the same area of the full image: the only source
// neighbours read are within `Renderer`'s tile margin, and the mask and blurred
// luminance are sampled by whole-image position.

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
    clarity: f32,
    sharpen: f32,
    padding: f32,
    hsl_hue: array<vec4<f32>, 2>,
    hsl_saturation: array<vec4<f32>, 2>,
    hsl_luminance: array<vec4<f32>, 2>,
}

struct Region {
    // Dimensions of the whole image, of which this dispatch renders a part.
    full_size: vec2<f32>,
    // Position of this dispatch's output (0, 0) within the whole image.
    offset: vec2<f32>,
    // Pixels to render. Can be smaller than the bound textures for edge tiles.
    extent: vec2<f32>,
    // Default RAW look: exposure offset in EV, and whether to apply the look at all.
    base_exposure: f32,
    has_base_look: f32,
    // Position of output (0, 0) in the source texture, which can hold extra margin.
    source_offset: vec2<f32>,
    // Pixels of valid data in the source texture.
    source_size: vec2<f32>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var target_image: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> adj: Adjustments;
@group(0) @binding(3) var<uniform> region: Region;
// Subject mask spanning the whole image; 1 is subject, 0 is background.
@group(0) @binding(4) var subject_mask: texture_2d<f32>;
@group(0) @binding(5) var mask_sampler: sampler;
// Heavily blurred scene-linear luminance of the whole image, for clarity.
@group(0) @binding(6) var blurred_luma: texture_2d<f32>;

const LUMA = vec3<f32>(0.2126, 0.7152, 0.0722);
const TONE_GAMMA = 2.2;
const MID_GRAY_TONE = 0.4586; // 0.18 ^ (1 / 2.2)
// Keep in sync with op-core/src/look.rs.
const LOOK_CONTRAST = 1.5;
const LOOK_KNEE = 0.348;

fn look_curve(x: f32) -> f32 {
    let toe = pow(max(x, 0.0), LOOK_CONTRAST);
    return toe / (toe + LOOK_KNEE);
}

// Turns sensor-referred RAW data into a display-referred image.
fn apply_base_look(rgb: vec3<f32>) -> vec3<f32> {
    if region.has_base_look == 0.0 {
        return rgb;
    }
    let boost = pow(2.0, region.base_exposure);
    let scaled = max(rgb * boost, vec3<f32>(0.0));
    let peak = max(max(max(scaled.r, scaled.g), scaled.b), 1e-5);
    // Curving the peak keeps hue; curving each channel desaturates bright colours as
    // film does. A blend of the two avoids both neon highlights and hue shifts.
    let hue_kept = scaled * look_curve(peak) / peak;
    let per_channel = vec3<f32>(look_curve(scaled.r), look_curve(scaled.g), look_curve(scaled.b));
    // Normalised so that a saturated photosite reaches pure white.
    return mix(hue_kept, per_channel, 0.5) / look_curve(boost);
}

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

fn source_at(pixel: vec2<i32>) -> vec3<f32> {
    let last = vec2<i32>(region.source_size) - vec2<i32>(1);
    return textureLoad(source, clamp(pixel, vec2<i32>(0), last), 0).rgb;
}

// Unsharp mask on perceptual luminance, which avoids colour fringes.
fn apply_sharpen(rgb: vec3<f32>, pixel: vec2<i32>) -> vec3<f32> {
    if adj.sharpen == 0.0 {
        return rgb;
    }
    let neighbours = source_at(pixel + vec2<i32>(1, 0)) + source_at(pixel - vec2<i32>(1, 0))
        + source_at(pixel + vec2<i32>(0, 1)) + source_at(pixel - vec2<i32>(0, 1));
    let luma = max(dot(rgb, LUMA), 1e-5);
    let tone = pow(luma, 1.0 / TONE_GAMMA);
    let around = pow(max(dot(neighbours * 0.25, LUMA), 1e-5), 1.0 / TONE_GAMMA);
    let sharpened = max(tone + (tone - around) * adj.sharpen * 2.5, 0.0);
    return rgb * pow(sharpened / tone, TONE_GAMMA);
}

// Boosts local contrast, weighted to midtones so it does not halo the extremes.
fn apply_clarity(rgb: vec3<f32>, uv: vec2<f32>, scene_gain: f32) -> vec3<f32> {
    if adj.clarity == 0.0 {
        return rgb;
    }
    let luma = max(dot(rgb, LUMA), 1e-5);
    let surround = max(textureSampleLevel(blurred_luma, mask_sampler, uv, 0.0).r, 0.0) * scene_gain;
    let tone = pow(luma, 1.0 / TONE_GAMMA);
    let around = pow(max(apply_base_look(vec3<f32>(surround)).g, 1e-5), 1.0 / TONE_GAMMA);
    let midtones = 1.0 - pow(abs(tone * 2.0 - 1.0), 2.0);
    let shaped = max(tone + (tone - around) * adj.clarity * midtones, 0.0);
    return rgb * pow(shaped / tone, TONE_GAMMA);
}

const HSL_CENTERS = array<f32, 8>(0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0);

fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let peak = max(max(c.r, c.g), c.b);
    let span = peak - min(min(c.r, c.g), c.b);
    if span <= 1e-6 {
        return vec3<f32>(0.0, 0.0, peak);
    }
    var hue: f32;
    if peak == c.r {
        hue = (c.g - c.b) / span;
    } else if peak == c.g {
        hue = 2.0 + (c.b - c.r) / span;
    } else {
        hue = 4.0 + (c.r - c.g) / span;
    }
    return vec3<f32>(fract(hue / 6.0 + 1.0) * 360.0, span / peak, peak);
}

fn hsv_to_rgb(c: vec3<f32>) -> vec3<f32> {
    let h = fract(c.x / 360.0) * 6.0;
    let k = vec3<f32>(5.0, 3.0, 1.0);
    let wave = clamp(min(fract((k + h) / 6.0) * 6.0, 4.0 - fract((k + h) / 6.0) * 6.0), vec3<f32>(0.0), vec3<f32>(1.0));
    return c.z * (vec3<f32>(1.0) - c.y * wave);
}

fn band_shift(band: u32) -> vec3<f32> {
    let group = band / 4u;
    let item = band % 4u;
    return vec3<f32>(
        adj.hsl_hue[group][item],
        adj.hsl_saturation[group][item],
        adj.hsl_luminance[group][item],
    );
}

fn apply_hsl(rgb: vec3<f32>) -> vec3<f32> {
    let hsv = rgb_to_hsv(max(rgb, vec3<f32>(0.0)));
    if hsv.y < 1e-4 {
        return rgb;
    }
    // Interpolate between the two bands around this hue, so a colour at a band's
    // centre answers to that band alone.
    var centers = HSL_CENTERS;
    var lower = 0u;
    for (var band = 1u; band < 8u; band++) {
        if centers[band] <= hsv.x {
            lower = band;
        }
    }
    let upper = (lower + 1u) % 8u;
    let span_end = select(centers[upper], 360.0, upper == 0u);
    let blend = smoothstep(0.0, 1.0, (hsv.x - centers[lower]) / (span_end - centers[lower]));
    let shift = mix(band_shift(lower), band_shift(upper), blend);
    let out = vec3<f32>(
        hsv.x + shift.x * 30.0,
        clamp(hsv.y * (1.0 + shift.y), 0.0, 1.0),
        hsv.z * pow(2.0, shift.z * hsv.y),
    );
    return hsv_to_rgb(out);
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

    let source_pixel = vec2<i32>(id.xy) + vec2<i32>(region.source_offset);
    var rgb = apply_sharpen(source_at(source_pixel), source_pixel);
    rgb = white_balance(rgb);
    let scene_gain = pow(2.0, adj.exposure + adj.background_exposure * background);
    rgb *= scene_gain;
    rgb = apply_base_look(rgb);
    rgb = apply_clarity(rgb, uv, scene_gain);
    rgb = apply_tone(rgb);
    rgb = apply_hsl(rgb);
    rgb = apply_color(rgb);
    rgb = apply_vignette(rgb, pixel);

    let tint = vec3<f32>(0.95, 0.2, 0.35);
    let display = mix(linear_to_srgb(rgb), tint, adj.mask_overlay * background * 0.65);
    textureStore(target_image, id.xy, vec4<f32>(display, 1.0));
}
