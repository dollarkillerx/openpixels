//! Bayer demosaicing with directional filtering and a posteriori decision, after
//! Menon, Andriani & Calvagno, IEEE Transactions on Image Processing, 2007.
//!
//! Green is estimated along both axes and each pixel keeps the direction whose
//! colour differences vary least, which avoids the zipper edges of bilinear
//! interpolation. Red and blue then follow the green detail.
//!
//! Runs in overlapping tiles so memory use stays independent of the image size.

use serde::{Deserialize, Serialize};

use crate::xtrans::demosaic_xtrans_block;

const RED: u8 = 0;
const GREEN: u8 = 1;
const BLUE: u8 = 2;

/// Pixels of context a tile needs around the area it outputs.
pub(crate) const TILE_MARGIN: usize = 8;

/// Offsets and weights of the causal window that accumulates colour-difference
/// gradients, as (dx, dy, weight) for the horizontal case.
const DECISION_WINDOW: [(isize, isize, f32); 8] = [
    (0, 2, 1.0),
    (-2, 2, 1.0),
    (-1, 1, 1.0),
    (0, 0, 3.0),
    (-2, 0, 3.0),
    (-1, -1, 1.0),
    (0, -2, 1.0),
    (-2, -2, 1.0),
];

/// Channel of each photosite in the repeating 2x2 tile, indexed `[y & 1][x & 1]`.
/// 0 is red, 1 green, 2 blue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BayerPattern(pub [[u8; 2]; 2]);

/// A sensor's colour filter layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cfa {
    Bayer(BayerPattern),
    XTrans([[u8; 6]; 6]),
}

impl BayerPattern {
    #[inline]
    fn channel(self, x: usize, y: usize) -> u8 {
        self.0[y & 1][x & 1]
    }
}

/// A single-channel tile. Reads past an edge reflect around the edge sample, which
/// keeps the Bayer phase, so a mirrored sample is always of the same colour.
struct Plane {
    width: usize,
    height: usize,
    data: Vec<f32>,
}

impl Plane {
    fn zeros(width: usize, height: usize) -> Self {
        Self { width, height, data: vec![0.0; width * height] }
    }

    #[inline]
    fn reflect(index: isize, len: usize) -> usize {
        let last = len as isize - 1;
        let mirrored = if index < 0 {
            -index
        } else if index > last {
            2 * last - index
        } else {
            index
        };
        mirrored.clamp(0, last) as usize
    }

    #[inline(always)]
    fn at(&self, x: isize, y: isize) -> f32 {
        // A negative index wraps to a huge value, so this one test covers both sides.
        let (ux, uy) = (x as usize, y as usize);
        if ux < self.width && uy < self.height {
            return self.data[uy * self.width + ux];
        }
        self.at_edge(x, y)
    }

    #[cold]
    fn at_edge(&self, x: isize, y: isize) -> f32 {
        let x = Self::reflect(x, self.width);
        let y = Self::reflect(y, self.height);
        self.data[y * self.width + x]
    }

    #[inline]
    fn set(&mut self, x: usize, y: usize, value: f32) {
        self.data[y * self.width + x] = value;
    }

    /// Average of the two neighbours along an axis.
    #[inline]
    fn between(&self, x: isize, y: isize, horizontal: bool) -> f32 {
        if horizontal {
            0.5 * (self.at(x - 1, y) + self.at(x + 1, y))
        } else {
            0.5 * (self.at(x, y - 1) + self.at(x, y + 1))
        }
    }
}

/// Demosaics one tile. `origin` is the tile's position in the sensor, for the phase.
fn demosaic_tile(mosaic: &Plane, pattern: BayerPattern, origin: (usize, usize)) -> [Plane; 3] {
    let (width, height) = (mosaic.width, mosaic.height);
    let channel = |x: usize, y: usize| pattern.channel(origin.0 + x, origin.1 + y);

    let mut green_h = Plane::zeros(width, height);
    let mut green_v = Plane::zeros(width, height);
    let mut chroma_h = Plane::zeros(width, height);
    let mut chroma_v = Plane::zeros(width, height);
    for y in 0..height {
        for x in 0..width {
            let (ix, iy) = (x as isize, y as isize);
            let m = mosaic.at(ix, iy);
            if channel(x, y) == GREEN {
                green_h.set(x, y, m);
                green_v.set(x, y, m);
                continue;
            }
            let h = mosaic.between(ix, iy, true)
                + 0.25 * (2.0 * m - mosaic.at(ix - 2, iy) - mosaic.at(ix + 2, iy));
            let v = mosaic.between(ix, iy, false)
                + 0.25 * (2.0 * m - mosaic.at(ix, iy - 2) - mosaic.at(ix, iy + 2));
            green_h.set(x, y, h);
            green_v.set(x, y, v);
            chroma_h.set(x, y, m - h);
            chroma_v.set(x, y, m - v);
        }
    }

    let mut gradient_h = Plane::zeros(width, height);
    let mut gradient_v = Plane::zeros(width, height);
    for y in 0..height as isize {
        for x in 0..width as isize {
            gradient_h.set(x as usize, y as usize, (chroma_h.at(x, y) - chroma_h.at(x + 2, y)).abs());
            gradient_v.set(x as usize, y as usize, (chroma_v.at(x, y) - chroma_v.at(x, y + 2)).abs());
        }
    }

    let mut green = Plane::zeros(width, height);
    let mut use_horizontal = vec![false; width * height];
    for y in 0..height {
        for x in 0..width {
            let (ix, iy) = (x as isize, y as isize);
            let (mut score_h, mut score_v) = (0.0, 0.0);
            for (dx, dy, weight) in DECISION_WINDOW {
                score_h += weight * gradient_h.at(ix + dx, iy + dy);
                score_v += weight * gradient_v.at(ix + dy, iy + dx);
            }
            let horizontal = score_v >= score_h;
            use_horizontal[y * width + x] = horizontal;
            green.set(x, y, if horizontal { green_h.at(ix, iy) } else { green_v.at(ix, iy) });
        }
    }

    let mut red = Plane::zeros(width, height);
    let mut blue = Plane::zeros(width, height);
    for y in 0..height {
        for x in 0..width {
            let value = mosaic.at(x as isize, y as isize);
            match channel(x, y) {
                RED => red.set(x, y, value),
                BLUE => blue.set(x, y, value),
                _ => {}
            }
        }
    }

    // Green sites: the same-row and same-column neighbours are the measured channels.
    let is_red_row = |y: usize| channel(0, y) == RED || channel(1, y) == RED;
    for y in 0..height {
        for x in 0..width {
            if channel(x, y) != GREEN {
                continue;
            }
            let (ix, iy) = (x as isize, y as isize);
            let g = green.at(ix, iy);
            let red_along_row = is_red_row(y);
            let r = g + red.between(ix, iy, red_along_row) - green.between(ix, iy, red_along_row);
            let b = g + blue.between(ix, iy, !red_along_row) - green.between(ix, iy, !red_along_row);
            red.set(x, y, r);
            blue.set(x, y, b);
        }
    }

    // Red and blue sites: fill the opposite channel along the direction green chose.
    for y in 0..height {
        for x in 0..width {
            let (ix, iy) = (x as isize, y as isize);
            let horizontal = use_horizontal[y * width + x];
            match channel(x, y) {
                BLUE => {
                    let b = blue.at(ix, iy);
                    let r = b + red.between(ix, iy, horizontal) - blue.between(ix, iy, horizontal);
                    red.set(x, y, r);
                }
                RED => {
                    let r = red.at(ix, iy);
                    let b = r + blue.between(ix, iy, horizontal) - red.between(ix, iy, horizontal);
                    blue.set(x, y, b);
                }
                _ => {}
            }
        }
    }

    [red, green, blue]
}

/// Demosaics a block whose first sample is at `origin` in the sensor, returning R, G, B.
pub(crate) fn demosaic_block(
    input: &[f32],
    width: usize,
    height: usize,
    cfa: &Cfa,
    origin: (usize, usize),
) -> [Vec<f32>; 3] {
    match cfa {
        Cfa::Bayer(pattern) => {
            let plane = Plane { width, height, data: input.to_vec() };
            demosaic_tile(&plane, *pattern, origin).map(|plane| plane.data)
        }
        Cfa::XTrans(pattern) => demosaic_xtrans_block(input, width, height, pattern, origin),
    }
}

/// Demosaics the `window` (x, y, width, height) of a `width` x `height` mosaic and
/// returns it as interleaved RGB. Values can overshoot 0..=1 near hard edges.
pub fn demosaic(
    mosaic: &[f32],
    width: usize,
    height: usize,
    pattern: BayerPattern,
    window: (usize, usize, usize, usize),
    tile_size: usize,
) -> Vec<f32> {
    assert_eq!(mosaic.len(), width * height);
    let (win_x, win_y, win_w, win_h) = window;
    assert!(win_x + win_w <= width && win_y + win_h <= height, "window out of bounds");
    assert!(tile_size > 0);

    let mut out = vec![0.0f32; win_w * win_h * 3];
    for tile_y in (win_y..win_y + win_h).step_by(tile_size) {
        for tile_x in (win_x..win_x + win_w).step_by(tile_size) {
            let tile_w = tile_size.min(win_x + win_w - tile_x);
            let tile_h = tile_size.min(win_y + win_h - tile_y);

            let x0 = tile_x.saturating_sub(TILE_MARGIN);
            let y0 = tile_y.saturating_sub(TILE_MARGIN);
            let x1 = (tile_x + tile_w + TILE_MARGIN).min(width);
            let y1 = (tile_y + tile_h + TILE_MARGIN).min(height);

            let mut input = Plane::zeros(x1 - x0, y1 - y0);
            for (row, target) in input.data.chunks_exact_mut(x1 - x0).enumerate() {
                let start = (y0 + row) * width + x0;
                target.copy_from_slice(&mosaic[start..start + (x1 - x0)]);
            }
            let [red, green, blue] = demosaic_tile(&input, pattern, (x0, y0));

            for y in 0..tile_h {
                for x in 0..tile_w {
                    let source = (tile_y + y - y0) * input.width + (tile_x + x - x0);
                    let target = ((tile_y + y - win_y) * win_w + (tile_x + x - win_x)) * 3;
                    out[target] = red.data[source];
                    out[target + 1] = green.data[source];
                    out[target + 2] = blue.data[source];
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGGB: BayerPattern = BayerPattern([[RED, GREEN], [GREEN, BLUE]]);
    const BGGR: BayerPattern = BayerPattern([[BLUE, GREEN], [GREEN, RED]]);

    /// Detail in several orientations and colours, the case bilinear handles worst.
    fn scene(x: usize, y: usize) -> [f32; 3] {
        let (fx, fy) = (x as f32, y as f32);
        let luma = 0.5 + 0.35 * ((fx * 0.45).sin() * (fy * 0.07).cos());
        let edge = if (fx * 0.7 + fy * 0.3) as i32 % 48 < 24 { 1.0 } else { 0.55 };
        [luma * edge, luma * edge * 0.8, luma * edge * 0.6]
    }

    fn mosaic_of(width: usize, height: usize, pattern: BayerPattern) -> Vec<f32> {
        let mut out = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                out.push(scene(x, y)[pattern.channel(x, y) as usize]);
            }
        }
        out
    }

    fn bilinear(mosaic: &[f32], width: usize, height: usize, pattern: BayerPattern) -> Vec<f32> {
        let mut out = vec![0.0; width * height * 3];
        for y in 0..height {
            for x in 0..width {
                for channel in 0..3u8 {
                    let (mut sum, mut count) = (0.0, 0.0);
                    for ny in y.saturating_sub(1)..(y + 2).min(height) {
                        for nx in x.saturating_sub(1)..(x + 2).min(width) {
                            if pattern.channel(nx, ny) == channel {
                                let weight = if nx == x || ny == y { 2.0 } else { 1.0 };
                                sum += weight * mosaic[ny * width + nx];
                                count += weight;
                            }
                        }
                    }
                    let exact = pattern.channel(x, y) == channel;
                    out[(y * width + x) * 3 + channel as usize] =
                        if exact { mosaic[y * width + x] } else { sum / count };
                }
            }
        }
        out
    }

    fn psnr(result: &[f32], width: usize, height: usize) -> f32 {
        let border = 8;
        let (mut error, mut samples) = (0.0f64, 0.0f64);
        for y in border..height - border {
            for x in border..width - border {
                let truth = scene(x, y);
                for (channel, expected) in truth.iter().enumerate() {
                    let difference = result[(y * width + x) * 3 + channel] - expected;
                    error += f64::from(difference * difference);
                    samples += 1.0;
                }
            }
        }
        (-10.0 * (error / samples).log10()) as f32
    }

    #[test]
    fn beats_bilinear_on_detailed_content() {
        let (width, height) = (256, 256);
        let mosaic = mosaic_of(width, height, RGGB);
        let ours = demosaic(&mosaic, width, height, RGGB, (0, 0, width, height), 64);
        let baseline = bilinear(&mosaic, width, height, RGGB);
        let (ours_db, baseline_db) = (psnr(&ours, width, height), psnr(&baseline, width, height));
        assert!(ours_db > baseline_db + 3.0, "ours {ours_db:.2} dB, bilinear {baseline_db:.2} dB");
    }

    #[test]
    fn flat_colour_is_reconstructed_exactly() {
        let (width, height) = (32, 32);
        let colour = [0.6, 0.3, 0.15];
        let mosaic: Vec<f32> = (0..height)
            .flat_map(|y| (0..width).map(move |x| colour[RGGB.channel(x, y) as usize]))
            .collect();
        let out = demosaic(&mosaic, width, height, RGGB, (0, 0, width, height), 16);
        for (index, value) in out.iter().enumerate() {
            assert!((value - colour[index % 3]).abs() < 1e-5, "index {index}: {value}");
        }
    }

    #[test]
    fn result_does_not_depend_on_tile_size() {
        let (width, height) = (200, 150);
        let mosaic = mosaic_of(width, height, BGGR);
        let window = (0, 0, width, height);
        let small = demosaic(&mosaic, width, height, BGGR, window, 37);
        let large = demosaic(&mosaic, width, height, BGGR, window, 500);
        assert_eq!(small, large);
    }

    #[test]
    fn window_matches_the_same_area_of_a_full_render() {
        let (width, height) = (120, 90);
        let mosaic = mosaic_of(width, height, RGGB);
        let full = demosaic(&mosaic, width, height, RGGB, (0, 0, width, height), 64);
        let (win_x, win_y, win_w, win_h) = (17, 11, 40, 30);
        let part = demosaic(&mosaic, width, height, RGGB, (win_x, win_y, win_w, win_h), 64);
        for y in 0..win_h {
            for x in 0..win_w * 3 {
                let expected = full[((win_y + y) * width + win_x) * 3 + x];
                assert_eq!(part[y * win_w * 3 + x], expected);
            }
        }
    }
}
