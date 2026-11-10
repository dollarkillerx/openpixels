//! Tone curves: a master curve and one per channel, each through any number of control
//! points. The same spline is drawn by `web/src/curve.ts`; both are checked against
//! `curve_cases.json`, so they cannot drift apart unnoticed.

use half::f16;
use serde::Deserialize;

/// Entries in the lookup table given to the GPU.
pub const CURVE_LUT_SIZE: usize = 256;
/// Control points closer together than this count as one.
const MIN_SPACING: f32 = 1e-4;
const IDENTITY_TOLERANCE: f32 = 1e-5;
/// How far a parametric slider at full travel moves its region of the curve.
const PARAMETRIC_REACH: f32 = 0.1;
/// Centre and half-width of each parametric region: highlights, lights, darks, shadows.
/// The outer two end exactly at black and white, so those stay pinned.
const PARAMETRIC_REGIONS: [(f32, f32); 4] = [(0.8, 0.2), (0.6, 0.25), (0.4, 0.25), (0.2, 0.2)];

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ToneCurve {
    /// Control points as `[input, output]` for the master, red, green and blue curves.
    /// An empty list is the identity.
    pub curve: [Vec<[f32; 2]>; 4],
    /// Amounts for the regions in [`PARAMETRIC_REGIONS`], added to the master curve.
    pub parametric: [f32; 4],
}

/// A smooth curve through points. Fritsch-Carlson tangents keep it from overshooting.
struct Spline {
    knots: Vec<[f32; 2]>,
    tangents: Vec<f32>,
}

impl Spline {
    /// None when fewer than two distinct inputs leave nothing to interpolate.
    fn new(points: &[[f32; 2]]) -> Option<Self> {
        let mut knots = points.to_vec();
        knots.sort_by(|a, b| a[0].total_cmp(&b[0]));
        knots.dedup_by(|next, kept| next[0] - kept[0] < MIN_SPACING);
        if knots.len() < 2 {
            return None;
        }
        let tangents = tangents_of(&knots);
        Some(Self { knots, tangents })
    }

    fn at(&self, x: f32) -> f32 {
        let last = self.knots.len() - 1;
        if x <= self.knots[0][0] {
            return self.knots[0][1];
        }
        if x >= self.knots[last][0] {
            return self.knots[last][1];
        }
        let segment = self.knots.partition_point(|knot| knot[0] <= x) - 1;
        let [x0, y0] = self.knots[segment];
        let [x1, y1] = self.knots[segment + 1];
        let width = x1 - x0;
        let t = (x - x0) / width;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * y0
            + (t3 - 2.0 * t2 + t) * width * self.tangents[segment]
            + (-2.0 * t3 + 3.0 * t2) * y1
            + (t3 - t2) * width * self.tangents[segment + 1]
    }
}

fn tangents_of(knots: &[[f32; 2]]) -> Vec<f32> {
    let spans = knots.len() - 1;
    let widths: Vec<f32> = knots.windows(2).map(|pair| pair[1][0] - pair[0][0]).collect();
    let slopes: Vec<f32> = knots.windows(2).zip(&widths).map(|(pair, width)| (pair[1][1] - pair[0][1]) / width).collect();
    let mut tangents = vec![slopes[0]; knots.len()];
    tangents[spans] = slopes[spans - 1];
    for i in 1..spans {
        tangents[i] = if slopes[i - 1] * slopes[i] <= 0.0 {
            0.0
        } else {
            (widths[i] * slopes[i - 1] + widths[i - 1] * slopes[i]) / (widths[i - 1] + widths[i])
        };
    }
    if spans > 1 {
        // Ends that curve into their neighbour, unless that would reverse the slope.
        let end = |slope: f32, inner: f32| if slope * (3.0 * slope - inner) <= 0.0 { 0.0 } else { (3.0 * slope - inner) / 2.0 };
        tangents[0] = end(slopes[0], tangents[1]);
        tangents[spans] = end(slopes[spans - 1], tangents[spans - 1]);
    }
    for i in 0..spans {
        if slopes[i] == 0.0 {
            tangents[i] = 0.0;
            tangents[i + 1] = 0.0;
            continue;
        }
        let (a, b) = (tangents[i] / slopes[i], tangents[i + 1] / slopes[i]);
        let size = a * a + b * b;
        if size > 9.0 {
            let scale = 3.0 / size.sqrt();
            tangents[i] = scale * a * slopes[i];
            tangents[i + 1] = scale * b * slopes[i];
        }
    }
    tangents
}

fn is_identity_channel(points: &[[f32; 2]]) -> bool {
    let near = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < IDENTITY_TOLERANCE && (a[1] - b[1]).abs() < IDENTITY_TOLERANCE;
    match (points.first(), points.last()) {
        (Some(&first), Some(&last)) => {
            near(first, [0.0, 0.0]) && near(last, [1.0, 1.0]) && points.iter().all(|p| (p[0] - p[1]).abs() < IDENTITY_TOLERANCE)
        }
        _ => true,
    }
}

impl ToneCurve {
    pub fn is_identity(&self) -> bool {
        self.parametric.iter().all(|&amount| amount == 0.0) && self.curve.iter().all(|points| is_identity_channel(points))
    }

    fn parametric_offset(&self, x: f32) -> f32 {
        self.parametric
            .iter()
            .zip(PARAMETRIC_REGIONS)
            .map(|(&amount, (centre, width))| {
                let distance = (x - centre).abs() / width;
                if distance >= 1.0 { 0.0 } else { amount * PARAMETRIC_REACH * (0.5 + 0.5 * (std::f32::consts::PI * distance).cos()) }
            })
            .sum()
    }

    /// The master curve with the parametric sliders, at `x`.
    fn master_at(&self, master: Option<&Spline>, x: f32) -> f32 {
        (master.map_or(x, |spline| spline.at(x)) + self.parametric_offset(x)).clamp(0.0, 1.0)
    }

    /// One row of RGBA half-float bits, with the master curve applied before each channel's.
    pub fn lut(&self) -> Vec<u16> {
        let [master, channels @ ..] = self.curve.each_ref().map(|points| Spline::new(points));
        let mut out = Vec::with_capacity(CURVE_LUT_SIZE * 4);
        let one = f16::from_f32(1.0).to_bits();
        for i in 0..CURVE_LUT_SIZE {
            let toned = self.master_at(master.as_ref(), i as f32 / (CURVE_LUT_SIZE - 1) as f32);
            for channel in &channels {
                let value = channel.as_ref().map_or(toned, |spline| spline.at(toned));
                out.push(f16::from_f32(value.clamp(0.0, 1.0)).to_bits());
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

    fn master(points: &[[f32; 2]]) -> Vec<f32> {
        let curve = ToneCurve { curve: [points.to_vec(), vec![], vec![], vec![]], parametric: [0.0; 4] };
        let lut = curve.lut();
        (0..CURVE_LUT_SIZE).map(|i| entry(&lut, i, 0)).collect()
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
    fn two_diagonal_points_and_extra_points_on_the_diagonal_change_nothing() {
        for points in [vec![[0.0, 0.0], [1.0, 1.0]], vec![[0.0, 0.0], [0.3, 0.3], [0.8, 0.8], [1.0, 1.0]]] {
            let curve = ToneCurve { curve: [points.clone(), vec![], vec![], vec![]], parametric: [0.0; 4] };
            assert!(curve.is_identity());
            for (i, value) in master(&points).iter().enumerate() {
                assert!((value - i as f32 / 255.0).abs() < 1e-3, "{i}: {value}");
            }
        }
    }

    #[test]
    fn raising_a_point_brightens_midtones_and_pins_the_ends() {
        let values = master(&[[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]]);
        assert!((values[128] - 0.7).abs() < 0.01);
        assert_eq!(values[0], 0.0);
        assert_eq!(values[255], 1.0);
    }

    #[test]
    fn a_channel_curve_affects_only_that_channel() {
        let mut curve = ToneCurve::default();
        curve.curve[1] = vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
        let lut = curve.lut();
        assert!(entry(&lut, 128, 0) > 0.65, "red changes");
        assert!((entry(&lut, 128, 1) - 128.0 / 255.0).abs() < 1e-3, "green does not");
    }

    #[test]
    fn the_curve_passes_through_unevenly_spaced_points() {
        let points = [[0.0, 0.05], [0.12, 0.3], [0.2, 0.33], [0.71, 0.6], [1.0, 0.95]];
        let values = master(&points);
        for [x, y] in points {
            let index = (x * 255.0_f32).round() as usize;
            assert!((values[index] - y).abs() < 0.01, "at {x}: {}", values[index]);
        }
    }

    #[test]
    fn a_rising_curve_never_dips_or_overshoots() {
        let points = [[0.0, 0.0], [0.1, 0.45], [0.2, 0.5], [0.9, 0.55], [1.0, 1.0]];
        let values = master(&points);
        assert!(values.windows(2).all(|pair| pair[1] >= pair[0] - 1e-3), "curve dips");
        let (y1, y2) = (0.5, 0.55);
        assert!(values[(0.5_f32 * 255.0) as usize] >= y1 && values[(0.5_f32 * 255.0) as usize] <= y2, "curve overshoots");
    }

    #[test]
    fn the_curve_has_no_corners_at_its_points() {
        let values = master(&[[0.0, 0.0], [0.25, 0.4], [0.75, 0.7], [1.0, 1.0]]);
        for index in [64, 191] {
            let before = values[index] - values[index - 1];
            let after = values[index + 1] - values[index];
            assert!((before - after).abs() < 0.002, "at {index}: {before} then {after}");
        }
    }

    #[test]
    fn flat_ends_hold_beyond_the_outer_points() {
        let values = master(&[[0.2, 0.1], [0.8, 0.9]]);
        assert!((values[10] - 0.1).abs() < 1e-3);
        assert!((values[250] - 0.9).abs() < 1e-3);
    }

    #[test]
    fn a_descending_curve_inverts_the_image() {
        let values = master(&[[0.0, 1.0], [1.0, 0.0]]);
        assert!((values[64] - (1.0 - 64.0 / 255.0)).abs() < 1e-3);
    }

    #[test]
    fn unsorted_or_repeated_points_are_tolerated() {
        let values = master(&[[1.0, 1.0], [0.5, 0.7], [0.5, 0.2], [0.0, 0.0]]);
        assert!(values.iter().all(|value| value.is_finite()));
        assert!(master(&[[0.5, 0.5]]).iter().enumerate().all(|(i, v)| (v - i as f32 / 255.0).abs() < 1e-3));
    }

    #[derive(serde::Deserialize)]
    struct SharedCase {
        points: Vec<[f32; 2]>,
        parametric: [f32; 4],
        at: Vec<f32>,
        expected: Vec<f32>,
    }

    #[derive(serde::Deserialize)]
    struct SharedCases {
        cases: Vec<SharedCase>,
    }

    #[test]
    fn matches_the_curve_the_interface_draws() {
        let shared: SharedCases = serde_json::from_str(include_str!("curve_cases.json")).unwrap();
        for (number, case) in shared.cases.iter().enumerate() {
            let curve = ToneCurve { curve: [case.points.clone(), vec![], vec![], vec![]], parametric: case.parametric };
            let spline = Spline::new(&case.points);
            for (&x, &expected) in case.at.iter().zip(&case.expected) {
                let value = curve.master_at(spline.as_ref(), x);
                assert!((value - expected).abs() < 1e-4, "case {number} at {x}: {value} against {expected}");
            }
        }
    }

    #[test]
    fn a_parametric_slider_moves_only_its_region_and_keeps_the_ends() {
        let curve = ToneCurve { parametric: [0.0, 0.0, 0.0, 1.0], ..Default::default() };
        assert!(!curve.is_identity());
        let lut = curve.lut();
        assert!(entry(&lut, 51, 0) > 51.0 / 255.0 + 0.08, "shadows rise");
        assert!((entry(&lut, 128, 0) - 128.0 / 255.0).abs() < 1e-3, "midtones stay");
        assert_eq!(entry(&lut, 0, 0), 0.0);
        assert_eq!(entry(&lut, 255, 0), 1.0);
    }
}
