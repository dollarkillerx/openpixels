//! The default look of RAW files: a filmic tone curve, plus an exposure offset chosen
//! per image so its brightness starts out matching the camera's own JPEG.
//!
//! The curve was fitted to camera JPEGs with `examples/tone_survey.rs`. Its constants
//! are repeated in `op-gpu/src/develop.wgsl`.

use crate::LinearImage;
use half::f16;

pub const LOOK_CONTRAST: f32 = 1.5;
pub const LOOK_KNEE: f32 = 0.348;
/// Used when a file has no usable embedded JPEG.
pub const DEFAULT_BASE_EXPOSURE: f32 = 1.0;

const MIN_SAMPLES: usize = 200;
const REFINEMENT_PASSES: usize = 4;
const FIT_RADIUS_SQUARED: f32 = 1.2;
const MAX_FIT_SAMPLES: usize = 6000;
const MATRIX_PARAMETERS: usize = 6;
const FIT_ITERATIONS: usize = 12;
const FIT_STEP: f64 = 1e-3;
const LIGHTNESS_WEIGHT: f64 = 0.25;
const IDENTITY_PULL: f64 = 8.0;
const ROBUST_LIMIT: f64 = 10.0;
/// How much of the colour error a fitted matrix must leave at most.
const MIN_IMPROVEMENT: f64 = 0.9;

/// Maps scene-linear light to display-linear light.
pub fn look_curve(x: f32) -> f32 {
    let toe = x.max(0.0).powf(LOOK_CONTRAST);
    toe / (toe + LOOK_KNEE)
}

fn look_curve_inverse(y: f32) -> f32 {
    (y * LOOK_KNEE / (1.0 - y)).powf(1.0 / LOOK_CONTRAST)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Finds the exposure offset, in EV, that makes `linear` match `display`, an sRGB
/// image of the same size in tightly packed RGB8. `None` if too little of the image
/// is usable, as in a nearly black or blown-out frame.
pub fn estimate_base_exposure(linear: &LinearImage, display: &[u8]) -> Option<f32> {
    let mut samples: Vec<(f32, f32)> = linear
        .pixels
        .chunks_exact(3)
        .zip(display.chunks_exact(3))
        .filter_map(|(rgb, shown)| {
            let [r, g, b] = [rgb[0], rgb[1], rgb[2]].map(|bits| f16::from_bits(bits).to_f32());
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let [sr, sg, sb] = [shown[0], shown[1], shown[2]].map(|v| srgb_to_linear(f32::from(v) / 255.0));
            let target = 0.2126 * sr + 0.7152 * sg + 0.0722 * sb;
            // Deep shadows and clipped highlights say little about exposure.
            (luma > 1e-4 && (0.03..0.85).contains(&target)).then_some((luma, target))
        })
        .collect();
    if samples.len() < MIN_SAMPLES {
        return None;
    }

    // The curve is normalised by where sensor saturation lands, which depends on the
    // exposure being solved for, so converge on it.
    let mut exposure = DEFAULT_BASE_EXPOSURE;
    let mut offsets = vec![0.0f32; samples.len()];
    for _ in 0..REFINEMENT_PASSES {
        let white = look_curve(2.0f32.powf(exposure));
        for (offset, (luma, target)) in offsets.iter_mut().zip(&samples) {
            let unnormalised = (target * white).min(0.999);
            *offset = (look_curve_inverse(unnormalised) / luma).log2();
        }
        let middle = offsets.len() / 2;
        exposure = *offsets.select_nth_unstable_by(middle, f32::total_cmp).1;
    }
    samples.clear();
    Some(exposure)
}

/// The default look, as `develop.wgsl` applies it, without the sRGB encoding.
pub fn base_look(rgb: [f32; 3], exposure: f32) -> [f32; 3] {
    let boost = 2.0f32.powf(exposure);
    let scaled = rgb.map(|v| (v * boost).max(0.0));
    let peak = scaled[0].max(scaled[1]).max(scaled[2]).max(1e-5);
    let kept = look_curve(peak) / peak;
    let white = look_curve(boost);
    scaled.map(|v| (v * kept + look_curve(v)) * 0.5 / white)
}

fn lab(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(|v| v.clamp(0.0, 1.0));
    let xyz = [
        (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.9505,
        0.2126 * r + 0.7152 * g + 0.0722 * b,
        (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.089,
    ];
    let f = xyz.map(|t| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 });
    [116.0 * f[1] - 16.0, 500.0 * (f[0] - f[1]), 200.0 * (f[1] - f[2])]
}

/// A row-preserving colour matrix: each row is identity plus two free weights, with the
/// third set so the row still sums to 1 and neutrals stay neutral.
fn matrix_from(p: &[f64; MATRIX_PARAMETERS]) -> [[f32; 3]; 3] {
    let row = |at: usize, identity: usize| -> [f32; 3] {
        let (a, b) = (p[at] as f32, p[at + 1] as f32);
        let mut values = [a, b, -a - b];
        values[identity] += 1.0;
        values
    };
    [row(0, 0), row(2, 1), row(4, 2)]
}

fn colour_residual(rgb: [f32; 3], target: &[f32; 3], matrix: &[[f32; 3]; 3], exposure: f32) -> [f64; 3] {
    let mixed = matrix.map(|row| row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2]);
    let ours = lab(base_look(mixed, exposure));
    [
        f64::from(ours[0] - target[0]) * LIGHTNESS_WEIGHT,
        f64::from(ours[1] - target[1]),
        f64::from(ours[2] - target[2]),
    ]
}

/// Fits the colour matrix that brings our render closest in colour to the camera's JPEG.
/// Both images must be the same size. `None` if too little of the image is usable.
///
/// The camera's tone curve differs from ours, so lightness counts for little; a pull
/// towards the identity keeps nearly neutral scenes from producing a wild matrix.
pub fn fit_camera_matrix(linear: &LinearImage, display: &[u8], exposure: f32) -> Option<[[f32; 3]; 3]> {
    let samples = colour_samples(linear, display);
    if samples.len() < MIN_SAMPLES {
        return None;
    }
    let cost_of = |p: &[f64; MATRIX_PARAMETERS]| -> f64 {
        let matrix = matrix_from(p);
        let data: f64 = samples
            .iter()
            .map(|(rgb, target)| robust(colour_residual(*rgb, target, &matrix, exposure).iter().map(|v| v * v).sum()))
            .sum();
        data / samples.len() as f64 + IDENTITY_PULL * p.iter().map(|v| v * v).sum::<f64>()
    };

    let mut p = [0.0f64; MATRIX_PARAMETERS];
    let mut cost = cost_of(&p);
    let mut damping = 1e-3;
    for _ in 0..FIT_ITERATIONS {
        let matrix = matrix_from(&p);
        let mut normal = [[0.0f64; MATRIX_PARAMETERS]; MATRIX_PARAMETERS];
        let mut gradient = [0.0f64; MATRIX_PARAMETERS];
        for (rgb, target) in &samples {
            let residual = colour_residual(*rgb, target, &matrix, exposure);
            let squared: f64 = residual.iter().map(|v| v * v).sum();
            let weight = robust(squared) / squared.max(1e-9);
            let jacobian: [[f64; 3]; MATRIX_PARAMETERS] = std::array::from_fn(|k| {
                let mut nudged = p;
                nudged[k] += FIT_STEP;
                let moved = colour_residual(*rgb, target, &matrix_from(&nudged), exposure);
                std::array::from_fn(|c| (moved[c] - residual[c]) / FIT_STEP)
            });
            for i in 0..MATRIX_PARAMETERS {
                gradient[i] += weight * (0..3).map(|c| jacobian[i][c] * residual[c]).sum::<f64>();
                for j in 0..MATRIX_PARAMETERS {
                    normal[i][j] += weight * (0..3).map(|c| jacobian[i][c] * jacobian[j][c]).sum::<f64>();
                }
            }
        }
        let count = samples.len() as f64;
        for i in 0..MATRIX_PARAMETERS {
            gradient[i] = -(gradient[i] / count + IDENTITY_PULL * p[i]);
            for value in &mut normal[i] {
                *value /= count;
            }
            normal[i][i] += IDENTITY_PULL + damping;
        }
        let Some(step) = crate::lens::solve(normal, gradient) else { break };
        let candidate: [f64; MATRIX_PARAMETERS] = std::array::from_fn(|i| p[i] + step[i]);
        let candidate_cost = cost_of(&candidate);
        if candidate_cost < cost {
            p = candidate;
            cost = candidate_cost;
            damping *= 0.5;
        } else {
            damping *= 8.0;
        }
    }

    // Some cameras are already close; a fit that barely helps is not worth offering.
    let identity = matrix_from(&[0.0; MATRIX_PARAMETERS]);
    let matrix = matrix_from(&p);
    let chroma_of = |matrix: &[[f32; 3]; 3]| -> f64 {
        samples
            .iter()
            .map(|(rgb, target)| {
                let [_, a, b] = colour_residual(*rgb, target, matrix, exposure);
                (a * a + b * b).sqrt()
            })
            .sum::<f64>()
    };
    (chroma_of(&matrix) < chroma_of(&identity) * MIN_IMPROVEMENT).then_some(matrix)
}

/// Grows linearly past a threshold, so a few misaligned pixels cannot steer the fit.
fn robust(squared: f64) -> f64 {
    if squared <= ROBUST_LIMIT * ROBUST_LIMIT {
        squared
    } else {
        ROBUST_LIMIT * (2.0 * squared.sqrt() - ROBUST_LIMIT)
    }
}

/// Well-exposed pixels away from the corners, where lens corrections differ.
fn colour_samples(linear: &LinearImage, display: &[u8]) -> Vec<([f32; 3], [f32; 3])> {
    let (width, height) = (linear.width as usize, linear.height as usize);
    let stride = ((width * height / MAX_FIT_SAMPLES) as f32).sqrt().ceil().max(1.0) as usize;
    let mut samples = Vec::new();
    for y in (0..height).step_by(stride) {
        let ny = (y as f32 + 0.5) / height as f32 * 2.0 - 1.0;
        for x in (0..width).step_by(stride) {
            let nx = (x as f32 + 0.5) / width as f32 * 2.0 - 1.0;
            if nx * nx + ny * ny > FIT_RADIUS_SQUARED {
                continue;
            }
            let at = (y * width + x) * 3;
            let rgb = [0, 1, 2].map(|c| f16::from_bits(linear.pixels[at + c]).to_f32());
            let shown = [0, 1, 2].map(|c| srgb_to_linear(f32::from(display[at + c]) / 255.0));
            let brightest = shown[0].max(shown[1]).max(shown[2]);
            if rgb.iter().all(|v| *v > 0.0) && (0.02..0.95).contains(&brightest) {
                samples.push((rgb, lab(shown)));
            }
        }
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linear_to_srgb_byte(linear: f32) -> u8 {
        let c = linear.clamp(0.0, 1.0);
        let encoded = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        (encoded * 255.0).round() as u8
    }

    /// A luminance ramp and the camera JPEG this model predicts for it.
    fn ramp_with_display(exposure: f32) -> (LinearImage, Vec<u8>) {
        let (width, height) = (64u32, 64u32);
        let white = look_curve(2.0f32.powf(exposure));
        let (mut pixels, mut display) = (Vec::new(), Vec::new());
        for i in 0..width * height {
            let value = 0.002 + 0.5 * i as f32 / (width * height) as f32;
            pixels.extend([f16::from_f32(value).to_bits(); 3]);
            let shown = look_curve(value * 2.0f32.powf(exposure)) / white;
            display.extend([linear_to_srgb_byte(shown); 3]);
        }
        (LinearImage::new(width, height, pixels), display)
    }

    #[test]
    fn recovers_a_known_exposure() {
        for expected in [0.0, 0.8, 1.5] {
            let (linear, display) = ramp_with_display(expected);
            let found = estimate_base_exposure(&linear, &display).unwrap();
            assert!((found - expected).abs() < 0.05, "expected {expected}, found {found}");
        }
    }

    /// Colours spread over the gamut, and the JPEG `matrix` would predict for them.
    fn colours_with_display(matrix: [[f32; 3]; 3], exposure: f32) -> (LinearImage, Vec<u8>) {
        let (width, height) = (64u32, 64u32);
        let (mut pixels, mut display) = (Vec::new(), Vec::new());
        for i in 0..width * height {
            let rgb = [0.02 + (i % 7) as f32 * 0.03, 0.02 + (i % 11) as f32 * 0.02, 0.02 + (i % 13) as f32 * 0.018];
            pixels.extend(rgb.map(|v| f16::from_f32(v).to_bits()));
            let mixed = matrix.map(|row| row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2]);
            display.extend(base_look(mixed, exposure).map(linear_to_srgb_byte));
        }
        (LinearImage::new(width, height, pixels), display)
    }

    #[test]
    fn recovers_a_known_colour_matrix() {
        let expected = [[1.15, -0.1, -0.05], [-0.08, 1.12, -0.04], [-0.03, -0.09, 1.12]];
        let (linear, display) = colours_with_display(expected, 1.0);
        let found = fit_camera_matrix(&linear, &display, 1.0).unwrap();
        for row in 0..3 {
            for col in 0..3 {
                assert!((found[row][col] - expected[row][col]).abs() < 0.04, "{found:?}");
            }
        }
    }

    #[test]
    fn a_matching_render_needs_no_matrix() {
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let (linear, display) = colours_with_display(identity, 1.0);
        assert_eq!(fit_camera_matrix(&linear, &display, 1.0), None);
    }

    #[test]
    fn gives_up_on_an_unusable_frame() {
        let linear = LinearImage::new(32, 32, vec![0; 32 * 32 * 3]);
        assert_eq!(estimate_base_exposure(&linear, &vec![0; 32 * 32 * 3]), None);
    }

    #[test]
    fn curve_is_monotonic_and_bounded() {
        let mut previous = 0.0;
        for step in 1..200 {
            let value = look_curve(step as f32 * 0.05);
            assert!(value > previous && value < 1.0);
            previous = value;
        }
    }
}
