//! Brings images made in other colour spaces into the pipeline's linear sRGB primaries.
//! Colours outside sRGB, such as the deep reds and greens a Display P3 or Adobe RGB file
//! can hold, become negative or over-one values rather than being clipped, so a wide
//! export can give them back.

use half::f16;

use crate::colour_space::OutputSpace;
use crate::proof::{ColourSpace, IccProfile};

/// Points per side of the grid that table-based profiles are sampled on.
const GRID: usize = 33;
/// Profiles this close to sRGB are treated as sRGB, which is faster and exact.
const SRGB_MATRIX_TOLERANCE: f32 = 0.01;
const SRGB_CURVE_TOLERANCE: f32 = 0.003;

enum Model {
    /// Per-channel linearisation at every code, then one matrix.
    Matrix { tables: [Vec<f32>; 3], matrix: [[f32; 3]; 3] },
    /// Linear sRGB at grid points over the device cube, interpolated.
    Grid(Vec<[f32; 3]>),
}

/// Converts encoded RGB in one colour space to linear sRGB half-floats.
pub struct InputProfile {
    profile: IccProfile,
    to_srgb: [[f32; 3]; 3],
}

fn srgb_decode(encoded: f32) -> f32 {
    if encoded <= 0.040_45 { encoded / 12.92 } else { ((encoded + 0.055) / 1.055).powf(2.4) }
}

fn product(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    std::array::from_fn(|row| std::array::from_fn(|col| (0..3).map(|k| a[row][k] * b[k][col]).sum()))
}

fn apply(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|row| m[row][0] * v[0] + m[row][1] * v[1] + m[row][2] * v[2])
}

impl InputProfile {
    /// A conversion for an embedded profile, or `None` when there is nothing to do: the
    /// profile is sRGB, is not an RGB profile, or cannot be read. Such images are taken as sRGB.
    pub fn from_icc(bytes: &[u8]) -> Option<Self> {
        let profile = IccProfile::parse(bytes).ok().filter(|profile| profile.space == ColourSpace::Rgb && !profile.is_output)?;
        let from_xyz = OutputSpace::Srgb.matrix_from_xyz_d50();
        let to_srgb = std::array::from_fn(|row| std::array::from_fn(|col| from_xyz[row][col] as f32));
        let input = Self { profile, to_srgb };
        (!input.is_srgb()).then_some(input)
    }

    /// Display P3, the space of most phone photos.
    pub fn display_p3() -> Self {
        Self::from_icc(&OutputSpace::P3.icc_profile()).expect("our own profile is valid and wider than sRGB")
    }

    fn is_srgb(&self) -> bool {
        const LEVELS: usize = 256;
        let Some((tables, to_xyz)) = self.profile.matrix_shaper(LEVELS) else { return false };
        let matrix = product(&self.to_srgb, &to_xyz);
        let identity = (0..3).all(|row| (0..3).all(|col| (matrix[row][col] - f32::from(u8::from(row == col))).abs() < SRGB_MATRIX_TOLERANCE));
        identity
            && tables.iter().all(|table| table.iter().enumerate().all(|(code, &linear)| (linear - srgb_decode(code as f32 / (LEVELS - 1) as f32)).abs() < SRGB_CURVE_TOLERANCE))
    }

    fn model(&self, levels: usize) -> Model {
        if let Some((tables, to_xyz)) = self.profile.matrix_shaper(levels) {
            return Model::Matrix { tables, matrix: product(&self.to_srgb, &to_xyz) };
        }
        let last = (GRID - 1) as f32;
        let mut grid = Vec::with_capacity(GRID * GRID * GRID);
        for r in 0..GRID {
            for g in 0..GRID {
                for b in 0..GRID {
                    let xyz = self.profile.xyz_of(&[r as f32 / last, g as f32 / last, b as f32 / last]);
                    grid.push(apply(&self.to_srgb, xyz));
                }
            }
        }
        Model::Grid(grid)
    }

    /// Interleaved RGB codes with `levels` steps each, to linear sRGB half-float bits.
    fn convert(&self, rgb: impl ExactSizeIterator<Item = usize>, levels: usize) -> Vec<u16> {
        let model = self.model(levels);
        let mut out = Vec::with_capacity(rgb.len());
        let mut pixel = [0usize; 3];
        for (index, code) in rgb.enumerate() {
            pixel[index % 3] = code;
            if index % 3 != 2 {
                continue;
            }
            let linear = match &model {
                Model::Matrix { tables, matrix } => apply(matrix, [tables[0][pixel[0]], tables[1][pixel[1]], tables[2][pixel[2]]]),
                Model::Grid(grid) => trilinear(grid, pixel.map(|code| code as f32 / (levels - 1) as f32)),
            };
            out.extend(linear.map(|value| f16::from_f32(value).to_bits()));
        }
        out
    }

    pub fn linearise_u8(&self, rgb: &[u8]) -> Vec<u16> {
        self.convert(rgb.iter().map(|&v| usize::from(v)), 256)
    }

    pub fn linearise_u16(&self, rgb: &[u16]) -> Vec<u16> {
        self.convert(rgb.iter().map(|&v| usize::from(v)), 65536)
    }
}

fn trilinear(grid: &[[f32; 3]], at: [f32; 3]) -> [f32; 3] {
    let last = (GRID - 1) as f32;
    let scaled = at.map(|v| v.clamp(0.0, 1.0) * last);
    let base = scaled.map(|v| (v as usize).min(GRID - 2));
    let t: [f32; 3] = std::array::from_fn(|i| scaled[i] - base[i] as f32);
    let mut out = [0.0; 3];
    for corner in 0..8 {
        let step = [corner >> 2 & 1, corner >> 1 & 1, corner & 1];
        let weight: f32 = (0..3).map(|i| if step[i] == 1 { t[i] } else { 1.0 - t[i] }).product();
        let value = grid[((base[0] + step[0]) * GRID + base[1] + step[1]) * GRID + base[2] + step[2]];
        for c in 0..3 {
            out[c] += weight * value[c];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::srgb_icc_profile;

    fn linear(bits: u16) -> f32 {
        f16::from_bits(bits).to_f32()
    }

    #[test]
    fn srgb_profiles_need_no_conversion() {
        assert!(InputProfile::from_icc(&srgb_icc_profile()).is_none());
        assert!(InputProfile::from_icc(b"not a profile").is_none());
    }

    #[test]
    fn p3_red_lies_outside_srgb_and_white_stays_white() {
        let p3 = InputProfile::display_p3();
        let out = p3.linearise_u8(&[255, 0, 0, 255, 255, 255, 128, 128, 128]);
        let [r, g, b] = [linear(out[0]), linear(out[1]), linear(out[2])];
        // Display P3 red is about (1.22, -0.04, -0.02) in linear sRGB.
        assert!(r > 1.15 && g < -0.02 && b < -0.01, "{r} {g} {b}");
        for c in 3..6 {
            assert!((linear(out[c]) - 1.0).abs() < 2e-3, "white {}", linear(out[c]));
        }
        assert!((linear(out[6]) - srgb_decode(128.0 / 255.0)).abs() < 2e-3);
    }

    #[test]
    fn sixteen_bit_codes_use_the_whole_range() {
        let p3 = InputProfile::display_p3();
        let out = p3.linearise_u16(&[65535, 65535, 65535, 0, 0, 0]);
        assert!((linear(out[1]) - 1.0).abs() < 2e-3);
        assert_eq!(linear(out[4]), 0.0);
    }

    #[test]
    fn adobe_rgb_green_is_wider_than_srgb_green() {
        let adobe = InputProfile::from_icc(&OutputSpace::Adobe.icc_profile()).unwrap();
        let out = adobe.linearise_u8(&[0, 255, 0]);
        assert!(linear(out[0]) < -0.2, "{}", linear(out[0]));
        assert!(linear(out[1]) > 0.95);
    }
}
