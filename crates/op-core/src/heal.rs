//! Manual spot healing: replaces a circle with texture from nearby, matched in colour
//! at the edge so the patch blends in.
//!
//! Positions and sizes are fractions of the image, so a spot found on the preview
//! applies unchanged to the full-resolution image.

use crate::LinearImage;
use half::f16;
use serde::{Deserialize, Serialize};

const CANDIDATE_DIRECTIONS: usize = 16;
const CANDIDATE_DISTANCES: [f32; 3] = [2.2, 3.0, 4.0];
const RING_SAMPLES: usize = 32;
const RING_RADIUS: f32 = 1.15;
const FEATHER_START: f32 = 0.7;
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealSpot {
    /// Centre, as a fraction of the width and height.
    pub x: f32,
    pub y: f32,
    /// Radius as a fraction of the long edge.
    pub radius: f32,
    /// Offset to the source patch, as a fraction of the long edge.
    pub source_dx: f32,
    pub source_dy: f32,
    /// Copies the source exactly instead of matching it to its new surroundings.
    #[serde(default)]
    pub clone: bool,
    /// Takes the red out of an eye instead of copying anything.
    #[serde(default)]
    pub red_eye: bool,
}

fn pixel(image: &LinearImage, x: isize, y: isize) -> [f32; 3] {
    let x = x.clamp(0, image.width as isize - 1) as usize;
    let y = y.clamp(0, image.height as isize - 1) as usize;
    let start = (y * image.width as usize + x) * 3;
    [0, 1, 2].map(|c| f16::from_bits(image.pixels[start + c]).to_f32())
}

fn ring_points(cx: f32, cy: f32, radius: f32) -> impl Iterator<Item = (isize, isize)> {
    (0..RING_SAMPLES).map(move |i| {
        let angle = i as f32 / RING_SAMPLES as f32 * std::f32::consts::TAU;
        ((cx + radius * angle.cos()).round() as isize, (cy + radius * angle.sin()).round() as isize)
    })
}

fn ring_mean(image: &LinearImage, cx: f32, cy: f32, radius: f32) -> [f32; 3] {
    let mut total = [0.0f32; 3];
    for (x, y) in ring_points(cx, cy, radius) {
        let value = pixel(image, x, y);
        for channel in 0..3 {
            total[channel] += value[channel] / RING_SAMPLES as f32;
        }
    }
    total
}

/// Picks the nearby patch whose surroundings best match the target's. Returns the
/// spot, or `None` if no candidate fits inside the image.
pub fn find_heal_source(image: &LinearImage, x: f32, y: f32, radius: f32) -> Option<HealSpot> {
    let long_edge = image.width.max(image.height) as f32;
    let (cx, cy, r) = (x * image.width as f32, y * image.height as f32, (radius * long_edge).max(1.0));
    let ring = RING_RADIUS * r;
    let target: Vec<f32> = ring_points(cx, cy, ring)
        .map(|(px, py)| {
            let value = pixel(image, px, py);
            LUMA[0] * value[0] + LUMA[1] * value[1] + LUMA[2] * value[2]
        })
        .collect();
    let target_mean = target.iter().sum::<f32>() / target.len() as f32;

    let mut best: Option<(f32, f32, f32)> = None;
    for direction in 0..CANDIDATE_DIRECTIONS {
        let angle = direction as f32 / CANDIDATE_DIRECTIONS as f32 * std::f32::consts::TAU;
        for distance in CANDIDATE_DISTANCES {
            let (dx, dy) = (angle.cos() * distance * r, angle.sin() * distance * r);
            let (sx, sy) = (cx + dx, cy + dy);
            let margin = ring + 1.0;
            if sx < margin || sy < margin || sx > image.width as f32 - margin || sy > image.height as f32 - margin {
                continue;
            }
            let lumas: Vec<f32> = ring_points(sx, sy, ring)
                .map(|(px, py)| {
                    let value = pixel(image, px, py);
                    LUMA[0] * value[0] + LUMA[1] * value[1] + LUMA[2] * value[2]
                })
                .collect();
            let mean = lumas.iter().sum::<f32>() / lumas.len() as f32;
            // Compare shape after matching brightness, as the blend corrects that anyway.
            let gain = if mean > 1e-5 { target_mean / mean } else { 1.0 };
            let difference: f32 = lumas.iter().zip(&target).map(|(s, t)| (s * gain - t).powi(2)).sum();
            // Prefer the closest of equally good patches, where lighting is most alike.
            let score = difference * (1.0 + 0.05 * distance);
            if best.is_none_or(|(current, _, _)| score < current) {
                best = Some((score, dx, dy));
            }
        }
    }

    let (_, dx, dy) = best?;
    Some(HealSpot { x, y, radius, source_dx: dx / long_edge, source_dy: dy / long_edge, clone: false, red_eye: false })
}

/// Replaces red with the level of the other channels, inside a circle and only where
/// red clearly dominates, so skin caught in the circle keeps its colour.
fn remove_red_eye(image: &mut LinearImage, spot: &HealSpot) {
    const REDNESS_START: f32 = 2.0;
    const REDNESS_FULL: f32 = 3.5;
    let long_edge = image.width.max(image.height) as f32;
    let (width, height) = (image.width as isize, image.height as isize);
    let (cx, cy, r) = (spot.x * width as f32, spot.y * height as f32, (spot.radius * long_edge).max(1.0));
    let (x0, x1) = (((cx - r).floor() as isize).max(0), ((cx + r).ceil() as isize + 1).min(width));
    let (y0, y1) = (((cy - r).floor() as isize).max(0), ((cy + r).ceil() as isize + 1).min(height));
    for y in y0..y1 {
        for x in x0..x1 {
            let distance = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt() / r;
            if distance >= 1.0 {
                continue;
            }
            let [red, green, blue] = pixel(image, x, y);
            let others = (green + blue) / 2.0;
            let t = ((red / others.max(1e-4) - REDNESS_START) / (REDNESS_FULL - REDNESS_START)).clamp(0.0, 1.0);
            let edge = ((1.0 - distance) / (1.0 - FEATHER_START)).clamp(0.0, 1.0);
            let start = (y as usize * width as usize + x as usize) * 3;
            image.pixels[start] = f16::from_f32(red + (others - red) * t * edge).to_bits();
        }
    }
}

/// Applies spots in order, so a later spot can build on an earlier one.
pub fn apply_heals(image: &mut LinearImage, spots: &[HealSpot]) {
    let long_edge = image.width.max(image.height) as f32;
    let (width, height) = (image.width as isize, image.height as isize);

    for spot in spots {
        if spot.red_eye {
            remove_red_eye(image, spot);
            continue;
        }
        let (cx, cy, r) = (spot.x * width as f32, spot.y * height as f32, (spot.radius * long_edge).max(1.0));
        let (dx, dy) = ((spot.source_dx * long_edge).round() as isize, (spot.source_dy * long_edge).round() as isize);
        let ring = RING_RADIUS * r;

        // Match the patch to the colour around the target, per channel.
        let around_target = ring_mean(image, cx, cy, ring);
        let around_source = ring_mean(image, cx + dx as f32, cy + dy as f32, ring);
        let gain: [f32; 3] = std::array::from_fn(|c| {
            if !spot.clone && around_source[c] > 1e-5 { around_target[c] / around_source[c] } else { 1.0 }
        });

        let (x0, x1) = (((cx - r).floor() as isize).max(0), ((cx + r).ceil() as isize + 1).min(width));
        let (y0, y1) = (((cy - r).floor() as isize).max(0), ((cy + r).ceil() as isize + 1).min(height));
        let mut replaced = Vec::with_capacity(((x1 - x0).max(0) * (y1 - y0).max(0)) as usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let distance = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt() / r;
                if distance >= 1.0 {
                    continue;
                }
                let t = ((distance - FEATHER_START) / (1.0 - FEATHER_START)).clamp(0.0, 1.0);
                let alpha = 1.0 - t * t * (3.0 - 2.0 * t);
                let target = pixel(image, x, y);
                let source = pixel(image, x + dx, y + dy);
                let blended: [u16; 3] = std::array::from_fn(|c| {
                    f16::from_f32(target[c] * (1.0 - alpha) + source[c] * gain[c] * alpha).to_bits()
                });
                replaced.push(((y as usize * width as usize + x as usize) * 3, blended));
            }
        }
        // Written afterwards so the patch reads only unmodified pixels.
        for (start, value) in replaced {
            image.pixels[start..start + 3].copy_from_slice(&value);
        }
    }
}

/// A region filled in by the inpainting model, kept at the model's resolution so it
/// can be placed on the preview and on the full-resolution image alike.
#[derive(Clone, Debug)]
pub struct ErasePatch {
    /// The rectangle it covers, as fractions of the width and height.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Side of the square `rgb` and `alpha` rasters.
    pub size: u32,
    /// sRGB-encoded, interleaved.
    pub rgb: Vec<u8>,
    /// 255 replaces the image, 0 leaves it alone.
    pub alpha: Vec<u8>,
}

/// Lays the patches over the image, in order, so a later one can build on an earlier one.
pub fn apply_erases(image: &mut LinearImage, patches: &[ErasePatch]) {
    if patches.is_empty() {
        return;
    }
    let to_linear = crate::decode::srgb_to_linear_table().map(|bits| f16::from_bits(bits).to_f32());
    let (width, height) = (image.width as usize, image.height as usize);

    for patch in patches {
        let size = patch.size as usize;
        let last = patch.size as f32 - 1.0;
        let (left, top) = (patch.x * width as f32, patch.y * height as f32);
        let (span_x, span_y) = (patch.width * width as f32, patch.height * height as f32);
        let x_range = (left.floor().max(0.0) as usize)..((left + span_x).ceil() as usize).min(width);
        let y_range = (top.floor().max(0.0) as usize)..((top + span_y).ceil() as usize).min(height);

        for y in y_range {
            let fy = ((y as f32 + 0.5 - top) / span_y * patch.size as f32 - 0.5).clamp(0.0, last);
            let (row0, ty) = (fy as usize, fy.fract());
            let row1 = (row0 + 1).min(size - 1);
            for x in x_range.clone() {
                let fx = ((x as f32 + 0.5 - left) / span_x * patch.size as f32 - 0.5).clamp(0.0, last);
                let (col0, tx) = (fx as usize, fx.fract());
                let col1 = (col0 + 1).min(size - 1);
                let corners = [
                    (row0 * size + col0, (1.0 - tx) * (1.0 - ty)),
                    (row0 * size + col1, tx * (1.0 - ty)),
                    (row1 * size + col0, (1.0 - tx) * ty),
                    (row1 * size + col1, tx * ty),
                ];
                let alpha: f32 = corners.iter().map(|&(at, w)| f32::from(patch.alpha[at]) * w).sum::<f32>() / 255.0;
                if alpha <= 0.0 {
                    continue;
                }
                let start = (y * width + x) * 3;
                for channel in 0..3 {
                    let fill: f32 = corners.iter().map(|&(at, w)| to_linear[patch.rgb[at * 3 + channel] as usize] * w).sum();
                    let current = f16::from_bits(image.pixels[start + channel]).to_f32();
                    image.pixels[start + channel] = f16::from_f32(current * (1.0 - alpha) + fill * alpha).to_bits();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: u32 = 200;

    /// Mid-grey fill over the middle of the patch only.
    fn centre_patch() -> ErasePatch {
        const PATCH: usize = 64;
        let alpha = (0..PATCH * PATCH)
            .map(|i| {
                let (x, y) = (i % PATCH, i / PATCH);
                if (16..48).contains(&x) && (16..48).contains(&y) { 255 } else { 0 }
            })
            .collect();
        ErasePatch { x: 0.25, y: 0.25, width: 0.5, height: 0.5, size: PATCH as u32, rgb: vec![188; PATCH * PATCH * 3], alpha }
    }

    #[test]
    fn an_erase_patch_replaces_only_where_its_alpha_allows() {
        let mut image = image_with(|_, _| 0.05);
        apply_erases(&mut image, &[centre_patch()]);
        assert!((value(&image, 100, 100) - 0.503).abs() < 0.01, "got {}", value(&image, 100, 100));
        // Inside the rectangle but outside the alpha, and outside the rectangle.
        assert!((value(&image, 55, 55) - 0.05).abs() < 1e-3);
        assert!((value(&image, 10, 10) - 0.05).abs() < 1e-3);
    }

    #[test]
    fn an_erase_patch_lands_in_the_same_place_at_any_resolution() {
        let mut large = LinearImage::new(800, 800, vec![f16::from_f32(0.05).to_bits(); 800 * 800 * 3]);
        apply_erases(&mut large, &[centre_patch()]);
        let at = |x: usize, y: usize| f16::from_bits(large.pixels[(y * 800 + x) * 3]).to_f32();
        assert!(at(400, 400) > 0.45);
        assert!((at(220, 220) - 0.05).abs() < 1e-3);
    }

    #[test]
    fn a_crop_copies_the_requested_rectangle() {
        let image = image_with(|x, y| (x + y) as f32 / 400.0);
        let crop = image.crop(10, 20, 30, 40);
        assert_eq!((crop.width, crop.height), (30, 40));
        assert_eq!(crop.pixels[0], image.pixels[(20 * SIZE as usize + 10) * 3]);
    }

    fn image_with(paint: impl Fn(u32, u32) -> f32) -> LinearImage {
        let mut pixels = Vec::new();
        for y in 0..SIZE {
            for x in 0..SIZE {
                pixels.extend([f16::from_f32(paint(x, y)).to_bits(); 3]);
            }
        }
        LinearImage::new(SIZE, SIZE, pixels)
    }

    fn value(image: &LinearImage, x: u32, y: u32) -> f32 {
        f16::from_bits(image.pixels[(y * SIZE + x) as usize * 3]).to_f32()
    }

    fn spotted(x: u32, y: u32) -> f32 {
        let (dx, dy) = (x as f32 - 100.0, y as f32 - 100.0);
        if dx * dx + dy * dy <= 16.0 { 0.05 } else { 0.5 }
    }

    #[test]
    fn red_eye_removal_neutralises_red_and_leaves_skin_alone() {
        let mut pixels = Vec::new();
        for y in 0..SIZE {
            for x in 0..SIZE {
                let colour: [f32; 3] = if x < 100 && y < 100 { [0.6, 0.1, 0.1] } else { [0.5, 0.35, 0.3] };
                pixels.extend(colour.map(|v| f16::from_f32(v).to_bits()));
            }
        }
        let mut image = LinearImage::new(SIZE, SIZE, pixels);
        let spot = |x, y| HealSpot { x, y, radius: 0.1, source_dx: 0.0, source_dy: 0.0, clone: false, red_eye: true };
        apply_heals(&mut image, &[spot(0.25, 0.25), spot(0.75, 0.75)]);
        let channel = |x: u32, y: u32, c: u32| f16::from_bits(image.pixels[((y * SIZE + x) * 3 + c) as usize]).to_f32();
        assert!((channel(50, 50, 0) - 0.1).abs() < 0.01, "red pupil: {}", channel(50, 50, 0));
        assert!((channel(150, 150, 0) - 0.5).abs() < 1e-3, "skin in the circle: {}", channel(150, 150, 0));
    }

    #[test]
    fn a_spot_is_replaced_by_its_surroundings() {
        let mut image = image_with(spotted);
        let spot = find_heal_source(&image, 0.5, 0.5, 0.04).unwrap();
        apply_heals(&mut image, &[spot]);
        assert!((value(&image, 100, 100) - 0.5).abs() < 0.01, "got {}", value(&image, 100, 100));
    }

    #[test]
    fn pixels_outside_the_spot_are_untouched() {
        let original = image_with(spotted);
        let mut image = original.clone();
        let spot = find_heal_source(&image, 0.5, 0.5, 0.04).unwrap();
        apply_heals(&mut image, &[spot]);
        assert_eq!(value(&image, 10, 10), value(&original, 10, 10));
        assert_eq!(value(&image, 120, 100), value(&original, 120, 100));
    }

    #[test]
    fn the_patch_is_matched_to_local_brightness() {
        // Brightness grows across the image; the source patch must be scaled to fit.
        let ramp = |x: u32, y: u32| {
            let (dx, dy) = (x as f32 - 100.0, y as f32 - 100.0);
            if dx * dx + dy * dy <= 16.0 { 0.02 } else { 0.1 + x as f32 / SIZE as f32 * 0.6 }
        };
        let mut image = image_with(ramp);
        let spot = find_heal_source(&image, 0.5, 0.5, 0.04).unwrap();
        apply_heals(&mut image, &[spot]);
        assert!((value(&image, 100, 100) - 0.4).abs() < 0.05, "got {}", value(&image, 100, 100));
    }

    #[test]
    fn a_cloned_spot_copies_the_source_without_matching_it() {
        let halves = |x: u32, _y: u32| if x < 100 { 0.8 } else { 0.2 };
        let mut image = image_with(halves);
        let spot = HealSpot { x: 0.75, y: 0.5, radius: 0.05, source_dx: -0.5, source_dy: 0.0, clone: true, red_eye: false };
        apply_heals(&mut image, &[spot]);
        assert!((value(&image, 150, 100) - 0.8).abs() < 0.01, "got {}", value(&image, 150, 100));
    }

    #[test]
    fn a_spot_applies_at_any_resolution() {
        let small = image_with(spotted);
        let spot = find_heal_source(&small, 0.5, 0.5, 0.04).unwrap();
        let mut pixels = Vec::new();
        for y in 0..SIZE * 2 {
            for x in 0..SIZE * 2 {
                pixels.extend([f16::from_f32(spotted(x / 2, y / 2)).to_bits(); 3]);
            }
        }
        let mut large = LinearImage::new(SIZE * 2, SIZE * 2, pixels);
        apply_heals(&mut large, &[spot]);
        let healed = f16::from_bits(large.pixels[(SIZE as usize * SIZE as usize * 2 + SIZE as usize) * 3]).to_f32();
        assert!((healed - 0.5).abs() < 0.01, "got {healed}");
    }
}
