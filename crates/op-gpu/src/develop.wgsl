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
    skin_smooth: f32,
    skin_blemish: f32,
    denoise_luma: f32,
    denoise_chroma: f32,
    curve_active: f32,
    hsl_hue: array<vec4<f32>, 2>,
    hsl_saturation: array<vec4<f32>, 2>,
    hsl_luminance: array<vec4<f32>, 2>,
    local: array<vec4<f32>, 4>,
    grade: array<vec4<f32>, 3>,
    local_overlay: f32,
    lut_amount: f32,
    camera_profile: f32,
    lens_auto: f32,
    lens_manual: vec4<f32>,
    texture_amount: f32,
    dehaze: f32,
    grain: f32,
    grain_size: f32,
    perspective: vec2<f32>,
    padding: vec2<f32>,
    local_range: array<vec4<f32>, 4>,
    replace_background: f32,
    replace_background_exposure: f32,
    replace_background_shift: f32,
    replace_padding: f32,
    replace_sky: f32,
    replace_sky_exposure: f32,
    replace_sky_shift: f32,
    replace_sky_relight: f32,
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
    // Lens light falloff correction: gain minus 1 over r², r⁴, r⁶, r⁸.
    lens_vignette: vec4<f32>,
    // Half size of the image with the half diagonal as 1.
    lens_aspect: vec2<f32>,
    // Colour matrix already blended by strength; the rows' w components are unused.
    camera_red: vec4<f32>,
    camera_green: vec4<f32>,
    camera_blue: vec4<f32>,
    // Scene-linear colour of the haze, for dehazing.
    airlight: vec4<f32>,
    // Colour of the new sky relative to neutral, for relighting the rest of the photo.
    sky_tint: vec4<f32>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var target_image: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> adj: Adjustments;
@group(0) @binding(3) var<uniform> region: Region;
// Subject mask spanning the whole image; 1 is subject, 0 is background.
@group(0) @binding(4) var subject_mask: texture_2d<f32>;
// Replacement images, sampled as linear light, and where the sky is.
@group(0) @binding(14) var background_image: texture_2d<f32>;
@group(0) @binding(15) var sky_mask: texture_2d<f32>;
@group(0) @binding(16) var sky_image: texture_2d<f32>;
@group(0) @binding(5) var mask_sampler: sampler;
// Heavily blurred scene-linear luminance of the whole image, for clarity.
@group(0) @binding(6) var blurred_luma: texture_2d<f32>;
// Log2 skin corrections for the whole image.
@group(0) @binding(7) var skin_smooth_map: texture_2d<f32>;
@group(0) @binding(8) var skin_blemish_map: texture_2d<f32>;
// One channel per local adjustment layer.
@group(0) @binding(9) var local_mask: texture_2d<f32>;
// Output tone per channel for each input tone, in one row.
@group(0) @binding(10) var curve_lut: texture_2d<f32>;
// Creative look, from display colour to display colour.
@group(0) @binding(11) var look_lut: texture_3d<f32>;
// Lightly blurred scene-linear luminance of the whole image, for texture.
@group(0) @binding(12) var detail_luma: texture_2d<f32>;
// Smoothed dark channel of the whole image, for dehazing.
@group(0) @binding(13) var haze_map: texture_2d<f32>;

// This pixel's combined local adjustments: exposure, contrast, saturation, temperature.
var<private> local_values: vec4<f32>;

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

fn apply_lens_light(rgb: vec3<f32>, pixel: vec2<f32>) -> vec3<f32> {
    let position = ((pixel + 0.5) / region.full_size - 0.5) * 2.0 * region.lens_aspect;
    let r2 = dot(position, position);
    let v = region.lens_vignette;
    return rgb * max(1.0 + r2 * (v.x + r2 * (v.y + r2 * (v.z + r2 * v.w))), 0.0);
}

fn apply_camera_matrix(rgb: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(region.camera_red.xyz, rgb), dot(region.camera_green.xyz, rgb), dot(region.camera_blue.xyz, rgb));
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
        (adj.temperature + local_values.w) * 0.5 + adj.tint * 0.25,
        -adj.tint * 0.5,
        -(adj.temperature + local_values.w) * 0.5 + adj.tint * 0.25,
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
    let slope = pow(2.0, (adj.contrast + local_values.y) * 1.2);
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
    let amount = max(1.0 + adj.saturation + local_values.z, 0.0) * (1.0 + adj.vibrance * (1.0 - chroma));
    return max(mix(vec3<f32>(luma), rgb, amount), vec3<f32>(0.0));
}

fn apply_skin(rgb: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    if adj.skin_smooth == 0.0 && adj.skin_blemish == 0.0 {
        return rgb;
    }
    let smoothing = textureSampleLevel(skin_smooth_map, mask_sampler, uv, 0.0).rgb;
    let blemishes = textureSampleLevel(skin_blemish_map, mask_sampler, uv, 0.0).rgb;
    return rgb * pow(vec3<f32>(2.0), smoothing * adj.skin_smooth + blemishes * adj.skin_blemish);
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

// Boosts fine detail. The difference is limited so that strong edges do not halo.
fn apply_texture(rgb: vec3<f32>, uv: vec2<f32>, scene_gain: f32) -> vec3<f32> {
    if adj.texture_amount == 0.0 {
        return rgb;
    }
    let luma = max(dot(rgb, LUMA), 1e-5);
    let surround = max(textureSampleLevel(detail_luma, mask_sampler, uv, 0.0).r, 0.0) * scene_gain;
    let tone = pow(luma, 1.0 / TONE_GAMMA);
    let around = pow(max(apply_base_look(vec3<f32>(surround)).g, 1e-5), 1.0 / TONE_GAMMA);
    let detail = clamp(tone - around, -0.12, 0.12);
    let amount = adj.texture_amount * select(0.75, 1.4, adj.texture_amount > 0.0);
    let shaped = max(tone + detail * amount, 0.0);
    return rgb * pow(shaped / tone, TONE_GAMMA);
}

// The haze model: seen = scene * transmission + air * (1 - transmission).
fn apply_dehaze(rgb: vec3<f32>, uv: vec2<f32>, scene_gain: f32) -> vec3<f32> {
    if adj.dehaze == 0.0 {
        return rgb;
    }
    let exponent = vec3<f32>(1.0 / TONE_GAMMA);
    let tone = pow(max(rgb, vec3<f32>(0.0)), exponent);
    let air = pow(max(apply_base_look(white_balance(region.airlight.rgb) * scene_gain), vec3<f32>(0.0)), exponent);
    var out: vec3<f32>;
    if adj.dehaze > 0.0 {
        let scene_dark = max(textureSampleLevel(haze_map, mask_sampler, uv, 0.0).r, 0.0) * scene_gain;
        let dark = pow(max(apply_base_look(vec3<f32>(scene_dark)).g, 1e-5), 1.0 / TONE_GAMMA);
        let transmission = clamp(1.0 - adj.dehaze * 0.9 * dark / max(max(air.r, max(air.g, air.b)), 1e-3), 0.25, 1.0);
        out = (tone - air) / transmission + air;
    } else {
        out = mix(tone, air, -adj.dehaze * 0.6);
    }
    return pow(max(out, vec3<f32>(0.0)), vec3<f32>(TONE_GAMMA));
}

// How far a pixel falls inside a layer's tone and colour limits, judged on the
// unedited image so that editing the layer does not change what it selects.
fn range_weight(limits: vec4<f32>, source_rgb: vec3<f32>) -> f32 {
    const SOFTNESS = 0.08;
    const NEUTRAL_START = 0.04;
    const NEUTRAL_END = 0.15;
    if limits.y == 0.0 {
        return 1.0;
    }
    var weight = 1.0;
    if limits.x > 0.0 || limits.y < 1.0 {
        let tone = pow(max(dot(apply_base_look(source_rgb), LUMA), 1e-5), 1.0 / TONE_GAMMA);
        if limits.x > 0.0 {
            weight *= smoothstep(limits.x - SOFTNESS, limits.x + SOFTNESS, tone);
        }
        if limits.y < 1.0 {
            weight *= 1.0 - smoothstep(limits.y - SOFTNESS, limits.y + SOFTNESS, tone);
        }
    }
    if limits.w > 0.0 {
        let hsv = rgb_to_hsv(max(source_rgb, vec3<f32>(0.0)));
        let away = abs(fract((hsv.x - limits.z) / 360.0 + 0.5) - 0.5) * 360.0;
        weight *= (1.0 - smoothstep(limits.w * 0.6, limits.w, away)) * smoothstep(NEUTRAL_START, NEUTRAL_END, hsv.y);
    }
    return weight;
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

const CURVE_SIZE = 256.0;

fn apply_curve(rgb: vec3<f32>) -> vec3<f32> {
    if adj.curve_active == 0.0 {
        return rgb;
    }
    let tone = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / TONE_GAMMA));
    let at = (tone * (CURVE_SIZE - 1.0) + 0.5) / CURVE_SIZE;
    let mapped = vec3<f32>(
        textureSampleLevel(curve_lut, mask_sampler, vec2<f32>(at.r, 0.5), 0.0).r,
        textureSampleLevel(curve_lut, mask_sampler, vec2<f32>(at.g, 0.5), 0.0).g,
        textureSampleLevel(curve_lut, mask_sampler, vec2<f32>(at.b, 0.5), 0.0).b,
    );
    return pow(max(mapped, vec3<f32>(0.0)), vec3<f32>(TONE_GAMMA));
}

const GRADE_STRENGTH = 0.15;

fn tint_of(setting: vec4<f32>) -> vec3<f32> {
    let colour = hsv_to_rgb(vec3<f32>(setting.x, 1.0, 1.0));
    return (colour - vec3<f32>(dot(colour, LUMA))) * setting.y * GRADE_STRENGTH;
}

// Tints each tonal range without changing its brightness.
fn apply_grading(rgb: vec3<f32>) -> vec3<f32> {
    if adj.grade[0].y == 0.0 && adj.grade[1].y == 0.0 && adj.grade[2].y == 0.0 {
        return rgb;
    }
    let display = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / TONE_GAMMA));
    let tone = dot(display, LUMA);
    let shadows = 1.0 - smoothstep(0.0, 0.5, tone);
    let highlights = smoothstep(0.5, 1.0, tone);
    let midtones = 1.0 - shadows - highlights;
    let tinted = display + tint_of(adj.grade[0]) * shadows + tint_of(adj.grade[1]) * midtones
        + tint_of(adj.grade[2]) * highlights;
    return pow(max(tinted, vec3<f32>(0.0)), vec3<f32>(TONE_GAMMA));
}

const DENOISE_OFFSETS = array<vec2<i32>, 24>(
    vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1),
    vec2<i32>(1, 1), vec2<i32>(-1, 1), vec2<i32>(1, -1), vec2<i32>(-1, -1),
    vec2<i32>(2, 0), vec2<i32>(-2, 0), vec2<i32>(0, 2), vec2<i32>(0, -2),
    vec2<i32>(2, 2), vec2<i32>(-2, 2), vec2<i32>(2, -2), vec2<i32>(-2, -2),
    vec2<i32>(3, 1), vec2<i32>(-3, -1), vec2<i32>(1, -3), vec2<i32>(-1, 3),
    vec2<i32>(4, 0), vec2<i32>(-4, 0), vec2<i32>(0, 4), vec2<i32>(0, -4),
);

// Edge-aware averaging in display-like space. Colour noise is smoothed further than
// luminance noise, because the eye forgives lost colour detail far more readily.
fn apply_denoise(centre: vec3<f32>, pixel: vec2<i32>) -> vec3<f32> {
    if adj.denoise_luma == 0.0 && adj.denoise_chroma == 0.0 {
        return centre;
    }
    let exponent = vec3<f32>(1.0 / TONE_GAMMA);
    let toned = pow(max(centre, vec3<f32>(0.0)), exponent);
    let luma = dot(toned, LUMA);
    let sigma_luma = max(adj.denoise_luma * 0.12, 1e-3);
    let sigma_chroma = max(adj.denoise_chroma * 0.25, 1e-3);

    var offsets = DENOISE_OFFSETS;
    var luma_total = luma;
    var luma_weight = 1.0;
    var chroma_total = toned;
    var chroma_weight = 1.0;
    for (var i = 0u; i < 24u; i++) {
        let sample_tone = pow(max(source_at(pixel + offsets[i]), vec3<f32>(0.0)), exponent);
        let sample_luma = dot(sample_tone, LUMA);
        let difference = sample_luma - luma;
        let for_luma = exp(-difference * difference / (2.0 * sigma_luma * sigma_luma));
        let for_chroma = exp(-difference * difference / (2.0 * sigma_chroma * sigma_chroma));
        luma_total += sample_luma * for_luma;
        luma_weight += for_luma;
        chroma_total += sample_tone * for_chroma;
        chroma_weight += for_chroma;
    }

    let smoothed_luma = mix(luma, luma_total / luma_weight, min(adj.denoise_luma * 1.5, 1.0));
    let smoothed_colour = mix(toned, chroma_total / chroma_weight, min(adj.denoise_chroma * 1.5, 1.0));
    let result = smoothed_colour + vec3<f32>(smoothed_luma - dot(smoothed_colour, LUMA));
    return pow(max(result, vec3<f32>(0.0)), vec3<f32>(TONE_GAMMA));
}

fn apply_look(display: vec3<f32>) -> vec3<f32> {
    if adj.lut_amount == 0.0 {
        return display;
    }
    let size = vec3<f32>(textureDimensions(look_lut));
    let at = (clamp(display, vec3<f32>(0.0), vec3<f32>(1.0)) * (size - 1.0) + 0.5) / size;
    return mix(display, textureSampleLevel(look_lut, mask_sampler, at, 0.0).rgb, adj.lut_amount);
}

fn apply_vignette(rgb: vec3<f32>, pixel: vec2<f32>) -> vec3<f32> {
    let centered = (pixel + 0.5 - region.full_size * 0.5) / (region.full_size * 0.5);
    let falloff = smoothstep(0.35, 1.45, length(centered));
    return rgb * pow(2.0, adj.vignette * 2.0 * falloff);
}

// A 2D hash of integer cell coordinates, the same on every tile.
fn cell_hash(cell: vec2<i32>) -> f32 {
    var v = vec2<u32>(cell) * vec2<u32>(1664525u, 22695477u) + vec2<u32>(1013904223u, 2531011u);
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    v ^= v >> vec2<u32>(16u);
    v.x += v.y * 1664525u;
    return f32(v.x >> 8u) / 16777215.0 * 2.0 - 1.0;
}

fn value_noise(position: vec2<f32>) -> f32 {
    let cell = floor(position);
    let blend = smoothstep(vec2<f32>(0.0), vec2<f32>(1.0), position - cell);
    let at = vec2<i32>(cell);
    let top = mix(cell_hash(at), cell_hash(at + vec2<i32>(1, 0)), blend.x);
    let bottom = mix(cell_hash(at + vec2<i32>(0, 1)), cell_hash(at + vec2<i32>(1, 1)), blend.x);
    return mix(top, bottom, blend.y);
}

// Film grain in display space, strongest in the midtones. Its cells grow with the
// image, so the full-size file looks like the smaller preview.
fn apply_grain(display: vec3<f32>, pixel: vec2<f32>) -> vec3<f32> {
    const REFERENCE_LONG_EDGE = 2048.0;
    const STRENGTH = 0.16;
    if adj.grain == 0.0 {
        return display;
    }
    let cell = mix(1.0, 3.0, adj.grain_size) * max(region.full_size.x, region.full_size.y) / REFERENCE_LONG_EDGE;
    let position = (pixel + 0.5) / max(cell, 1.0);
    let noise = value_noise(position) * 0.75 + value_noise(position * 1.7 + vec2<f32>(37.0, 11.0)) * 0.25;
    let tone = dot(display, LUMA);
    let weight = 0.35 + 0.65 * (1.0 - pow(abs(tone * 2.0 - 1.0), 2.0));
    // Cells smaller than a pixel average out, as they would in a downscaled scan.
    return display + vec3<f32>(noise * adj.grain * STRENGTH * weight * min(cell, 1.0));
}

fn linear_to_srgb(linear: vec3<f32>) -> vec3<f32> {
    let c = clamp(linear, vec3<f32>(0.0), vec3<f32>(1.0));
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, c <= vec3<f32>(0.0031308));
}

// 16 * 16 = 256 invocations, the WebGPU default limit per workgroup.
// A replacement image at a whole-image position, filling the frame without stretching
// and showing the centre of what does not fit.
fn replacement_at(image: texture_2d<f32>, uv: vec2<f32>, shift: f32, exposure: f32) -> vec3<f32> {
    let image_size = vec2<f32>(textureDimensions(image));
    let ratio = (region.full_size.x / region.full_size.y) / (image_size.x / image_size.y);
    let visible = select(vec2<f32>(1.0, 1.0 / ratio), vec2<f32>(ratio, 1.0), ratio < 1.0);
    var at = (uv - 0.5) * visible + 0.5;
    at.y -= shift;
    return textureSampleLevel(image, mask_sampler, at, 0.0).rgb * pow(2.0, exposure);
}

// Blends in the replacements. Comes after the tone and colour edits, which are tuned to
// the photo, and before the look and grain, which unify them.
fn apply_replacement(rgb: vec3<f32>, uv: vec2<f32>, background: f32) -> vec3<f32> {
    var out = rgb;
    var sky = 0.0;
    if adj.replace_sky > 0.0 {
        // Tightened, because a half-covered edge lets the old, usually brighter sky through.
        sky = smoothstep(0.3, 0.7, textureSampleLevel(sky_mask, mask_sampler, uv, 0.0).r);
        out *= mix(vec3<f32>(1.0), region.sky_tint.rgb, adj.replace_sky_relight * adj.replace_sky * (1.0 - sky));
    }
    if adj.replace_background > 0.0 {
        // Favours the new background, so fine hair does not keep a rim of the old one.
        let alpha = smoothstep(0.1, 0.75, background) * (1.0 - sky) * adj.replace_background;
        let fill = replacement_at(background_image, uv, adj.replace_background_shift, adj.replace_background_exposure);
        out = mix(out, fill, clamp(alpha, 0.0, 1.0));
    }
    if adj.replace_sky > 0.0 {
        // With a background replaced too, the subject mask is known and can stand in front.
        var alpha = sky * adj.replace_sky;
        if adj.replace_background > 0.0 {
            alpha *= background;
        }
        let fill = replacement_at(sky_image, uv, adj.replace_sky_shift, adj.replace_sky_exposure);
        out = mix(out, fill, clamp(alpha, 0.0, 1.0));
    }
    return out;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if f32(id.x) >= region.extent.x || f32(id.y) >= region.extent.y {
        return;
    }

    let pixel = vec2<f32>(id.xy) + region.offset;
    let uv = (pixel + 0.5) / region.full_size;
    let background = 1.0 - textureSampleLevel(subject_mask, mask_sampler, uv, 0.0).r;

    let source_pixel = vec2<i32>(id.xy) + vec2<i32>(region.source_offset);
    let unedited = source_at(source_pixel);
    var weights = textureSampleLevel(local_mask, mask_sampler, uv, 0.0);
    for (var layer = 0u; layer < 4u; layer++) {
        weights[layer] *= range_weight(adj.local_range[layer], unedited);
    }
    local_values = adj.local[0] * weights.r + adj.local[1] * weights.g
        + adj.local[2] * weights.b + adj.local[3] * weights.a;

    var rgb = apply_denoise(unedited, source_pixel);
    rgb = apply_sharpen(rgb, source_pixel);
    rgb = apply_skin(rgb, uv);
    rgb = apply_lens_light(rgb, pixel);
    rgb = apply_camera_matrix(rgb);
    rgb = white_balance(rgb);
    let scene_gain = pow(2.0, adj.exposure + adj.background_exposure * background + local_values.x);
    rgb *= scene_gain;
    rgb = apply_base_look(rgb);
    rgb = apply_dehaze(rgb, uv, scene_gain);
    rgb = apply_clarity(rgb, uv, scene_gain);
    rgb = apply_texture(rgb, uv, scene_gain);
    rgb = apply_tone(rgb);
    rgb = apply_curve(rgb);
    rgb = apply_grading(rgb);
    rgb = apply_hsl(rgb);
    rgb = apply_color(rgb);
    rgb = apply_vignette(rgb, pixel);
    rgb = apply_replacement(rgb, uv, background);

    let tint = vec3<f32>(0.95, 0.2, 0.35);
    var display = mix(apply_grain(apply_look(linear_to_srgb(rgb)), pixel), tint, adj.mask_overlay * background * 0.65);
    if adj.local_overlay > 0.0 {
        let layer = u32(adj.local_overlay) - 1u;
        display = mix(display, vec3<f32>(0.2, 0.6, 0.95), weights[layer] * 0.55);
    }
    textureStore(target_image, id.xy, vec4<f32>(display, 1.0));
}
