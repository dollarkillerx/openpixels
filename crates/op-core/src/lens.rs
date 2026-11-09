//! Lens corrections: distortion, vignetting and lateral chromatic aberration.
//!
//! Corrections are radial polynomials in `r²`, where `r` is the distance from the image
//! centre with 1 at the corner. They come from data the camera stored in the file, when
//! it did, and from the manual sliders. Vignetting belongs to the sensor image, so it is
//! applied while developing; the geometric parts are applied after developing, like
//! liquify, so everything else stays aligned to the unwarped image.

use serde::{Deserialize, Serialize};

use crate::Adjustments;

/// How far the distortion slider bends the image at the corner, at full travel.
const MANUAL_DISTORTION: f32 = 0.2;
const MANUAL_VIGNETTE: f32 = 0.75;
const MANUAL_CHROMA: f32 = 0.003;

const BOUNDARY_STEPS: usize = 64;
const FIT_SAMPLES: usize = 64;

const SONY_VIGNETTE_TAG: u16 = 0x7032;
const SONY_CHROMA_TAG: u16 = 0x7035;
const SONY_DISTORTION_TAG: u16 = 0x7037;
const SONY_KNOTS: usize = 16;

const SUB_IFDS_TAG: u16 = 0x014A;
const DNG_OPCODE_LIST_2: u16 = 0xC741;
const DNG_OPCODE_LIST_3: u16 = 0xC74E;
const OPCODE_WARP_RECTILINEAR: u32 = 1;
const OPCODE_FIX_VIGNETTE_RADIAL: u32 = 3;

const MAX_IFD_ENTRIES: usize = 4096;
const MAX_IFDS: usize = 32;

/// What a file says about its lens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LensProfile {
    /// Source radius over output radius, minus 1, as coefficients of r², r⁴, r⁶ and r⁸.
    pub distortion: [f32; 4],
    /// Light gain minus 1, as coefficients of r², r⁴, r⁶ and r⁸.
    pub vignette: [f32; 4],
    /// Scale of the red channel relative to green, minus 1: constant and r² coefficients.
    pub chroma_red: [f32; 2],
    pub chroma_blue: [f32; 2],
}

impl LensProfile {
    /// Finds correction data in a TIFF-based RAW file.
    pub fn from_file(bytes: &[u8]) -> Option<Self> {
        let tiff = Tiff::new(bytes)?;
        let mut tags = FoundTags::default();
        tiff.walk(&mut tags);
        tags.sony().or_else(|| tags.dng())
    }
}

#[derive(Default)]
struct FoundTags<'a> {
    sony_vignette: Option<Vec<i16>>,
    sony_chroma: Option<Vec<i16>>,
    sony_distortion: Option<Vec<i16>>,
    opcodes: Vec<&'a [u8]>,
}

impl FoundTags<'_> {
    fn sony(&self) -> Option<LensProfile> {
        let knots = |values: &Option<Vec<i16>>, extra: usize| -> Option<Vec<f32>> {
            let values = values.as_ref()?;
            (values.first() == Some(&((SONY_KNOTS * extra) as i16)) && values.len() == SONY_KNOTS * extra + 1)
                .then(|| values[1..].iter().map(|&v| f32::from(v)).collect())
        };
        let distortion = knots(&self.sony_distortion, 1)?;
        let vignette = knots(&self.sony_vignette, 1)?;
        let chroma = knots(&self.sony_chroma, 2)?;

        let scaled = |values: &[f32], convert: &dyn Fn(f32) -> f32| -> Vec<(f32, f32)> {
            sample_knots(&values.iter().copied().map(convert).collect::<Vec<_>>())
        };
        let shape = scaled(&distortion, &|v| v * 2f32.powi(-14) + 1.0);
        // Stored as a darkening of 2^(0.5 - 2^(v - 1)); the correction is its reciprocal.
        let gain = scaled(&vignette, &|v| 2f32.powf(2f32.powf(v * 2f32.powi(-13) - 1.0) - 0.5));
        let (red, blue) = chroma.split_at(SONY_KNOTS);
        let chroma_of = |values: &[f32]| fit_chroma(&scaled(values, &|v| v * 2f32.powi(-21)));
        Some(LensProfile {
            distortion: fit_relative(&shape),
            vignette: fit_gain(&gain),
            chroma_red: chroma_of(red),
            chroma_blue: chroma_of(blue),
        })
    }

    /// DNG opcodes, whose coefficients are already radial polynomials.
    fn dng(&self) -> Option<LensProfile> {
        let mut profile = None::<LensProfile>;
        for list in &self.opcodes {
            for (id, parameters) in Opcodes::new(list)? {
                let entry = profile.get_or_insert_with(LensProfile::default);
                match id {
                    OPCODE_WARP_RECTILINEAR => apply_dng_warp(entry, parameters)?,
                    OPCODE_FIX_VIGNETTE_RADIAL => apply_dng_vignette(entry, parameters)?,
                    _ => {}
                }
            }
        }
        profile.filter(|found| *found != LensProfile::default())
    }
}

/// `WarpRectilinear`: one or three planes of (kr0..kr3, kt0, kt1), then the centre.
fn apply_dng_warp(profile: &mut LensProfile, parameters: &[u8]) -> Option<()> {
    let planes = be_u32(parameters, 0)? as usize;
    if planes != 1 && planes != 3 {
        return None;
    }
    let radial = |plane: usize| -> Option<[f32; 4]> {
        let at = |i: usize| be_f64(parameters, 4 + plane * 48 + i * 8);
        Some([at(0)? as f32, at(1)? as f32, at(2)? as f32, at(3)? as f32])
    };
    let reference = radial(if planes == 3 { 1 } else { 0 })?;
    let normal = |k: f32| k / reference[0];
    profile.distortion = [normal(reference[1]), normal(reference[2]), normal(reference[3]), 0.0];
    if planes == 3 {
        let scale_of = |plane: [f32; 4]| {
            let samples: Vec<(f32, f32)> = (0..FIT_SAMPLES)
                .map(|i| {
                    let r2 = i as f32 / (FIT_SAMPLES - 1) as f32;
                    let eval = |k: [f32; 4]| k[0] + r2 * (k[1] + r2 * (k[2] + r2 * k[3]));
                    (r2, eval(plane) / eval(reference) - 1.0)
                })
                .collect();
            fit_chroma(&samples)
        };
        profile.chroma_red = scale_of(radial(0)?);
        profile.chroma_blue = scale_of(radial(2)?);
    }
    Some(())
}

/// `FixVignetteRadial`: gain = 1 + k0 r² + k1 r⁴ + k2 r⁶ + k3 r⁸ + k4 r¹⁰.
fn apply_dng_vignette(profile: &mut LensProfile, parameters: &[u8]) -> Option<()> {
    for (index, slot) in profile.vignette.iter_mut().enumerate() {
        *slot = be_f64(parameters, index * 8)? as f32;
    }
    Some(())
}

/// Knot values, at radii of (i + 0.5) / (knots - 1), as (r², value) samples.
fn sample_knots(values: &[f32]) -> Vec<(f32, f32)> {
    let last = (values.len() - 1) as f32;
    (0..FIT_SAMPLES)
        .map(|i| {
            let radius = i as f32 / (FIT_SAMPLES - 1) as f32;
            let position = (radius * last - 0.5).max(0.0);
            let low = position.floor() as usize;
            let high = (low + 1).min(values.len() - 1);
            let value = values[low] + (values[high] - values[low]) * (position - low as f32);
            (radius * radius, value)
        })
        .collect()
}

/// Least-squares polynomial in `x`, starting at `x^first_power`.
fn fit_polynomial<const N: usize>(samples: &[(f32, f32)], first_power: i32) -> [f32; N] {
    let mut normal = [[0.0f64; N]; N];
    let mut right = [0.0f64; N];
    for &(x, y) in samples {
        let basis: [f64; N] = std::array::from_fn(|i| f64::from(x).powi(first_power + i as i32));
        for i in 0..N {
            right[i] += basis[i] * f64::from(y);
            for j in 0..N {
                normal[i][j] += basis[i] * basis[j];
            }
        }
    }
    solve(normal, right).map_or([0.0; N], |solved| solved.map(|v| v as f32))
}

/// Gaussian elimination with partial pivoting.
#[allow(clippy::needless_range_loop)] // Rows are indexed in pairs.
pub(crate) fn solve<const N: usize>(mut a: [[f64; N]; N], mut b: [f64; N]) -> Option<[f64; N]> {
    for column in 0..N {
        let pivot = (column..N).max_by(|&x, &y| a[x][column].abs().total_cmp(&a[y][column].abs()))?;
        if a[pivot][column].abs() < 1e-12 {
            return None;
        }
        a.swap(column, pivot);
        b.swap(column, pivot);
        for row in column + 1..N {
            let factor = a[row][column] / a[column][column];
            for k in column..N {
                a[row][k] -= factor * a[column][k];
            }
            b[row] -= factor * b[column];
        }
    }
    let mut x = [0.0; N];
    for row in (0..N).rev() {
        let known: f64 = (row + 1..N).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - known) / a[row][row];
    }
    Some(x)
}

/// Fits a shape relative to its value at the centre, since `LensMap` sets the scale.
fn fit_relative(samples: &[(f32, f32)]) -> [f32; 4] {
    let centre = samples[0].1;
    let relative: Vec<(f32, f32)> = samples.iter().map(|&(x, y)| (x, y / centre - 1.0)).collect();
    fit_polynomial(&relative, 1)
}

fn fit_gain(samples: &[(f32, f32)]) -> [f32; 4] {
    let excess: Vec<(f32, f32)> = samples.iter().map(|&(x, y)| (x, y - 1.0)).collect();
    fit_polynomial(&excess, 1)
}

fn fit_chroma(samples: &[(f32, f32)]) -> [f32; 2] {
    fit_polynomial(samples, 0)
}

/// The corrections in effect for an edit, ready to evaluate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensMap {
    /// Half-diagonal-normalised half size: `aspect[0]² + aspect[1]² = 1`.
    pub aspect: [f32; 2],
    pub distortion: [f32; 4],
    /// Zooms just enough that the corrected frame has no empty edges.
    pub scale: f32,
    /// Red constant and r², blue constant and r².
    pub chroma: [f32; 4],
    pub vignette: [f32; 4],
}

impl LensMap {
    pub fn new(profile: Option<&LensProfile>, adjustments: &Adjustments, width: u32, height: u32) -> Self {
        let automatic = profile.filter(|_| adjustments.lens_auto != 0.0).copied().unwrap_or_default();
        let [manual_distortion, manual_vignette, manual_red, manual_blue] = adjustments.lens_manual;

        let mut distortion = automatic.distortion;
        // A positive slider straightens barrel distortion.
        distortion[0] -= manual_distortion * MANUAL_DISTORTION;
        let mut vignette = automatic.vignette;
        vignette[0] += manual_vignette * MANUAL_VIGNETTE;
        let chroma = [
            automatic.chroma_red[0] + manual_red * MANUAL_CHROMA,
            automatic.chroma_red[1],
            automatic.chroma_blue[0] + manual_blue * MANUAL_CHROMA,
            automatic.chroma_blue[1],
        ];

        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let diagonal = w.hypot(h);
        let aspect = [w / diagonal, h / diagonal];
        let mut map = Self { aspect, distortion, scale: 1.0, chroma, vignette };
        map.scale = map.fitting_scale();
        map
    }

    pub fn is_identity(&self) -> bool {
        self.distortion == [0.0; 4] && self.chroma == [0.0; 4] && self.scale == 1.0
    }

    fn radial(&self, r2: f32) -> f32 {
        let [a, b, c, d] = self.distortion;
        1.0 + r2 * (a + r2 * (b + r2 * (c + r2 * d)))
    }

    /// The largest zoom-out that keeps the whole output inside the source frame.
    fn fitting_scale(&self) -> f32 {
        if self.distortion == [0.0; 4] {
            return 1.0;
        }
        let mut furthest = 0.0f32;
        for step in 0..=BOUNDARY_STEPS {
            let position = step as f32 / BOUNDARY_STEPS as f32 * 2.0 - 1.0;
            for (x, y) in [(position, 1.0), (1.0, position)] {
                let (px, py) = (x * self.aspect[0], y * self.aspect[1]);
                let stretch = self.radial(px * px + py * py);
                furthest = furthest.max(x.abs() * stretch).max(y.abs() * stretch);
            }
        }
        1.0 / furthest
    }

    /// Where a corrected position reads from, for `channel` (0 red, 1 green, 2 blue).
    pub fn source_uv(&self, u: f32, v: f32, channel: usize) -> (f32, f32) {
        let (px, py) = ((u - 0.5) * 2.0 * self.aspect[0], (v - 0.5) * 2.0 * self.aspect[1]);
        let r2 = px * px + py * py;
        let fringe = match channel {
            0 => 1.0 + self.chroma[0] + self.chroma[1] * r2,
            2 => 1.0 + self.chroma[2] + self.chroma[3] * r2,
            _ => 1.0,
        };
        let stretch = self.radial(r2) * self.scale * fringe;
        (0.5 + px * stretch / (2.0 * self.aspect[0]), 0.5 + py * stretch / (2.0 * self.aspect[1]))
    }

    /// Resamples a developed image through the geometric corrections.
    pub fn apply_rgb8(&self, rgb: &[u8], width: u32, height: u32) -> Vec<u8> {
        if self.is_identity() {
            return rgb.to_vec();
        }
        let (w, h) = (width as usize, height as usize);
        let mut out = vec![0u8; rgb.len()];
        let fringing = self.chroma != [0.0; 4];
        let sample = |x: f32, y: f32, channel: usize| -> u8 {
            let (x, y) = (x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32));
            let (x0, y0) = (x as usize, y as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (tx, ty) = (x - x0 as f32, y - y0 as f32);
            let at = |px: usize, py: usize| f32::from(rgb[(py * w + px) * 3 + channel]);
            let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
            let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
            (top * (1.0 - ty) + bottom * ty).round() as u8
        };
        for (row, line) in out.chunks_exact_mut(w * 3).enumerate() {
            let v = (row as f32 + 0.5) / height as f32;
            for (col, target) in line.chunks_exact_mut(3).enumerate() {
                let u = (col as f32 + 0.5) / width as f32;
                let (green_u, green_v) = self.source_uv(u, v, 1);
                for (channel, value) in target.iter_mut().enumerate() {
                    let (su, sv) =
                        if fringing && channel != 1 { self.source_uv(u, v, channel) } else { (green_u, green_v) };
                    *value = sample(su * width as f32 - 0.5, sv * height as f32 - 0.5, channel);
                }
            }
        }
        out
    }
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn be_f64(data: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_be_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

/// Iterates the (id, parameters) of a DNG opcode list, which is always big-endian.
struct Opcodes<'a> {
    data: &'a [u8],
    remaining: u32,
    position: usize,
}

impl<'a> Opcodes<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        Some(Self { data, remaining: be_u32(data, 0)?, position: 4 })
    }
}

impl<'a> Iterator for Opcodes<'a> {
    type Item = (u32, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let id = be_u32(self.data, self.position)?;
        let size = be_u32(self.data, self.position + 12)? as usize;
        let start = self.position + 16;
        let parameters = self.data.get(start..start.checked_add(size)?)?;
        self.position = start + size;
        Some((id, parameters))
    }
}

/// Just enough TIFF to find a few private tags.
struct Tiff<'a> {
    data: &'a [u8],
    big_endian: bool,
}

impl<'a> Tiff<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        let big_endian = match data.get(..2)? {
            b"II" => false,
            b"MM" => true,
            _ => return None,
        };
        Some(Self { data, big_endian })
    }

    fn u16(&self, at: usize) -> Option<u16> {
        let bytes: [u8; 2] = self.data.get(at..at + 2)?.try_into().ok()?;
        Some(if self.big_endian { u16::from_be_bytes(bytes) } else { u16::from_le_bytes(bytes) })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let bytes: [u8; 4] = self.data.get(at..at + 4)?.try_into().ok()?;
        Some(if self.big_endian { u32::from_be_bytes(bytes) } else { u32::from_le_bytes(bytes) })
    }

    fn walk(&self, found: &mut FoundTags<'a>) {
        let Some(first) = self.u32(4) else { return };
        let mut queue = vec![first as usize];
        let mut visited = Vec::new();
        while let Some(offset) = queue.pop() {
            if offset == 0 || visited.contains(&offset) || visited.len() >= MAX_IFDS {
                continue;
            }
            visited.push(offset);
            self.read_ifd(offset, found, &mut queue);
        }
    }

    fn read_ifd(&self, offset: usize, found: &mut FoundTags<'a>, queue: &mut Vec<usize>) -> Option<()> {
        let count = usize::from(self.u16(offset)?);
        if count > MAX_IFD_ENTRIES {
            return None;
        }
        for index in 0..count {
            let entry = offset + 2 + index * 12;
            let tag = self.u16(entry)?;
            let kind = self.u16(entry + 2)?;
            let items = self.u32(entry + 4)? as usize;
            let width = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 13 => 4,
                _ => continue,
            };
            let bytes = items.checked_mul(width)?;
            let value = if bytes <= 4 { entry + 8 } else { self.u32(entry + 8)? as usize };
            match tag {
                SUB_IFDS_TAG if kind == 4 || kind == 13 => {
                    for item in 0..items.min(MAX_IFDS) {
                        queue.push(self.u32(value + item * 4)? as usize);
                    }
                }
                SONY_VIGNETTE_TAG | SONY_CHROMA_TAG | SONY_DISTORTION_TAG if kind == 8 => {
                    let values = (0..items).map(|i| self.u16(value + i * 2).map(|v| v as i16)).collect::<Option<Vec<_>>>()?;
                    match tag {
                        SONY_VIGNETTE_TAG => found.sony_vignette = Some(values),
                        SONY_CHROMA_TAG => found.sony_chroma = Some(values),
                        _ => found.sony_distortion = Some(values),
                    }
                }
                DNG_OPCODE_LIST_2 | DNG_OPCODE_LIST_3 if kind == 7 => {
                    found.opcodes.push(self.data.get(value..value.checked_add(bytes)?)?);
                }
                _ => {}
            }
        }
        queue.push(self.u32(offset + 2 + count * 12)? as usize);
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_with(distortion: f32) -> LensMap {
        let adjustments = Adjustments { lens_manual: [distortion, 0.0, 0.0, 0.0], ..Default::default() };
        LensMap::new(None, &adjustments, 300, 200)
    }

    #[test]
    fn no_correction_is_the_identity() {
        let map = map_with(0.0);
        assert!(map.is_identity());
        assert_eq!(map.source_uv(0.25, 0.75, 1), (0.25, 0.75));
        let rgb: Vec<u8> = (0..300 * 200 * 3).map(|i| (i % 251) as u8).collect();
        assert_eq!(map.apply_rgb8(&rgb, 300, 200), rgb);
    }

    #[test]
    fn the_centre_stays_put_and_the_output_stays_inside_the_source() {
        for amount in [-1.0, -0.3, 0.4, 1.0] {
            let map = map_with(amount);
            let (cu, cv) = map.source_uv(0.5, 0.5, 1);
            assert!((cu - 0.5).abs() < 1e-5 && (cv - 0.5).abs() < 1e-5);
            for (u, v) in [(0.0, 0.0), (1.0, 1.0), (0.0, 0.5), (0.5, 1.0), (1.0, 0.2)] {
                let (su, sv) = map.source_uv(u, v, 1);
                assert!((-1e-4..=1.0001).contains(&su) && (-1e-4..=1.0001).contains(&sv), "{amount}: {su},{sv}");
            }
        }
    }

    #[test]
    fn the_automatic_profile_only_applies_when_enabled() {
        let profile = LensProfile { distortion: [0.04, 0.0, 0.0, 0.0], ..Default::default() };
        let off = LensMap::new(Some(&profile), &Adjustments::default(), 300, 200);
        assert!(off.is_identity());
        let on = LensMap::new(Some(&profile), &Adjustments { lens_auto: 1.0, ..Default::default() }, 300, 200);
        assert_eq!(on.distortion[0], 0.04);
    }

    #[test]
    fn red_and_blue_read_from_their_own_radius() {
        let adjustments = Adjustments { lens_manual: [0.0, 0.0, 1.0, -1.0], ..Default::default() };
        let map = LensMap::new(None, &adjustments, 300, 200);
        let centre_distance = |(u, _): (f32, f32)| (u - 0.5).abs();
        let [red, green, blue] = [0, 1, 2].map(|channel| centre_distance(map.source_uv(0.9, 0.5, channel)));
        assert!(red > green && green > blue, "{red} {green} {blue}");
    }

    fn le_tiff(tags: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
        let mut out = b"II*\0\x08\0\0\0".to_vec();
        out.extend((tags.len() as u16).to_le_bytes());
        let mut data_at = 8 + 2 + tags.len() * 12 + 4;
        let mut data = Vec::<u8>::new();
        for (tag, kind, bytes) in tags {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            let width = if *kind == 8 { 2 } else { 1 };
            out.extend(((bytes.len() / width) as u32).to_le_bytes());
            out.extend((data_at as u32).to_le_bytes());
            data.extend(bytes);
            data_at += bytes.len();
        }
        out.extend(0u32.to_le_bytes());
        out.extend(data);
        out
    }

    fn sony_array(first: i16, values: impl Iterator<Item = i16>) -> Vec<u8> {
        std::iter::once(first).chain(values).flat_map(i16::to_le_bytes).collect()
    }

    #[test]
    fn sony_curves_become_polynomials_that_follow_them() {
        let distortion: Vec<i16> = (0..16).map(|i| -(i * i * 4)).collect();
        let file = le_tiff(&[
            (SONY_VIGNETTE_TAG, 8, sony_array(16, (0..16).map(|i| i * i * 40))),
            (SONY_CHROMA_TAG, 8, sony_array(32, (0..16).map(|_| 1000).chain((0..16).map(|_| -500)))),
            (SONY_DISTORTION_TAG, 8, sony_array(16, distortion.iter().copied())),
        ]);
        let profile = LensProfile::from_file(&file).unwrap();

        let r2_sum = |k: [f32; 4]| k.iter().sum::<f32>();
        // The corner falls midway between the last two knots.
        let last_two = f32::from(distortion[14]) * 0.5 + f32::from(distortion[15]) * 0.5;
        let expected_corner = last_two * 2f32.powi(-14);
        assert!((r2_sum(profile.distortion) - expected_corner).abs() < 2e-3, "{:?}", profile.distortion);
        let corner_gain = 2f32.powf(2f32.powf(8420.0 * 2f32.powi(-13) - 1.0) - 0.5);
        assert!((r2_sum(profile.vignette) + 1.0 - corner_gain).abs() < 0.05, "{:?}", profile.vignette);
        assert!((profile.chroma_red[0] - 1000.0 * 2f32.powi(-21)).abs() < 1e-5);
        assert!((profile.chroma_blue[0] + 500.0 * 2f32.powi(-21)).abs() < 1e-5);
    }

    #[test]
    fn dng_opcodes_are_read() {
        let mut warp = 1u32.to_be_bytes().to_vec();
        for k in [1.0f64, -0.05, 0.01, 0.0, 0.0, 0.0, 0.5, 0.5] {
            warp.extend(k.to_be_bytes());
        }
        let mut vignette = Vec::<u8>::new();
        for k in [0.5f64, 0.25, 0.0, 0.0, 0.0, 0.5, 0.5] {
            vignette.extend(k.to_be_bytes());
        }
        let mut list = 2u32.to_be_bytes().to_vec();
        for (id, parameters) in [(OPCODE_WARP_RECTILINEAR, &warp), (OPCODE_FIX_VIGNETTE_RADIAL, &vignette)] {
            list.extend(id.to_be_bytes());
            list.extend([0u8; 8]);
            list.extend((parameters.len() as u32).to_be_bytes());
            list.extend(parameters);
        }
        let profile = LensProfile::from_file(&le_tiff(&[(DNG_OPCODE_LIST_3, 7, list)])).unwrap();
        assert!((profile.distortion[0] + 0.05).abs() < 1e-6);
        assert!((profile.distortion[1] - 0.01).abs() < 1e-6);
        assert_eq!(profile.vignette[..2], [0.5, 0.25]);
    }

    #[test]
    fn files_without_lens_data_and_garbage_are_harmless() {
        assert_eq!(LensProfile::from_file(b"not a tiff"), None);
        assert_eq!(LensProfile::from_file(&le_tiff(&[])), None);
        assert_eq!(LensProfile::from_file(b"II*\0\xff\xff\xff\xff"), None);
    }
}
