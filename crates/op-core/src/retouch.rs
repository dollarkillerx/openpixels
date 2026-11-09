//! Classical skin retouching, computed once per image at preview resolution.
//!
//! Skin is split by spatial frequency in log space. Mid frequencies carry uneven tone
//! and blotches, while the finest ones carry pores, so smoothing removes only the band
//! between two edge-preserving filters. Blemishes are small spots darker than their
//! surroundings. Both results are stored as log2 corrections that the GPU samples by
//! position, which makes them valid at any export resolution.

use crate::LinearImage;
use crate::image_buf::box_filter;
use half::f16;

/// Filter radii as fractions of the face width.
const SMALL_RADIUS: f32 = 0.008;
const LARGE_RADIUS: f32 = 0.04;
const FEATHER_RADIUS: f32 = 0.012;

/// Guided filter regularisation in squared stops: differences well above the square
/// root of these count as real edges and are preserved.
const SMALL_EPSILON: f32 = 0.01;
const LARGE_EPSILON: f32 = 0.04;

/// How many stops darker than its surroundings a spot must be to count as a blemish.
const SPOT_START: f32 = 0.06;
const SPOT_FULL: f32 = 0.2;
/// How far from a pixel to look for brighter skin, as a fraction of the face width.
const SPOT_PROBE_RADIUS: f32 = 0.03;
const SPOT_DIRECTIONS: [(isize, isize); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];
const SPOT_COVERAGE_GAIN: f32 = 3.0;

const LOG_FLOOR: f32 = 1e-4;

/// Corrections in log2 per channel, as RGBA half-float bits; alpha is unused.
pub struct SkinRetouch {
    pub width: u32,
    pub height: u32,
    pub smooth: Vec<u16>,
    pub blemish: Vec<u16>,
}

fn smoothstep(start: f32, end: f32, value: f32) -> f32 {
    let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Edge-preserving smoothing: a guided filter using the image as its own guide.
fn guided_self(plane: &[f32], width: usize, height: usize, radius: usize, epsilon: f32) -> Vec<f32> {
    let mean = box_filter(plane, width, height, radius);
    let squares: Vec<f32> = plane.iter().map(|v| v * v).collect();
    let mean_of_squares = box_filter(&squares, width, height, radius);

    let mut slope = vec![0.0f32; plane.len()];
    let mut offset = vec![0.0f32; plane.len()];
    for i in 0..plane.len() {
        let variance = (mean_of_squares[i] - mean[i] * mean[i]).max(0.0);
        slope[i] = variance / (variance + epsilon);
        offset[i] = (1.0 - slope[i]) * mean[i];
    }
    let mean_slope = box_filter(&slope, width, height, radius);
    let mean_offset = box_filter(&offset, width, height, radius);
    plane.iter().zip(mean_slope.iter().zip(&mean_offset)).map(|(v, (a, b))| a * v + b).collect()
}

fn median(values: &mut [f32]) -> f32 {
    let middle = values.len() / 2;
    *values.select_nth_unstable_by(middle, f32::total_cmp).1
}

/// Builds the corrections for `image`. `mask` holds `width * height` bytes marking
/// facial skin, and `face_width` is the typical face width in pixels.
pub fn build_skin_retouch(image: &LinearImage, mask: &[u8], face_width: f32) -> SkinRetouch {
    let (width, height) = (image.width as usize, image.height as usize);
    assert_eq!(mask.len(), width * height);
    let mut smooth = vec![0u16; width * height * 4];
    let mut blemish = vec![0u16; width * height * 4];
    let empty = |smooth, blemish| SkinRetouch { width: image.width, height: image.height, smooth, blemish };

    let radius_of = |fraction: f32| ((face_width * fraction).round() as usize).max(1);
    let (small_radius, large_radius) = (radius_of(SMALL_RADIUS), radius_of(LARGE_RADIUS).max(3));

    let hard: Vec<f32> = mask.iter().map(|&value| f32::from(value) / 255.0).collect();
    let feather = radius_of(FEATHER_RADIUS);
    let feathered = box_filter(&box_filter(&hard, width, height, feather), width, height, feather);

    // Work only around the faces, with enough room for the largest filter.
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
    for y in 0..height {
        for x in 0..width {
            if feathered[y * width + x] > 1.0 / 255.0 {
                (x0, y0) = (x0.min(x), y0.min(y));
                (x1, y1) = (x1.max(x + 1), y1.max(y + 1));
            }
        }
    }
    if x0 >= x1 || y0 >= y1 {
        return empty(smooth, blemish);
    }
    let margin = large_radius * 2;
    let (x0, y0) = (x0.saturating_sub(margin), y0.saturating_sub(margin));
    let (x1, y1) = ((x1 + margin).min(width), (y1 + margin).min(height));
    let (crop_w, crop_h) = (x1 - x0, y1 - y0);

    let mut planes = [vec![0.0f32; crop_w * crop_h], vec![0.0; crop_w * crop_h], vec![0.0; crop_w * crop_h]];
    let mut crop_mask = vec![0.0f32; crop_w * crop_h];
    for row in 0..crop_h {
        for col in 0..crop_w {
            let source = (y0 + row) * width + x0 + col;
            let target = row * crop_w + col;
            crop_mask[target] = feathered[source];
            for (channel, plane) in planes.iter_mut().enumerate() {
                let value = f16::from_bits(image.pixels[source * 3 + channel]).to_f32();
                plane[target] = value.max(LOG_FLOOR).log2();
            }
        }
    }

    // The face polygon also covers hair and shadows; keep only what looks like this
    // face's own skin.
    let (mut greens, mut red_diffs, mut blue_diffs) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..crop_mask.len() {
        if crop_mask[i] > 0.5 {
            greens.push(planes[1][i]);
            red_diffs.push(planes[0][i] - planes[1][i]);
            blue_diffs.push(planes[2][i] - planes[1][i]);
        }
    }
    if greens.is_empty() {
        return empty(smooth, blemish);
    }
    let (skin_green, skin_red, skin_blue) =
        (median(&mut greens), median(&mut red_diffs), median(&mut blue_diffs));

    let filter = |source: &[Vec<f32>; 3], radius, epsilon| {
        [0, 1, 2].map(|channel: usize| guided_self(&source[channel], crop_w, crop_h, radius, epsilon))
    };
    let looks_like_skin = |r: f32, g: f32, b: f32| {
        smoothstep(-1.4, -0.8, g - skin_green)
            * (1.0 - smoothstep(0.35, 0.8, ((r - g) - skin_red).abs()))
            * (1.0 - smoothstep(0.35, 0.8, ((b - g) - skin_blue).abs()))
    };

    // First find blemishes and work out what each should be replaced with.
    let fine = filter(&planes, small_radius, SMALL_EPSILON);
    let probe = radius_of(SPOT_PROBE_RADIUS).max(small_radius + 2) as isize;
    let mut spots = vec![0.0f32; crop_w * crop_h];
    let mut replacements = [vec![0.0f32; crop_w * crop_h], vec![0.0; crop_w * crop_h], vec![0.0; crop_w * crop_h]];
    let sample = |plane: &[f32], x: isize, y: isize| {
        let x = x.clamp(0, crop_w as isize - 1) as usize;
        let y = y.clamp(0, crop_h as isize - 1) as usize;
        plane[y * crop_w + x]
    };
    let luma_at = |x: isize, y: isize| {
        0.25 * sample(&fine[0], x, y) + 0.6 * sample(&fine[1], x, y) + 0.15 * sample(&fine[2], x, y)
    };
    for row in 0..crop_h {
        for col in 0..crop_w {
            let i = row * crop_w + col;
            if crop_mask[i] <= 0.0 {
                continue;
            }
            // A blemish is darker than its surroundings in every direction. A shadow
            // edge or a crease is only darker across one axis, so the minimum rejects it.
            let (cx, cy) = (col as isize, row as isize);
            let centre = luma_at(cx, cy);
            let mut contrast = f32::MAX;
            let mut surround = [0.0f32; 3];
            for (dx, dy) in SPOT_DIRECTIONS {
                for sign in [1, -1] {
                    let (sx, sy) = (cx + sign * dx * probe, cy + sign * dy * probe);
                    contrast = contrast.min(luma_at(sx, sy) - centre);
                    for (channel, total) in surround.iter_mut().enumerate() {
                        *total += sample(&fine[channel], sx, sy) / 8.0;
                    }
                }
            }
            // Judged by the surroundings: a dark mole on a cheek is not skin-coloured
            // itself, but what surrounds it is, while hair is surrounded by more hair.
            spots[i] = smoothstep(SPOT_START, SPOT_FULL, contrast)
                * crop_mask[i]
                * looks_like_skin(surround[0], surround[1], surround[2]);
            for channel in 0..3 {
                replacements[channel][i] = surround[channel];
            }
        }
    }

    // A spot's rim falls below the detection threshold, so spread the result over it.
    let reach = small_radius.max(1);
    let coverage: Vec<f32> = box_filter(&spots, crop_w, crop_h, reach)
        .iter()
        .map(|value| (value * SPOT_COVERAGE_GAIN).min(1.0))
        .collect();

    // Smoothing works on the healed image, or it would ring around every blemish.
    let mut healed = planes.clone();
    for i in 0..crop_mask.len() {
        if coverage[i] > 0.0 {
            for channel in 0..3 {
                healed[channel][i] += (replacements[channel][i] - planes[channel][i]) * coverage[i];
            }
        }
    }
    let small = filter(&healed, small_radius, SMALL_EPSILON);
    let large = filter(&healed, large_radius, LARGE_EPSILON);

    for row in 0..crop_h {
        for col in 0..crop_w {
            let i = row * crop_w + col;
            if crop_mask[i] <= 0.0 {
                continue;
            }
            let weight = crop_mask[i] * looks_like_skin(healed[0][i], healed[1][i], healed[2][i]);
            let target = ((y0 + row) * width + x0 + col) * 4;
            for channel in 0..3 {
                smooth[target + channel] = f16::from_f32((large[channel][i] - small[channel][i]) * weight).to_bits();
                blemish[target + channel] = f16::from_f32(healed[channel][i] - planes[channel][i]).to_bits();
            }
        }
    }
    empty(smooth, blemish)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: usize = 200;
    const FACE: f32 = 200.0;
    const SKIN: [f32; 3] = [0.4, 0.28, 0.22];

    fn image_with(paint: impl Fn(usize, usize) -> [f32; 3]) -> LinearImage {
        let mut pixels = Vec::with_capacity(SIZE * SIZE * 3);
        for y in 0..SIZE {
            for x in 0..SIZE {
                pixels.extend(paint(x, y).map(|v| f16::from_f32(v).to_bits()));
            }
        }
        LinearImage::new(SIZE as u32, SIZE as u32, pixels)
    }

    fn correction(plane: &[u16], x: usize, y: usize) -> f32 {
        f16::from_bits(plane[(y * SIZE + x) * 4 + 1]).to_f32()
    }

    fn full_mask() -> Vec<u8> {
        vec![255; SIZE * SIZE]
    }

    fn spotted(x: usize, y: usize) -> [f32; 3] {
        let (dx, dy) = (x as f32 - 100.0, y as f32 - 100.0);
        if dx * dx + dy * dy <= 9.0 { SKIN.map(|v| v * 0.5) } else { SKIN }
    }

    #[test]
    fn a_dark_spot_is_lifted_to_its_surroundings() {
        let image = image_with(spotted);
        let retouch = build_skin_retouch(&image, &full_mask(), FACE);
        // The spot is one stop dark, so a full correction is about +1.
        let centre = correction(&retouch.blemish, 100, 100);
        assert!(centre > 0.6 && centre < 1.2, "centre correction {centre}");
    }

    #[test]
    fn a_dark_off_colour_mole_is_still_removed() {
        let mole = |x: usize, y: usize| {
            let (dx, dy) = (x as f32 - 100.0, y as f32 - 100.0);
            if dx * dx + dy * dy <= 9.0 { [0.08, 0.02, 0.012] } else { SKIN }
        };
        let retouch = build_skin_retouch(&image_with(mole), &full_mask(), FACE);
        // Green is 0.02 against 0.28 around it, about 3.8 stops.
        let centre = correction(&retouch.blemish, 100, 100);
        assert!(centre > 3.0 && centre < 4.5, "centre correction {centre}");
    }

    #[test]
    fn smoothing_does_not_ring_around_a_spot() {
        let retouch = build_skin_retouch(&image_with(spotted), &full_mask(), FACE);
        for x in 104..112 {
            let value = correction(&retouch.smooth, x, 100);
            assert!(value.abs() < 0.03, "x = {x}: {value}");
        }
    }

    #[test]
    fn flat_skin_is_left_alone() {
        let image = image_with(spotted);
        let retouch = build_skin_retouch(&image, &full_mask(), FACE);
        assert!(correction(&retouch.blemish, 40, 40).abs() < 0.01);
        assert!(correction(&retouch.smooth, 40, 40).abs() < 0.01);
    }

    #[test]
    fn nothing_changes_outside_the_mask() {
        let image = image_with(spotted);
        let mut mask = vec![0u8; SIZE * SIZE];
        for y in 20..60 {
            for x in 20..60 {
                mask[y * SIZE + x] = 255;
            }
        }
        let retouch = build_skin_retouch(&image, &mask, FACE);
        assert_eq!(correction(&retouch.blemish, 100, 100), 0.0);
    }

    #[test]
    fn dark_hair_inside_the_mask_is_not_treated_as_skin() {
        let hair = |x: usize, y: usize| {
            if x < 40 { [0.012, 0.01, 0.009] } else { spotted(x, y) }
        };
        let retouch = build_skin_retouch(&image_with(hair), &full_mask(), FACE);
        assert_eq!(correction(&retouch.blemish, 15, 100), 0.0);
        assert_eq!(correction(&retouch.smooth, 15, 100), 0.0);
    }

    #[test]
    fn a_gradual_shadow_is_not_a_blemish() {
        let shaded = |x: usize, _y: usize| SKIN.map(|v| v * (0.6 + 0.4 * x as f32 / SIZE as f32));
        let retouch = build_skin_retouch(&image_with(shaded), &full_mask(), FACE);
        assert!(correction(&retouch.blemish, 100, 100).abs() < 0.02);
    }

    #[test]
    fn a_shadow_edge_is_not_a_blemish() {
        let edged = |x: usize, _y: usize| if x < 100 { SKIN.map(|v| v * 0.6) } else { SKIN };
        let retouch = build_skin_retouch(&image_with(edged), &full_mask(), FACE);
        assert!(correction(&retouch.blemish, 96, 100).abs() < 0.02);
    }

    #[test]
    fn an_empty_mask_yields_no_corrections() {
        let retouch = build_skin_retouch(&image_with(spotted), &vec![0; SIZE * SIZE], FACE);
        assert!(retouch.smooth.iter().chain(&retouch.blemish).all(|&bits| bits == 0));
    }
}
