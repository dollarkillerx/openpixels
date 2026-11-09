//! Tone curves: a master curve and one per channel, each with five control points at
//! fixed inputs. Points are stored as offsets from the identity, so all zeros is flat.

use half::f16;
use serde::Deserialize;

const POINTS: usize = 5;
const SPACING: f32 = 0.25;
/// Entries in the lookup table given to the GPU.
pub const CURVE_LUT_SIZE: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ToneCurve {
    /// Offsets for the master, red, green and blue curves.
    pub curve: [[f32; POINTS]; 4],
}

/// Monotone cubic interpolation (Fritsch-Carlson), which cannot overshoot its points.
fn evaluate(offsets: &[f32; POINTS], x: f32) -> f32 {
    let values: [f32; POINTS] = std::array::from_fn(|i| i as f32 * SPACING + offsets[i]);
    let secants: [f32; POINTS - 1] = std::array::from_fn(|i| (values[i + 1] - values[i]) / SPACING);
    let tangents: [f32; POINTS] = std::array::from_fn(|i| match i {
        0 => secants[0],
        4 => secants[3],
        _ if secants[i - 1] * secants[i] <= 0.0 => 0.0,
        _ => 2.0 * secants[i - 1] * secants[i] / (secants[i - 1] + secants[i]),
    });

    let x = x.clamp(0.0, 1.0);
    let segment = ((x / SPACING) as usize).min(POINTS - 2);
    let t = (x - segment as f32 * SPACING) / SPACING;
    let (t2, t3) = (t * t, t * t * t);
    let value = (2.0 * t3 - 3.0 * t2 + 1.0) * values[segment]
        + (t3 - 2.0 * t2 + t) * SPACING * tangents[segment]
        + (-2.0 * t3 + 3.0 * t2) * values[segment + 1]
        + (t3 - t2) * SPACING * tangents[segment + 1];
    value.clamp(0.0, 1.0)
}

impl ToneCurve {
    pub fn is_identity(&self) -> bool {
        self.curve.iter().flatten().all(|&offset| offset == 0.0)
    }

    /// One row of RGBA half-float bits, with the master curve applied before each channel's.
    pub fn lut(&self) -> Vec<u16> {
        let mut out = Vec::with_capacity(CURVE_LUT_SIZE * 4);
        let one = f16::from_f32(1.0).to_bits();
        for i in 0..CURVE_LUT_SIZE {
            let master = evaluate(&self.curve[0], i as f32 / (CURVE_LUT_SIZE - 1) as f32);
            for channel in 1..4 {
                out.push(f16::from_f32(evaluate(&self.curve[channel], master)).to_bits());
            }
            out.push(one);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(lut: &[u16], index: usize, channel: usize) -> f32 {
        f16::from_bits(lut[index * 4 + channel]).to_f32()
    }

    #[test]
    fn the_default_curve_is_the_identity() {
        let curve = ToneCurve::default();
        assert!(curve.is_identity());
        let lut = curve.lut();
        for index in [0, 64, 128, 200, 255] {
            let expected = index as f32 / 255.0;
            assert!((entry(&lut, index, 0) - expected).abs() < 1e-3, "index {index}");
        }
    }

    #[test]
    fn raising_the_midpoint_brightens_midtones_only() {
        let mut curve = ToneCurve::default();
        curve.curve[0][2] = 0.2;
        let lut = curve.lut();
        assert!(entry(&lut, 128, 0) > 0.65);
        assert_eq!(entry(&lut, 0, 0), 0.0);
        assert_eq!(entry(&lut, 255, 0), 1.0);
    }

    #[test]
    fn a_channel_curve_affects_only_that_channel() {
        let mut curve = ToneCurve::default();
        curve.curve[1][2] = 0.2;
        let lut = curve.lut();
        assert!(entry(&lut, 128, 0) > 0.65, "red changes");
        assert!((entry(&lut, 128, 1) - 128.0 / 255.0).abs() < 1e-3, "green does not");
    }

    #[test]
    fn the_curve_does_not_overshoot_between_points() {
        let mut curve = ToneCurve::default();
        curve.curve[0] = [0.0, 0.25, 0.0, 0.0, 0.0];
        let lut = curve.lut();
        let values: Vec<f32> = (0..256).map(|i| entry(&lut, i, 0)).collect();
        assert!(values.windows(2).all(|pair| pair[1] >= pair[0] - 1e-3), "curve dips");
    }
}
