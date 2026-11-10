//! Lens corrections: distortion, vignetting and lateral chromatic aberration.
//!
//! Corrections are radial polynomials in `r²`, where `r` is the distance from the image
//! centre with 1 at the corner. They come from data the camera stored in the file, when
//! it did, and from the manual sliders. Vignetting belongs to the sensor image, so it is
//! applied while developing; the geometric parts are applied after developing, like
//! liquify, so everything else stays aligned to the unwarped image.

use serde::{Deserialize, Serialize};

use crate::Adjustments;
use crate::output::Sample;

/// How far the distortion slider bends the image at the corner, at full travel.
const MANUAL_DISTORTION: f32 = 0.2;
const MANUAL_VIGNETTE: f32 = 0.75;
const MANUAL_CHROMA: f32 = 0.003;
/// How far the perspective sliders tilt the image plane at full travel.
const MANUAL_PERSPECTIVE: f32 = 0.3;

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

/// Corrections for one shot from the Lensfun database, already interpolated to its
/// focal length and aperture. The field names are Lensfun's.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(tag = "model", rename_all = "lowercase")]
pub enum LensfunDistortion {
    Ptlens { a: f32, b: f32, c: f32 },
    Poly3 { k1: f32 },
    Poly5 { k1: f32, k2: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(tag = "model", rename_all = "lowercase")]
pub enum LensfunTca {
    Linear { kr: f32, kb: f32 },
    Poly3 {
        vr: f32,
        vb: f32,
        #[serde(default)]
        cr: f32,
        #[serde(default)]
        cb: f32,
        #[serde(default)]
        br: f32,
        #[serde(default)]
        bb: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct LensfunVignetting {
    pub k1: f32,
    pub k2: f32,
    pub k3: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LensfunCalibration {
    pub distortion: Option<LensfunDistortion>,
    pub tca: Option<LensfunTca>,
    pub vignetting: Option<LensfunVignetting>,
    /// Lensfun's distortion radius at this image's corner. Its radius of 1 is half the
    /// short edge of the sensor the lens was calibrated on.
    pub hugin_scale: f32,
    /// Lensfun's vignetting radius at this image's corner; 1 on the calibration sensor.
    pub corner_scale: f32,
}

impl LensProfile {
    /// Converts Lensfun's models to the polynomials in the half-diagonal radius used here.
    pub fn from_lensfun(calibration: &LensfunCalibration) -> Self {
        let samples = |value: &dyn Fn(f32) -> f32| -> Vec<(f32, f32)> {
            (0..FIT_SAMPLES)
                .map(|i| {
                    let radius = (i as f32 + 0.5) / FIT_SAMPLES as f32;
                    (radius * radius, value(radius))
                })
                .collect()
        };
        let hugin = calibration.hugin_scale;
        let mut profile = LensProfile::default();
        if let Some(model) = calibration.distortion {
            let ratio = move |r: f32| {
                let r = r * hugin;
                match model {
                    LensfunDistortion::Ptlens { a, b, c } => a * r * r * r + b * r * r + c * r + 1.0 - a - b - c,
                    LensfunDistortion::Poly3 { k1 } => k1 * r * r + 1.0 - k1,
                    LensfunDistortion::Poly5 { k1, k2 } => 1.0 + k1 * r * r + k2 * r * r * r * r,
                }
            };
            let centre = ratio(0.0);
            profile.distortion = fit_polynomial(&samples(&|r| ratio(r) / centre - 1.0), 1);
        }
        if let Some(model) = calibration.tca {
            let ([vr, cr, br], [vb, cb, bb]) = match model {
                LensfunTca::Linear { kr, kb } => ([kr, 0.0, 0.0], [kb, 0.0, 0.0]),
                LensfunTca::Poly3 { vr, vb, cr, cb, br, bb } => ([vr, cr, br], [vb, cb, bb]),
            };
            let curve = |v: f32, c: f32, b: f32| samples(&|r| (b * r * hugin + c) * r * hugin + v - 1.0);
            profile.chroma_red = fit_chroma(&curve(vr, cr, br));
            profile.chroma_blue = fit_chroma(&curve(vb, cb, bb));
        }
        if let Some(LensfunVignetting { k1, k2, k3 }) = calibration.vignetting {
            let scale = calibration.corner_scale;
            let gain = move |r: f32| {
                let r2 = (r * scale).powi(2);
                1.0 / (1.0 + r2 * (k1 + r2 * (k2 + r2 * k3)))
            };
            profile.vignette = fit_polynomial(&samples(&|r| gain(r) - 1.0), 1);
        }
        profile
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
    // A zero or broken first term would divide everything into infinities.
    if !reference.iter().all(|k| k.is_finite()) || reference[0].abs() < 1e-6 {
        return None;
    }
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
    if !centre.is_finite() || centre.abs() < 1e-6 {
        return [0.0; 4];
    }
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
    /// Zooms just enough that the corrected frame has no empty edges, or, when
    /// `keep_whole`, just enough that all of the photo is in it.
    pub scale: f32,
    pub keep_whole: bool,
    /// Red constant and r², blue constant and r².
    pub chroma: [f32; 4],
    pub vignette: [f32; 4],
    /// Keystone terms, vertical then horizontal.
    pub perspective: [f32; 2],
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
        let perspective = adjustments.perspective.map(|value| value * MANUAL_PERSPECTIVE);
        let keep_whole = adjustments.perspective_keep != 0.0;
        let mut map = Self { aspect, distortion, scale: 1.0, keep_whole, chroma, vignette, perspective };
        map.scale = map.fitting_scale();
        map
    }

    pub fn is_identity(&self) -> bool {
        self.distortion == [0.0; 4] && self.chroma == [0.0; 4] && self.scale == 1.0 && self.perspective == [0.0; 2]
    }

    /// Takes a centred position through the keystone correction.
    fn keystone(&self, px: f32, py: f32) -> (f32, f32) {
        let depth = 1.0 - self.perspective[0] * py / self.aspect[1] - self.perspective[1] * px / self.aspect[0];
        (px / depth, py / depth)
    }

    fn radial(&self, r2: f32) -> f32 {
        let [a, b, c, d] = self.distortion;
        1.0 + r2 * (a + r2 * (b + r2 * (c + r2 * d)))
    }

    /// The zoom at which the output frame just fits inside the source, or, when
    /// `keep_whole`, the source just fits inside the output frame.
    fn fitting_scale(&self) -> f32 {
        if self.distortion == [0.0; 4] && self.perspective == [0.0; 2] {
            return 1.0;
        }
        let (mut furthest, mut nearest) = (0.0f32, f32::MAX);
        for step in 0..=BOUNDARY_STEPS {
            let position = step as f32 / BOUNDARY_STEPS as f32 * 2.0 - 1.0;
            // All four sides, since a keystone is not symmetric.
            for (x, y) in [(position, 1.0), (position, -1.0), (1.0, position), (-1.0, position)] {
                let (px, py) = self.keystone(x * self.aspect[0], y * self.aspect[1]);
                let stretch = self.radial(px * px + py * py);
                let reach = ((px / self.aspect[0]).abs() * stretch).max((py / self.aspect[1]).abs() * stretch);
                furthest = furthest.max(reach);
                nearest = nearest.min(reach);
            }
        }
        1.0 / if self.keep_whole { nearest } else { furthest }
    }

    /// Where a corrected position reads from, for `channel` (0 red, 1 green, 2 blue).
    pub fn source_uv(&self, u: f32, v: f32, channel: usize) -> (f32, f32) {
        let (px, py) = self.keystone((u - 0.5) * 2.0 * self.aspect[0], (v - 0.5) * 2.0 * self.aspect[1]);
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
    pub fn apply_rgb<S: Sample>(&self, rgb: &[S], width: u32, height: u32) -> Vec<S> {
        if self.is_identity() {
            return rgb.to_vec();
        }
        let (w, h) = (width as usize, height as usize);
        let mut out = vec![S::default(); rgb.len()];
        let fringing = self.chroma != [0.0; 4];
        let sample = |x: f32, y: f32, channel: usize| -> S {
            let (x, y) = (x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32));
            let (x0, y0) = (x as usize, y as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (tx, ty) = (x - x0 as f32, y - y0 as f32);
            let at = |px: usize, py: usize| rgb[(py * w + px) * 3 + channel].to_f32();
            let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
            let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
            S::from_f32(top * (1.0 - ty) + bottom * ty)
        };
        for (row, line) in out.chunks_exact_mut(w * 3).enumerate() {
            let v = (row as f32 + 0.5) / height as f32;
            for (col, target) in line.chunks_exact_mut(3).enumerate() {
                let u = (col as f32 + 0.5) / width as f32;
                let (green_u, green_v) = self.source_uv(u, v, 1);
                for (channel, value) in target.iter_mut().enumerate() {
                    let (su, sv) =
                        if fringing && channel != 1 { self.source_uv(u, v, channel) } else { (green_u, green_v) };
                    if self.keep_whole && !((0.0..=1.0).contains(&su) && (0.0..=1.0).contains(&sv)) {
                        continue;
                    }
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
mod fit_tests {
    use super::*;

    fn map(perspective: [f32; 2], keep: f32) -> LensMap {
        LensMap::new(None, &Adjustments { perspective, perspective_keep: keep, ..Default::default() }, 600, 400)
    }

    fn boundary() -> Vec<(f32, f32)> {
        let edge = (0..=40).map(|i| i as f32 / 40.0);
        edge.flat_map(|t| [(t, 0.0), (t, 1.0), (0.0, t), (1.0, t)]).collect()
    }

    #[test]
    fn the_frame_stays_inside_the_photo_for_either_keystone_direction() {
        for perspective in [[0.6, 0.0], [-0.6, 0.0], [0.0, 0.5], [0.4, -0.4]] {
            let map = map(perspective, 0.0);
            for (u, v) in boundary() {
                let (x, y) = map.source_uv(u, v, 1);
                assert!((-1e-3..=1.001).contains(&x) && (-1e-3..=1.001).contains(&y), "{perspective:?} at {u},{v}: {x},{y}");
            }
        }
    }

    #[test]
    fn keeping_the_whole_photo_puts_every_edge_of_the_frame_outside_it() {
        for perspective in [[0.6, 0.0], [-0.6, 0.0], [0.0, 0.5], [0.4, -0.4]] {
            let map = map(perspective, 1.0);
            for (u, v) in boundary() {
                let (x, y) = map.source_uv(u, v, 1);
                assert!(!(0.001..0.999).contains(&x) || !(0.001..0.999).contains(&y), "{perspective:?} at {u},{v} reads inside: {x},{y}");
            }
        }
    }

    #[test]
    fn what_falls_outside_the_photo_is_black_when_keeping_it_whole() {
        let map = map([0.6, 0.0], 1.0);
        let rgb = vec![200u8; 600 * 400 * 3];
        let out = map.apply_rgb(&rgb, 600, 400);
        assert_eq!(out[(399 * 600 + 2) * 3], 0, "a corner beyond the photo");
        assert!(out[(200 * 600 + 300) * 3] > 190, "the middle is the photo");
    }
}

#[cfg(test)]
mod lensfun_tests {
    use super::*;

    fn polynomial(terms: [f32; 4], r2: f32) -> f32 {
        r2 * (terms[0] + r2 * (terms[1] + r2 * (terms[2] + r2 * terms[3])))
    }

    fn lensfun(distortion: Option<LensfunDistortion>, tca: Option<LensfunTca>, vignetting: Option<LensfunVignetting>) -> LensProfile {
        // A 3:2 full-frame image on a 3:2 full-frame calibration.
        LensProfile::from_lensfun(&LensfunCalibration { distortion, tca, vignetting, hugin_scale: 1.5f32.hypot(1.0), corner_scale: 1.0 })
    }

    #[test]
    fn distortion_is_measured_from_half_the_short_edge() {
        let profile = lensfun(Some(LensfunDistortion::Poly3 { k1: -0.05 }), None, None);
        // Halfway to the corner is 0.9014 of half the short edge.
        let r: f32 = 0.5 * 1.5f32.hypot(1.0);
        let expected = (-0.05 * r * r + 1.05) / 1.05 - 1.0;
        assert!((polynomial(profile.distortion, 0.25) - expected).abs() < 1e-4, "{:?}", profile.distortion);
    }

    #[test]
    fn ptlens_matches_its_formula_near_the_corner() {
        let (a, b, c) = (0.01f32, -0.04, 0.02);
        let profile = lensfun(Some(LensfunDistortion::Ptlens { a, b, c }), None, None);
        let r = 0.98f32.sqrt() * 1.5f32.hypot(1.0);
        let centre = 1.0 - a - b - c;
        let expected = (a * r * r * r + b * r * r + c * r + centre) / centre - 1.0;
        assert!((polynomial(profile.distortion, 0.98) - expected).abs() < 0.002, "{:?}", profile.distortion);
    }

    #[test]
    fn vignetting_brightens_the_corner_by_the_inverse_falloff() {
        let profile = lensfun(None, None, Some(LensfunVignetting { k1: -0.5, k2: 0.1, k3: 0.0 }));
        let gain = 1.0 + polynomial(profile.vignette, 0.96);
        let expected = 1.0 / (1.0 + 0.96 * (-0.5 + 0.96 * 0.1));
        assert!((gain - expected).abs() < 0.01, "{gain} against {expected}");
    }

    #[test]
    fn tca_scales_red_and_blue_around_green() {
        let profile = lensfun(None, Some(LensfunTca::Linear { kr: 1.001, kb: 0.998 }), None);
        assert!((profile.chroma_red[0] - 0.001).abs() < 1e-5);
        assert!((profile.chroma_blue[0] + 0.002).abs() < 1e-5);
    }

    #[test]
    fn a_calibration_with_nothing_in_it_changes_nothing() {
        assert_eq!(lensfun(None, None, None), LensProfile::default());
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn perspective_keeps_the_output_inside_the_source() {
        let adjustments = Adjustments { perspective: [0.8, -0.5], ..Default::default() };
        let map = LensMap::new(None, &adjustments, 300, 200);
        assert!(!map.is_identity());
        for step in 0..=16 {
            let along = step as f32 / 16.0;
            for (u, v) in [(along, 0.0), (along, 1.0), (0.0, along), (1.0, along)] {
                let (x, y) = map.source_uv(u, v, 1);
                assert!((-1e-4..=1.0001).contains(&x) && (-1e-4..=1.0001).contains(&y), "({u}, {v}) -> ({x}, {y})");
            }
        }
    }

    #[test]
    fn vertical_perspective_widens_one_end_of_the_frame() {
        let adjustments = Adjustments { perspective: [1.0, 0.0], ..Default::default() };
        let map = LensMap::new(None, &adjustments, 300, 200);
        let width_at = |v: f32| map.source_uv(1.0, v, 1).0 - map.source_uv(0.0, v, 1).0;
        assert!(width_at(0.0) < width_at(1.0), "{} vs {}", width_at(0.0), width_at(1.0));
    }

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
        assert_eq!(map.apply_rgb(&rgb, 300, 200), rgb);
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
