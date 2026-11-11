// Global develop pass. Reads scene-linear RGB (sRGB primaries), writes display sRGB,
// or for an export another colour space.
//
// Colours outside sRGB arrive as negative or over-one values, from a wide-gamut file or
// a RAW's camera matrix, and are carried through: tone maps work on signed values, and
// curves and looks, which are defined over 0..1, act on the colour desaturated into
// range and get the excess chroma back after (`gamut_excess`). A wide export can then
// keep them; the 8-bit screen target clips them as it always did.
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
    sharpen_radius: f32,
    sharpen_detail: f32,
    sharpen_masking: f32,
    post_crop_vignette: f32,
    calibration_red_hue: f32,
    calibration_red_sat: f32,
    calibration_green_hue: f32,
    calibration_green_sat: f32,
    calibration_blue_hue: f32,
    calibration_blue_sat: f32,
    calibration_padding: vec2<f32>,
    point_hue: f32,
    point_width: f32,
    point_hue_shift: f32,
    point_sat_shift: f32,
    point_lum_shift: f32,
    output_space: f32,
    point_padding_1: f32,
    point_padding_2: f32,
    // Exposure into the film, halation, grain, frame long edge in mm (0 is 35mm).
    film: vec4<f32>,
    // Printer lights in stops.
    film_lights: vec4<f32>,
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
    // Export colour space: rows of the matrix from linear sRGB, with the transfer curve
    // in the first row's w. 0 leaves sRGB as it is.
    output_red: vec4<f32>,
    output_green: vec4<f32>,
    output_blue: vec4<f32>,
    // Film simulation: 0 off, 1 negative and print, 2 slide; whether black and white; and the
    // exposure in stops that keeps mid grey as it was. The negative LUT is in look_lut.
    film: vec4<f32>,
    // RMS grain per layer at 16 evenly spaced densities, in ADX code per pixel per mm.
    film_grain: array<vec4<f32>, 16>,
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
// Film simulation: from printing densities through the print to display colour.
@group(0) @binding(17) var film_output: texture_3d<f32>;
// Light scattered back by the film base around highlights, spread for 16mm, 35mm and 70mm.
@group(0) @binding(18) var halation_map: texture_2d<f32>;

// This pixel's combined local adjustments: exposure, contrast, saturation, temperature.
var<private> local_values: vec4<f32>;

const LUMA = vec3<f32>(0.2126, 0.7152, 0.0722);
const TONE_GAMMA = 2.2;
const MID_GRAY_TONE = 0.4586; // 0.18 ^ (1 / 2.2)
// Keep in sync with op-core/src/look.rs.
const LOOK_CONTRAST = 1.5;
const LOOK_KNEE = 0.348;

// A power that keeps the sign, so values below 0 survive a trip through gamma.
fn signed_pow(v: vec3<f32>, exponent: f32) -> vec3<f32> {
    return sign(v) * pow(abs(v), vec3<f32>(exponent));
}

// The chroma that desaturating a colour towards its own luminance would have to remove
// to bring it inside the 0..1 cube: what lies outside sRGB. Colours too dark or too
// bright to fit at any saturation, such as noise below black, have none and are left
// to be clipped as before.
fn gamut_excess(c: vec3<f32>) -> vec3<f32> {
    let luma = dot(c, LUMA);
    if luma <= 0.0 || luma >= 1.0 {
        return vec3<f32>(0.0);
    }
    let below = select(vec3<f32>(1.0), luma / (luma - c), c < vec3<f32>(0.0));
    let above = select(vec3<f32>(1.0), (1.0 - luma) / (c - luma), c > vec3<f32>(1.0));
    let fits = min(min(min(below.x, below.y), below.z), min(min(above.x, above.y), above.z));
    return (c - vec3<f32>(luma)) * (1.0 - fits);
}

fn look_curve(x: f32) -> f32 {
    let toe = pow(max(x, 0.0), LOOK_CONTRAST);
    return toe / (toe + LOOK_KNEE);
}

fn lens_light_gain(pixel: vec2<f32>) -> f32 {
    let position = ((pixel + 0.5) / region.full_size - 0.5) * 2.0 * region.lens_aspect;
    let r2 = dot(position, position);
    let v = region.lens_vignette;
    return max(1.0 + r2 * (v.x + r2 * (v.y + r2 * (v.z + r2 * v.w))), 0.0);
}

fn apply_lens_light(rgb: vec3<f32>, pixel: vec2<f32>) -> vec3<f32> {
    return rgb * lens_light_gain(pixel);
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
    let scaled = rgb * boost;
    let peak = max(max(max(scaled.r, scaled.g), scaled.b), 1e-5);
    // Curving the peak keeps hue; curving each channel desaturates bright colours as
    // film does. A blend of the two avoids both neon highlights and hue shifts.
    let hue_kept = scaled * look_curve(peak) / peak;
    // A channel below 0, outside sRGB, has no curve of its own and follows the peak.
    let per_channel = select(vec3<f32>(look_curve(scaled.r), look_curve(scaled.g), look_curve(scaled.b)), hue_kept, scaled < vec3<f32>(0.0));
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
    tone += adj.whites * 0.18 * tone_band(tone, 1.0, 0.5);

    // Exponential contrast around mid gray; continuous at 0 and monotonic.
    let slope = pow(2.0, (adj.contrast + local_values.y) * 1.2);
    let centered = max(tone, 0.0) / MID_GRAY_TONE;
    return MID_GRAY_TONE * pow(centered, slope);
}

// The most a tone edit multiplies a pixel by. Beyond it the rest is added as grey, so
// Blacks and Shadows can lift pure black without multiplying sensor noise without limit.
const MAX_TONE_GAIN = 16.0;

fn apply_tone(input: vec3<f32>) -> vec3<f32> {
    // Below black there is only noise, which lifted blacks must not turn into dark specks.
    let rgb = select(input, max(input, vec3<f32>(0.0)), dot(input, LUMA) <= 0.0);
    let luma = dot(rgb, LUMA);
    let wanted = pow(max(shape_tone(pow(max(luma, 1e-5), 1.0 / TONE_GAMMA)), 0.0), TONE_GAMMA);
    // Scale RGB together so hue survives tone edits. Chroma outside sRGB is not
    // amplified, for in the shadows it is mostly noise.
    let ratio = min(wanted / max(luma, 1e-5), MAX_TONE_GAIN);
    let excess = gamut_excess(rgb);
    return (rgb - excess) * ratio + excess * min(ratio, 1.0) + vec3<f32>(max(wanted - max(luma, 0.0) * ratio, 0.0));
}

fn apply_color(rgb: vec3<f32>) -> vec3<f32> {
    let luma = dot(rgb, LUMA);
    let peak = max(max(rgb.r, rgb.g), rgb.b);
    let trough = min(min(rgb.r, rgb.g), rgb.b);
    let chroma = clamp((peak - trough) / max(peak, 1e-5), 0.0, 1.0);
    // Vibrance acts mostly on muted colours, saturation on all of them equally.
    let amount = max(1.0 + adj.saturation + local_values.z, 0.0) * (1.0 + adj.vibrance * (1.0 - chroma));
    return mix(vec3<f32>(luma), rgb, amount);
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

fn ring_tone(pixel: vec2<i32>, distance: i32) -> f32 {
    let neighbours = source_at(pixel + vec2<i32>(distance, 0)) + source_at(pixel - vec2<i32>(distance, 0))
        + source_at(pixel + vec2<i32>(0, distance)) + source_at(pixel - vec2<i32>(0, distance));
    return pow(max(dot(neighbours * 0.25, LUMA), 1e-5), 1.0 / TONE_GAMMA);
}

// Unsharp mask on perceptual luminance, which avoids colour fringes.
fn apply_sharpen(rgb: vec3<f32>, pixel: vec2<i32>) -> vec3<f32> {
    if adj.sharpen == 0.0 {
        return rgb;
    }
    let luma = max(dot(rgb, LUMA), 1e-5);
    let tone = pow(luma, 1.0 / TONE_GAMMA);
    let finest = ring_tone(pixel, 1);
    let radius = clamp(adj.sharpen_radius, 1.0, 3.0);
    let at_radius = mix(ring_tone(pixel, i32(floor(radius))), ring_tone(pixel, i32(ceil(radius))), fract(radius));
    let around = mix(at_radius, finest, adj.sharpen_detail);
    var strength = adj.sharpen * 2.5;
    if adj.sharpen_masking > 0.0 {
        strength *= smoothstep(adj.sharpen_masking * 0.015, adj.sharpen_masking * 0.1, abs(tone - finest));
    }
    // The neighbours are read before noise reduction, so differences the size of the
    // noise it removed are left alone rather than sharpened back in.
    if adj.denoise_luma > 0.0 {
        let noise = adj.denoise_luma * 0.06;
        strength *= smoothstep(noise, noise * 2.5, abs(tone - finest));
    }
    let sharpened = max(tone + (tone - around) * strength, 0.0);
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
    // Tones above white get no clarity; the weight would otherwise turn negative.
    let midtones = max(1.0 - pow(abs(min(tone, 1.0) * 2.0 - 1.0), 2.0), 0.0);
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
    let tone = signed_pow(rgb, 1.0 / TONE_GAMMA);
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
    return signed_pow(out, TONE_GAMMA);
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

// Saturation above 1 is a colour outside sRGB. Edits may lower it, or raise an sRGB
// colour to 1, but do not push colours further out than they were.
fn shifted_saturation(saturation: f32, shift: f32) -> f32 {
    return clamp(saturation * (1.0 + shift), 0.0, max(saturation, 1.0));
}

fn apply_hsl(rgb: vec3<f32>) -> vec3<f32> {
    let hsv = rgb_to_hsv(rgb);
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
        shifted_saturation(hsv.y, shift.y),
        hsv.z * pow(2.0, shift.z * min(hsv.y, 1.0)),
    );
    return hsv_to_rgb(out);
}

const CURVE_SIZE = 256.0;

fn apply_curve(rgb: vec3<f32>) -> vec3<f32> {
    if adj.curve_active == 0.0 {
        return rgb;
    }
    let tone = signed_pow(rgb, 1.0 / TONE_GAMMA);
    let excess = gamut_excess(tone);
    let at = (clamp(tone - excess, vec3<f32>(0.0), vec3<f32>(1.0)) * (CURVE_SIZE - 1.0) + 0.5) / CURVE_SIZE;
    let mapped = vec3<f32>(
        textureSampleLevel(curve_lut, mask_sampler, vec2<f32>(at.r, 0.5), 0.0).r,
        textureSampleLevel(curve_lut, mask_sampler, vec2<f32>(at.g, 0.5), 0.0).g,
        textureSampleLevel(curve_lut, mask_sampler, vec2<f32>(at.b, 0.5), 0.0).b,
    );
    return signed_pow(mapped + excess, TONE_GAMMA);
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
    let signed = signed_pow(rgb, 1.0 / TONE_GAMMA);
    let excess = gamut_excess(signed);
    let display = max(signed - excess, vec3<f32>(0.0));
    let tone = dot(display, LUMA);
    let shadows = 1.0 - smoothstep(0.0, 0.5, tone);
    let highlights = smoothstep(0.5, 1.0, tone);
    let midtones = 1.0 - shadows - highlights;
    let tinted = display + tint_of(adj.grade[0]) * shadows + tint_of(adj.grade[1]) * midtones
        + tint_of(adj.grade[2]) * highlights;
    return signed_pow(max(tinted, vec3<f32>(0.0)) + excess, TONE_GAMMA);
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
    let toned = signed_pow(centre, 1.0 / TONE_GAMMA);
    let luma = dot(toned, LUMA);
    let sigma_luma = max(adj.denoise_luma * 0.12, 1e-3);
    let sigma_chroma = max(adj.denoise_chroma * 0.25, 1e-3);

    var offsets = DENOISE_OFFSETS;
    var luma_total = luma;
    var luma_weight = 1.0;
    var chroma_total = toned;
    var chroma_weight = 1.0;
    for (var i = 0u; i < 24u; i++) {
        let sample_tone = signed_pow(source_at(pixel + offsets[i]), 1.0 / TONE_GAMMA);
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
    return signed_pow(result, TONE_GAMMA);
}

fn apply_look(display: vec3<f32>) -> vec3<f32> {
    if adj.lut_amount == 0.0 {
        return display;
    }
    let size = vec3<f32>(textureDimensions(look_lut));
    let at = (clamp(display, vec3<f32>(0.0), vec3<f32>(1.0)) * (size - 1.0) + 0.5) / size;
    return mix(display, textureSampleLevel(look_lut, mask_sampler, at, 0.0).rgb, adj.lut_amount);
}

// Keep in sync with op-core/src/film.rs.
const FILM_LOG_MIN = -8.0;
const FILM_LOG_MAX = 7.0;
const FILM_MID = 0.18;
const ADX_PER_DENSITY = vec3<f32>(0.24414435, 0.22461280, 0.23193713);
const DENSITY_PER_STOP = 0.30103;
const FILM_WHITEST = 0.995;
const FILM_GRAIN_SAMPLES = 16u;
// Scattered light reaches the red layer most, through the base below it.
const HALATION_TINT = vec3<f32>(1.0, 0.3, 0.05);

fn sample_cube(lut: texture_3d<f32>, at: vec3<f32>) -> vec3<f32> {
    let size = vec3<f32>(textureDimensions(lut));
    return textureSampleLevel(lut, mask_sampler, (clamp(at, vec3<f32>(0.0), vec3<f32>(1.0)) * (size - 1.0) + 0.5) / size, 0.0).rgb;
}

// Roughly Gaussian noise with a standard deviation of 1, different for each seed.
fn film_noise(pixel: vec2<f32>, seed: i32) -> f32 {
    let at = vec2<i32>(floor(pixel)) * vec2<i32>(4, 1) + vec2<i32>(0, seed * 1000003);
    return cell_hash(at) + cell_hash(at + vec2<i32>(1, 0)) + cell_hash(at + vec2<i32>(2, 0));
}

fn film_grain_sigma(density: vec3<f32>) -> vec3<f32> {
    let position = clamp(density, vec3<f32>(0.0), vec3<f32>(1.0)) * f32(FILM_GRAIN_SAMPLES - 1u);
    let index = min(vec3<u32>(floor(position)), vec3<u32>(FILM_GRAIN_SAMPLES - 2u));
    let t = position - vec3<f32>(index);
    return vec3<f32>(
        mix(region.film_grain[index.x].x, region.film_grain[index.x + 1u].x, t.x),
        mix(region.film_grain[index.y].y, region.film_grain[index.y + 1u].y, t.y),
        mix(region.film_grain[index.z].z, region.film_grain[index.z + 1u].z, t.z),
    );
}

// A film look on the desktop: the display colour is turned back into scene light, exposed
// onto the negative, printed with the printer lights and viewed, with the stock's grain
// in the negative's densities and halation glowing into its red layer. Otherwise the
// look LUT, from display colour to display colour.
fn apply_film(display: vec3<f32>, pixel: vec2<f32>, uv: vec2<f32>, gain: f32) -> vec3<f32> {
    let mode = region.film.x;
    if mode == 0.0 {
        return apply_look(display);
    }
    if adj.lut_amount == 0.0 {
        return display;
    }
    let linear = min(srgb_to_linear(display), vec3<f32>(FILM_WHITEST));
    var scene = pow(max(linear * LOOK_KNEE / (1.0 - linear), vec3<f32>(0.0)), vec3<f32>(1.0 / LOOK_CONTRAST));
    let frame_mm = select(36.0, adj.film.w, adj.film.w > 0.0);
    if adj.film.y > 0.0 {
        let halo = textureSampleLevel(halation_map, mask_sampler, uv, 0.0).rgb;
        let small = mix(halo.r, halo.g, clamp((frame_mm - 12.5) / 23.5, 0.0, 1.0));
        let spread = select(mix(halo.g, halo.b, clamp((frame_mm - 36.0) / 34.0, 0.0, 1.0)), small, frame_mm < 36.0);
        scene += HALATION_TINT * adj.film.y * spread * gain;
    }
    scene *= exp2(region.film.z + adj.film.x);
    let code = (log2(max(scene, vec3<f32>(1e-12)) / FILM_MID) - FILM_LOG_MIN) / (FILM_LOG_MAX - FILM_LOG_MIN);
    var density = sample_cube(look_lut, code);
    let mono = region.film.y > 0.0;
    if mode == 1.0 {
        let per_density = select(ADX_PER_DENSITY, vec3<f32>(ADX_PER_DENSITY.y), mono);
        let lights = select(adj.film_lights.xyz, adj.film_lights.yyy, mono);
        density -= lights * DENSITY_PER_STOP * per_density;
    }
    if adj.film.z > 0.0 {
        let pixels_per_mm = max(region.full_size.x, region.full_size.y) / frame_mm;
        let first = film_noise(pixel, 0);
        let noise = select(vec3<f32>(first, film_noise(pixel, 1), film_noise(pixel, 2)), vec3<f32>(first), mono);
        density += film_grain_sigma(density) * pixels_per_mm * adj.film.z * noise;
    }
    return mix(display, sample_cube(film_output, density), adj.lut_amount);
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

// The sRGB curve, extended to negative and over-one values by symmetry and continuation.
fn linear_to_srgb(linear: vec3<f32>) -> vec3<f32> {
    let c = abs(linear);
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return sign(linear) * select(high, low, c <= vec3<f32>(0.0031308));
}

fn srgb_to_linear(encoded: vec3<f32>) -> vec3<f32> {
    let c = abs(encoded);
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return sign(encoded) * select(high, low, c <= vec3<f32>(0.04045));
}

// For an export to a wider space: from extended sRGB to that space, clipped to its gamut.
fn encode_output(display: vec3<f32>) -> vec3<f32> {
    let transfer = region.output_red.w;
    if transfer == 0.0 {
        return display;
    }
    let linear = srgb_to_linear(display);
    let c = clamp(vec3<f32>(dot(region.output_red.xyz, linear), dot(region.output_green.xyz, linear), dot(region.output_blue.xyz, linear)),
        vec3<f32>(0.0), vec3<f32>(1.0));
    if transfer == 1.0 {
        return linear_to_srgb(c);
    }
    if transfer == 2.0 {
        return pow(c, vec3<f32>(256.0 / 563.0));
    }
    // ProPhoto's ROMM curve, linear near black.
    return select(pow(c, vec3<f32>(1.0 / 1.8)), c * 16.0, c < vec3<f32>(1.0 / 512.0));
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

// Shifts one picked colour, chosen on the unedited source so edits do not move the selection.
fn apply_point_colour(rgb: vec3<f32>, source_rgb: vec3<f32>) -> vec3<f32> {
    if adj.point_width == 0.0 {
        return rgb;
    }
    let weight = range_weight(vec4<f32>(0.0, 1.0, adj.point_hue, adj.point_width), source_rgb);
    if weight == 0.0 {
        return rgb;
    }
    let hsv = rgb_to_hsv(rgb);
    let shifted = hsv_to_rgb(vec3<f32>(
        hsv.x + adj.point_hue_shift * 40.0,
        shifted_saturation(hsv.y, adj.point_sat_shift),
        hsv.z * pow(2.0, adj.point_lum_shift),
    ));
    return mix(rgb, shifted, weight);
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
    // The surround maps are made from the uncorrected proxy, so they get the lens light
    // correction here to stay comparable with the pixel.
    let surround_gain = scene_gain * lens_light_gain(pixel);
    rgb = apply_dehaze(rgb, uv, surround_gain);
    rgb = apply_clarity(rgb, uv, surround_gain);
    rgb = apply_texture(rgb, uv, surround_gain);
    rgb = apply_tone(rgb);
    rgb = apply_curve(rgb);
    rgb = apply_grading(rgb);
    rgb = apply_hsl(rgb);
    rgb = apply_point_colour(rgb, unedited);
    rgb = apply_color(rgb);
    rgb = apply_vignette(rgb, pixel);
    rgb = apply_replacement(rgb, uv, background);

    let tint = vec3<f32>(0.95, 0.2, 0.35);
    // The look and grain see the colour as it fits in sRGB; what lies outside rejoins after.
    let encoded = linear_to_srgb(rgb);
    let excess = gamut_excess(encoded);
    let fitted = clamp(encoded - excess, vec3<f32>(0.0), vec3<f32>(1.0));
    var display = mix(apply_grain(apply_film(fitted, pixel, uv, scene_gain), pixel) + excess, tint, adj.mask_overlay * background * 0.65);
    if adj.local_overlay > 0.0 {
        let layer = min(u32(max(adj.local_overlay, 1.0)) - 1u, 3u);
        display = mix(display, vec3<f32>(0.2, 0.6, 0.95), weights[layer] * 0.55);
    }
    textureStore(target_image, id.xy, vec4<f32>(encode_output(display), 1.0));
}
