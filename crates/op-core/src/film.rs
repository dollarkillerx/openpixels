//! Film simulation: a negative LUT and an output LUT, made by scripts/generate-film-looks.py
//! from the film data in spectral_film_lut (MIT), joined by what happens between camera and
//! print. The shader does the same per pixel; this side balances exposure for a pairing and
//! makes the halation map.
//!
//! The negative LUT maps scene light, log2-coded per channel around mid grey, to the
//! densities a printer sees, ADX-coded. The output LUT maps those densities through a print
//! stock (or a slide's own projection) to display sRGB.

use half::f16;

use crate::LinearImage;

/// Keep in sync with scripts/generate-film-looks.py and the shader.
pub const LOG_MIN: f32 = -8.0;
pub const LOG_MAX: f32 = 7.0;
pub const MID: f32 = 0.18;
pub const ADX_SCALING: f32 = 2.0;
/// ADX code per unit of density, per channel.
pub const ADX_PER_DENSITY: [f32; 3] = [1.00 * 8000.0 / 65535.0 * ADX_SCALING, 0.92 * 8000.0 / 65535.0 * ADX_SCALING, 0.95 * 8000.0 / 65535.0 * ADX_SCALING];
/// Density change for one stop of printer light: log10(2).
pub const DENSITY_PER_STOP: f32 = std::f32::consts::LOG10_2;
/// Display mid grey, which the default exposure keeps as bright.
pub const MID_GREY: f32 = 0.5;
pub const GRAIN_SAMPLES: usize = 16;
/// Film frame long edges in millimetres, for the formats offered.
pub const FORMATS_MM: [f32; 3] = [12.5, 36.0, 70.0];
const HALATION_LONG_EDGE: u32 = 384;
/// The spread of light scattered back from the film base, in millimetres of film.
const HALATION_SPREAD_MM: f32 = 0.3;
/// Scene light, after the default tone curve is undone, from which highlights glow.
const HALATION_THRESHOLD: f32 = 2.0;

// The default tone curve (look.rs), undone to recover scene light from display colour.
const LOOK_CONTRAST: f32 = 1.5;
const LOOK_KNEE: f32 = 0.348;
const WHITEST: f32 = 0.995;

/// A 3D LUT: `size`^3 RGB triples, red varying fastest.
#[derive(Clone, Debug)]
pub struct FilmLut {
    pub size: u32,
    pub data: Vec<f32>,
}

impl FilmLut {
    /// Samples at a point in 0..=1, with the trilinear filtering the GPU uses.
    pub fn sample(&self, at: [f32; 3]) -> [f32; 3] {
        let n = self.size as usize;
        let last = (n - 1) as f32;
        let position = at.map(|v| v.clamp(0.0, 1.0) * last);
        let base = position.map(|p| (p as usize).min(n - 2));
        let t: [f32; 3] = std::array::from_fn(|i| position[i] - base[i] as f32);
        let mut out = [0.0; 3];
        for corner in 0..8 {
            let step = [corner & 1, corner >> 1 & 1, corner >> 2 & 1];
            let weight: f32 = (0..3).map(|i| if step[i] == 1 { t[i] } else { 1.0 - t[i] }).product();
            let index = (((base[2] + step[2]) * n + base[1] + step[1]) * n + base[0] + step[0]) * 3;
            for (value, corner_value) in out.iter_mut().zip(&self.data[index..index + 3]) {
                *value += weight * corner_value;
            }
        }
        out
    }
}

/// A negative paired with a print, or a slide with its projection.
pub struct Film {
    pub negative: FilmLut,
    pub output: FilmLut,
    /// Slides are projected, so printer lights do nothing.
    pub slide: bool,
    /// Black and white: one layer, so grain is the same in every channel.
    pub mono: bool,
}

fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Scene light for a display value, undoing the default tone curve.
pub fn display_to_scene(display: f32) -> f32 {
    let linear = srgb_decode(display.clamp(0.0, 1.0)).min(WHITEST);
    (linear * LOOK_KNEE / (1.0 - linear)).powf(1.0 / LOOK_CONTRAST)
}

/// The negative LUT's input for scene light.
pub fn log_code(scene: f32) -> f32 {
    ((scene.max(1e-12) / MID).log2() - LOG_MIN) / (LOG_MAX - LOG_MIN)
}

/// How printer lights, in stops, move the ADX-coded densities.
pub fn printer_light_offset(lights: [f32; 3], mono: bool) -> [f32; 3] {
    if mono {
        let shift = -lights[1] * DENSITY_PER_STOP * ADX_PER_DENSITY[1];
        return [shift; 3];
    }
    std::array::from_fn(|c| -lights[c] * DENSITY_PER_STOP * ADX_PER_DENSITY[c])
}

impl Film {
    /// A display colour through the film, as the shader computes it without grain or halation.
    pub fn display(&self, display: [f32; 3], exposure: f32, lights: [f32; 3]) -> [f32; 3] {
        let gain = exposure.exp2();
        let code = display.map(|v| log_code(display_to_scene(v) * gain));
        let mut density = self.negative.sample(code);
        if !self.slide {
            let offset = printer_light_offset(lights, self.mono);
            for c in 0..3 {
                density[c] += offset[c];
            }
        }
        self.output.sample(density).map(|v| v.clamp(0.0, 1.0))
    }

    /// The exposure, in stops, that leaves mid grey as bright as it went in, so choosing a
    /// pairing changes colour and contrast but not overall brightness. As the generator does.
    pub fn balance(&self) -> f32 {
        let (mut low, mut high) = (-4.0f32, 4.0f32);
        for _ in 0..30 {
            let middle = (low + high) / 2.0;
            let out = self.display([MID_GREY; 3], middle, [0.0; 3]);
            if 0.2126 * out[0] + 0.7152 * out[1] + 0.0722 * out[2] > MID_GREY {
                high = middle;
            } else {
                low = middle;
            }
        }
        (low + high) / 2.0
    }
}

/// Blurs a single-channel image with a Gaussian of about `sigma` pixels, as three box blurs.
fn blur(values: &mut [f32], width: usize, height: usize, sigma: f32) {
    // Box widths for three passes approximating the Gaussian (Kovesi).
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let lower = (ideal.floor() as usize).max(1) | 1;
    let lower = if lower as f32 > ideal { lower.saturating_sub(2).max(1) } else { lower };
    let upper = lower + 2;
    let wanted = ((12.0 * sigma * sigma - (3 * lower * lower) as f32 - (12 * lower) as f32 - 9.0) / (-4.0 * lower as f32 - 4.0)).round().max(0.0) as usize;
    let mut scratch = vec![0.0; values.len()];
    for pass in 0..3 {
        let radius = (if pass < wanted { lower } else { upper }) / 2;
        box_pass(values, &mut scratch, width, height, radius, true);
        box_pass(&scratch, values, width, height, radius, false);
    }
}

fn box_pass(from: &[f32], to: &mut [f32], width: usize, height: usize, radius: usize, across: bool) {
    let (lines, length) = if across { (height, width) } else { (width, height) };
    let at = |line: usize, i: usize| if across { line * width + i } else { i * width + line };
    let span = (2 * radius + 1) as f32;
    for line in 0..lines {
        let value = |i: isize| from[at(line, i.clamp(0, length as isize - 1) as usize)];
        let mut sum: f32 = (-(radius as isize)..=radius as isize).map(value).sum();
        for i in 0..length {
            to[at(line, i)] = sum / span;
            sum += value(i as isize + radius as isize + 1) - value(i as isize - radius as isize);
        }
    }
}

impl LinearImage {
    /// Light that glows from highlights by halation, as `(width, height, RGBA f16 bits)`:
    /// red, green and blue hold the spread for 16mm, 35mm and 70mm film. Values are scene
    /// light above the threshold, after `scene_gain`, the default RAW exposure.
    pub fn halation_map(&self, scene_gain: Option<f32>) -> (u32, u32, Vec<u16>) {
        let small = self.proxy(HALATION_LONG_EDGE);
        let (width, height) = (small.width as usize, small.height as usize);
        let source: Vec<f32> = small
            .pixels
            .chunks_exact(3)
            .map(|rgb| {
                let red = f16::from_bits(rgb[0]).to_f32().max(0.0);
                // A RAW is scene light already; anything else is display light, whose highlights
                // the default curve's inverse spreads back out.
                let scene = match scene_gain {
                    Some(gain) => red * gain,
                    None => display_to_scene(srgb_encode(red)),
                };
                (scene - HALATION_THRESHOLD).max(0.0)
            })
            .collect();
        let long_edge = width.max(height) as f32;
        let layers: Vec<Vec<f32>> = FORMATS_MM
            .iter()
            .map(|frame_mm| {
                let sigma = HALATION_SPREAD_MM / frame_mm * long_edge;
                // A tight core and a wide tail, as scattered light falls off.
                let mut near = source.clone();
                blur(&mut near, width, height, sigma);
                let mut far = source.clone();
                blur(&mut far, width, height, sigma * 3.0);
                near.iter().zip(&far).map(|(a, b)| 0.6 * a + 0.4 * b).collect()
            })
            .collect();
        let one = f16::from_f32(1.0).to_bits();
        let mut rgba = Vec::with_capacity(width * height * 4);
        for ((small_film, medium), large) in layers[0].iter().zip(&layers[1]).zip(&layers[2]) {
            rgba.extend([f16::from_f32(*small_film).to_bits(), f16::from_f32(*medium).to_bits(), f16::from_f32(*large).to_bits(), one]);
        }
        (small.width, small.height, rgba)
    }
}

fn srgb_encode(linear: f32) -> f32 {
    if linear <= 0.003_130_8 { linear * 12.92 } else { 1.055 * linear.powf(1.0 / 2.4) - 0.055 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_lut(path: &str) -> FilmLut {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
        let bytes = std::fs::read(format!("{root}{path}")).unwrap_or_else(|e| panic!("{path}: {e}"));
        let data: Vec<f32> = bytes.chunks_exact(2).map(|b| f32::from(u16::from_le_bytes([b[0], b[1]])) / 65535.0).collect();
        let size = ((data.len() / 3) as f64).cbrt().round() as u32;
        FilmLut { size, data }
    }

    #[test]
    fn the_composed_web_looks_match_the_desktop_pairs() {
        // Portra 400 on Portra Endura, the web's look 31, from the desktop's two LUTs.
        let film = Film { negative: read_lut("desktop/film-sim/negative-31.lut"), output: read_lut("desktop/film-sim/print-1.lut"), slide: false, mono: false };
        let web = read_lut("web/public/film-looks/31.lut");
        let exposure = film.balance();
        assert!((exposure + 0.22).abs() < 0.02, "{exposure}");
        let n = web.size as usize;
        for (r, g, b) in [(0, 0, 0), (16, 16, 16), (32, 32, 32), (5, 20, 30), (30, 8, 2)] {
            let expected = &web.data[((b * n + g) * n + r) * 3..][..3];
            let got = film.display([r, g, b].map(|i| i as f32 / (n - 1) as f32), exposure, [0.0; 3]);
            for c in 0..3 {
                assert!((got[c] - expected[c]).abs() < 2e-3, "{r},{g},{b}: {got:?} vs {expected:?}");
            }
        }
        // A stop more red light makes the print less red... for a negative, more cyan.
        let warmer = film.display([MID_GREY; 3], exposure, [0.0, 0.0, 0.0]);
        let lit = film.display([MID_GREY; 3], exposure, [0.3, 0.0, 0.0]);
        assert!(lit[0] != warmer[0]);
    }

    #[test]
    fn mid_grey_stays_put_and_brighter_exposure_brightens() {
        let film = Film { negative: read_lut("desktop/film-sim/negative-46.lut"), output: read_lut("desktop/film-sim/print-7.lut"), slide: false, mono: false };
        let exposure = film.balance();
        let grey = film.display([MID_GREY; 3], exposure, [0.0; 3]);
        assert!((0.2126 * grey[0] + 0.7152 * grey[1] + 0.0722 * grey[2] - MID_GREY).abs() < 0.01);
        let brighter = film.display([MID_GREY; 3], exposure + 1.0, [0.0; 3]);
        assert!(brighter[1] > grey[1] + 0.05);
    }

    #[test]
    fn halation_glows_around_highlights_only() {
        let (w, h) = (200u32, 100u32);
        let mut pixels = vec![f16::from_f32(0.2).to_bits(); (w * h * 3) as usize];
        for y in 45..55 {
            for x in 95..105 {
                let i = ((y * w + x) * 3) as usize;
                pixels[i] = f16::from_f32(1.0).to_bits();
            }
        }
        let (mw, mh, map) = LinearImage::new(w, h, pixels).halation_map(None);
        assert_eq!((mw, mh), (w, h));
        let at = |x: u32, y: u32, c: usize| f16::from_bits(map[((y * mw + x) * 4) as usize + c]).to_f32();
        assert!(at(100, 50, 1) > 0.5, "the highlight glows");
        assert!(at(110, 50, 1) > 0.01, "the glow spreads");
        assert_eq!(at(10, 10, 1), 0.0, "dark areas stay dark");
        assert!(at(115, 50, 0) > at(115, 50, 2), "small formats spread further");
    }
}
