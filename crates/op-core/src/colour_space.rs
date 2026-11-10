//! Colour spaces an export can be delivered in, and the conversion from the sRGB the
//! pipeline renders. The pipeline works with sRGB primaries, so a wider space keeps
//! the same colours, tagged and encoded for that space.

use serde::Deserialize;

use crate::Sample;
use crate::icc::{rgb_icc_profile, srgb_icc_profile};

/// D50, the ICC profile connection space white, as xy.
const D50_XY: [f64; 2] = [0.3457, 0.3585];
const D65_XY: [f64; 2] = [0.3127, 0.3290];
const ADOBE_GAMMA: f64 = 563.0 / 256.0;
const ROMM_GAMMA: f64 = 1.8;
/// ProPhoto (ROMM) is linear below this encoded level, to avoid an infinite slope.
const ROMM_TOE: f64 = 1.0 / 512.0;
/// Steps in the encoding table. It is indexed by the square root of the linear value,
/// which spends its steps where the transfer curves are steep, and interpolated.
const ENCODE_STEPS: usize = 1 << 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputSpace {
    #[default]
    Srgb,
    P3,
    Adobe,
    Prophoto,
}

#[derive(Clone, Copy)]
enum Transfer {
    Srgb,
    Gamma(f64),
    Romm,
}

impl Transfer {
    fn encode(self, linear: f64) -> f64 {
        let v = linear.clamp(0.0, 1.0);
        match self {
            Self::Srgb => if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 },
            Self::Gamma(gamma) => v.powf(1.0 / gamma),
            Self::Romm => if v < ROMM_TOE { v * 16.0 } else { v.powf(1.0 / ROMM_GAMMA) },
        }
    }

    fn decode(self, encoded: f64) -> f64 {
        let v = encoded.clamp(0.0, 1.0);
        match self {
            Self::Srgb => if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) },
            Self::Gamma(gamma) => v.powf(gamma),
            Self::Romm => if v < ROMM_TOE * 16.0 { v / 16.0 } else { v.powf(ROMM_GAMMA) },
        }
    }
}

struct Definition {
    name: &'static str,
    primaries: [[f64; 2]; 3],
    white: [f64; 2],
    transfer: Transfer,
}

impl OutputSpace {
    fn definition(self) -> Definition {
        match self {
            Self::Srgb => Definition { name: "sRGB (OpenPixels)", primaries: [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]], white: D65_XY, transfer: Transfer::Srgb },
            Self::P3 => Definition { name: "Display P3 (OpenPixels)", primaries: [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]], white: D65_XY, transfer: Transfer::Srgb },
            Self::Adobe => {
                Definition { name: "Adobe RGB (1998) compatible (OpenPixels)", primaries: [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]], white: D65_XY, transfer: Transfer::Gamma(ADOBE_GAMMA) }
            }
            Self::Prophoto => {
                Definition { name: "ProPhoto RGB (OpenPixels)", primaries: [[0.7347, 0.2653], [0.1596, 0.8404], [0.0366, 0.0001]], white: D50_XY, transfer: Transfer::Romm }
            }
        }
    }

    /// The columns are the red, green and blue colorants in D50 XYZ, as an ICC profile states them.
    pub fn to_xyz_d50(self) -> [[f64; 3]; 3] {
        let definition = self.definition();
        adapt_to_d50(rgb_to_xyz(definition.primaries, definition.white), definition.white)
    }

    /// The ICC profile to embed in files in this space.
    pub fn icc_profile(self) -> Vec<u8> {
        if self == Self::Srgb {
            // The published colorants, which differ from the computed ones in the fourth place.
            return srgb_icc_profile();
        }
        let definition = self.definition();
        let m = self.to_xyz_d50();
        let colorants = [0, 1, 2].map(|col| [m[0][col], m[1][col], m[2][col]]);
        rgb_icc_profile(definition.name, colorants, |encoded| definition.transfer.decode(encoded))
    }

    /// Re-encodes sRGB-encoded samples in this space, into samples of type `T`.
    pub fn convert_from_srgb<S: Sample, T: Sample>(self, rgb: &[S]) -> Vec<T> {
        let full = |bits: u32| f64::from((1u32 << bits) - 1);
        let (in_max, out_max) = (full(S::BITS.into()), full(T::BITS.into()));
        let decode: Vec<f32> = (0..=in_max as u32).map(|v| Transfer::Srgb.decode(f64::from(v) / in_max) as f32).collect();
        let transfer = self.definition().transfer;
        let last = (ENCODE_STEPS - 1) as f32;
        let encode: Vec<f32> = (0..ENCODE_STEPS)
            .map(|i| {
                let root = i as f64 / f64::from(last);
                (transfer.encode(root * root) * out_max) as f32
            })
            .collect();
        let matrix = self.matrix_from_srgb_linear();
        rgb.chunks_exact(3)
            .flat_map(|pixel| {
                let linear = [0, 1, 2].map(|c| decode[pixel[c].to_u32() as usize]);
                [0, 1, 2].map(|row| {
                    let value = matrix[row][0] * linear[0] + matrix[row][1] * linear[1] + matrix[row][2] * linear[2];
                    let at = value.clamp(0.0, 1.0).sqrt() * last;
                    let index = (at as usize).min(ENCODE_STEPS - 2);
                    let t = at - index as f32;
                    T::from_u32((encode[index] + (encode[index + 1] - encode[index]) * t).round() as u32)
                })
            })
            .collect()
    }

    /// How the develop shader is told to encode into this space.
    pub fn shader_code(self) -> f32 {
        match self {
            Self::Srgb => 0.0,
            Self::P3 => 1.0,
            Self::Adobe => 2.0,
            Self::Prophoto => 3.0,
        }
    }

    pub fn from_shader_code(code: f32) -> Self {
        match code as u32 {
            1 => Self::P3,
            2 => Self::Adobe,
            3 => Self::Prophoto,
            _ => Self::Srgb,
        }
    }

    /// Rows of the matrix from linear sRGB, with the shader's transfer curve in the first
    /// row's fourth place: 0 leaves sRGB as it is, 1 is the sRGB curve, 2 Adobe's gamma, 3 ROMM.
    pub fn shader_rows(self) -> [[f32; 4]; 3] {
        let transfer = match self.definition().transfer {
            _ if self == Self::Srgb => 0.0,
            Transfer::Srgb => 1.0,
            Transfer::Gamma(_) => 2.0,
            Transfer::Romm => 3.0,
        };
        let m = if self == Self::Srgb { [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] } else { self.matrix_from_srgb_linear() };
        std::array::from_fn(|row| [m[row][0], m[row][1], m[row][2], if row == 0 { transfer } else { 0.0 }])
    }

    /// D50 XYZ, as ICC profiles give colours, to linear values in this space.
    pub fn matrix_from_xyz_d50(self) -> [[f64; 3]; 3] {
        invert(self.to_xyz_d50())
    }

    /// Linear sRGB to linear values in this space.
    pub fn matrix_from_srgb_linear(self) -> [[f32; 3]; 3] {
        let to_xyz = OutputSpace::Srgb.to_xyz_d50();
        let from_xyz = invert(self.to_xyz_d50());
        std::array::from_fn(|row| std::array::from_fn(|col| (0..3).map(|k| from_xyz[row][k] * to_xyz[k][col]).sum::<f64>() as f32))
    }
}

fn xy_to_xyz([x, y]: [f64; 2]) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// The matrix from linear RGB to XYZ with this white, scaled so white has Y = 1.
fn rgb_to_xyz(primaries: [[f64; 2]; 3], white: [f64; 2]) -> [[f64; 3]; 3] {
    let columns = primaries.map(xy_to_xyz);
    let base: [[f64; 3]; 3] = std::array::from_fn(|row| std::array::from_fn(|col| columns[col][row]));
    let w = xy_to_xyz(white);
    let inverse = invert(base);
    let scale: [f64; 3] = std::array::from_fn(|row| (0..3).map(|k| inverse[row][k] * w[k]).sum());
    std::array::from_fn(|row| std::array::from_fn(|col| base[row][col] * scale[col]))
}

/// Bradford chromatic adaptation from `white` to D50, applied to an RGB-to-XYZ matrix.
fn adapt_to_d50(matrix: [[f64; 3]; 3], white: [f64; 2]) -> [[f64; 3]; 3] {
    const BRADFORD: [[f64; 3]; 3] = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
    let cone = |xyz: [f64; 3]| -> [f64; 3] { std::array::from_fn(|row| (0..3).map(|k| BRADFORD[row][k] * xyz[k]).sum()) };
    let (source, target) = (cone(xy_to_xyz(white)), cone(xy_to_xyz(D50_XY)));
    let scale: [[f64; 3]; 3] = std::array::from_fn(|row| std::array::from_fn(|col| if row == col { target[row] / source[row] } else { 0.0 }));
    let adapt = multiply(invert(BRADFORD), multiply(scale, BRADFORD));
    multiply(adapt, matrix)
}

fn multiply(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|row| std::array::from_fn(|col| (0..3).map(|k| a[row][k] * b[k][col]).sum()))
}

fn invert(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let [[a, b, c], [d, e, f], [g, h, i]] = m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    [
        [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det],
        [(f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det],
        [(d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IccProfile;

    #[test]
    fn computed_srgb_matches_the_published_d50_colorants() {
        let m = OutputSpace::Srgb.to_xyz_d50();
        let published = [[0.436_074_7, 0.385_064_9, 0.143_080_4], [0.222_504_5, 0.716_878_6, 0.060_616_9], [0.013_932_2, 0.097_104_5, 0.714_173_3]];
        for row in 0..3 {
            for col in 0..3 {
                assert!((m[row][col] - published[row][col]).abs() < 2e-4, "{row},{col}: {}", m[row][col]);
            }
        }
    }

    #[test]
    fn the_srgb_profile_is_unchanged() {
        assert_eq!(OutputSpace::Srgb.icc_profile(), srgb_icc_profile());
    }

    #[test]
    fn every_profile_is_readable_and_white_stays_white() {
        for space in [OutputSpace::Srgb, OutputSpace::P3, OutputSpace::Adobe, OutputSpace::Prophoto] {
            let profile = IccProfile::parse(&space.icc_profile()).unwrap_or_else(|e| panic!("{space:?}: {e}"));
            assert!(profile.name.contains("OpenPixels"));
            let white: Vec<u8> = space.convert_from_srgb::<u8, u8>(&[255, 255, 255]);
            assert_eq!(white, [255, 255, 255], "{space:?}");
            let black: Vec<u16> = space.convert_from_srgb::<u16, u16>(&[0, 0, 0]);
            assert_eq!(black, [0, 0, 0], "{space:?}");
        }
    }

    #[test]
    fn a_wider_space_stores_srgb_red_as_a_less_extreme_value() {
        for space in [OutputSpace::P3, OutputSpace::Adobe, OutputSpace::Prophoto] {
            let red: Vec<u8> = space.convert_from_srgb::<u8, u8>(&[255, 0, 0]);
            assert!(red[0] < 255, "{space:?}: {red:?}");
        }
        // Adobe RGB shares sRGB's red primary, so sRGB red stays a pure red.
        assert_eq!(OutputSpace::Adobe.convert_from_srgb::<u8, u8>(&[255, 0, 0]), [219, 0, 0]);
        let grey: Vec<u8> = OutputSpace::P3.convert_from_srgb::<u8, u8>(&[128, 128, 128]);
        assert_eq!(grey, [128, 128, 128], "P3 shares sRGB's white and curve");
    }

    #[test]
    fn sixteen_bit_greys_keep_their_levels_in_a_space_with_the_same_curve() {
        let ramp: Vec<u16> = (0..=4000u16).flat_map(|v| [v, v, v]).chain((0..=255u16).flat_map(|v| [v * 257; 3])).collect();
        let converted: Vec<u16> = OutputSpace::P3.convert_from_srgb(&ramp);
        for (before, after) in ramp.iter().zip(&converted) {
            assert!(before.abs_diff(*after) <= 1, "{before} became {after}");
        }
    }

    #[test]
    fn converting_and_reading_back_through_the_embedded_profile_returns_the_colour() {
        let srgb = IccProfile::parse(&srgb_icc_profile()).unwrap();
        for space in [OutputSpace::P3, OutputSpace::Adobe, OutputSpace::Prophoto] {
            let target = IccProfile::parse(&space.icc_profile()).unwrap();
            for colour in [[200u16 * 257, 40 * 257, 30 * 257], [30 * 257, 160 * 257, 90 * 257], [120 * 257, 120 * 257, 200 * 257]] {
                let converted: Vec<u16> = space.convert_from_srgb::<u16, u16>(&colour);
                let as_target = target.xyz_of(&converted.iter().map(|&v| f32::from(v) / 65535.0).collect::<Vec<_>>());
                let as_srgb = srgb.xyz_of(&colour.map(|v| f32::from(v) / 65535.0));
                for c in 0..3 {
                    assert!((as_target[c] - as_srgb[c]).abs() < 2e-3, "{space:?} {colour:?}: {as_target:?} vs {as_srgb:?}");
                }
            }
        }
    }
}
