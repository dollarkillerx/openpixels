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
