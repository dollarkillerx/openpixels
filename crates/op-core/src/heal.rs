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
    Some(HealSpot { x, y, radius, source_dx: dx / long_edge, source_dy: dy / long_edge, clone: false })
}

/// Applies spots in order, so a later spot can build on an earlier one.
pub fn apply_heals(image: &mut LinearImage, spots: &[HealSpot]) {
    let long_edge = image.width.max(image.height) as f32;
    let (width, height) = (image.width as isize, image.height as isize);

    for spot in spots {
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

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: u32 = 200;

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
        let spot = HealSpot { x: 0.75, y: 0.5, radius: 0.05, source_dx: -0.5, source_dy: 0.0, clone: true };
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
