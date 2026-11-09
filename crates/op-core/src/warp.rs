//! Liquify: a displacement field built up from brush strokes.
//!
//! The field stores, for each position, the offset in UV to read the image from. It
//! is applied after developing, so masks and skin corrections, which are aligned to
//! the unwarped image, stay correct. Strokes are kept as fractions of the image, so
//! the field can be rebuilt at any time.

use half::f16;
use serde::{Deserialize, Serialize};

use crate::output::Sample;

const FIELD_LONG_EDGE: u32 = 512;
/// How far one bloat or pucker step moves content, relative to its distance from the centre.
const RADIAL_RATE: f32 = 0.08;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarpStroke {
    /// Brush centre in UV.
    pub x: f32,
    pub y: f32,
    /// How far the brush moved, in UV.
    pub dx: f32,
    pub dy: f32,
    /// Radius as a fraction of the long edge.
    pub radius: f32,
    /// 0..=1.
    pub strength: f32,
    /// 0 pushes with the brush, 1 bloats, 2 puckers.
    #[serde(default)]
    pub mode: u8,
}

#[derive(Clone, Debug)]
pub struct WarpField {
    pub width: u32,
    pub height: u32,
    image_width: f32,
    image_height: f32,
    /// Interleaved (du, dv).
    offsets: Vec<f32>,
    touched: bool,
}

impl WarpField {
    /// An empty field for an image of the given shape.
    pub fn new(image_width: u32, image_height: u32) -> Self {
        let long = image_width.max(image_height).max(1) as f32;
        let scale = FIELD_LONG_EDGE as f32 / long;
        let width = ((image_width as f32 * scale).round() as u32).max(2);
        let height = ((image_height as f32 * scale).round() as u32).max(2);
        Self {
            width,
            height,
            image_width: image_width as f32,
            image_height: image_height as f32,
            offsets: vec![0.0; width as usize * height as usize * 2],
            touched: false,
        }
    }

    pub fn is_identity(&self) -> bool {
        !self.touched
    }

    /// Interleaved (du, dv) for the whole field.
    pub fn offsets(&self) -> &[f32] {
        &self.offsets
    }

    /// Bilinear lookup of the offset at a UV position.
    pub fn offset_at(&self, u: f32, v: f32) -> (f32, f32) {
        sample(&self.offsets, self.width as usize, self.height as usize, u, v)
    }

    /// Adds a stroke, dragging the image along with the brush.
    pub fn apply(&mut self, stroke: &WarpStroke) {
        let (width, height) = (self.width as usize, self.height as usize);
        let long = self.image_width.max(self.image_height);
        let (aspect_x, aspect_y) = (self.image_width / long, self.image_height / long);
        let radius_squared = stroke.radius * stroke.radius;

        // The region to change: a brush radius around the centre, in field cells.
        let half_x = stroke.radius / aspect_x;
        let half_y = stroke.radius / aspect_y;
        let to_cell = |value: f32, size: usize| (value * size as f32 - 0.5).clamp(0.0, (size - 1) as f32);
        let x0 = to_cell(stroke.x - half_x, width).floor() as usize;
        let x1 = to_cell(stroke.x + half_x, width).ceil() as usize;
        let y0 = to_cell(stroke.y - half_y, height).floor() as usize;
        let y1 = to_cell(stroke.y + half_y, height).ceil() as usize;

        // Each cell reads its old offset from where the brush pushed it from, which
        // composes strokes properly instead of just adding them up.
        let previous = self.offsets.clone();
        for y in y0..=y1 {
            let v = (y as f32 + 0.5) / height as f32;
            for x in x0..=x1 {
                let u = (x as f32 + 0.5) / width as f32;
                let dist_x = (u - stroke.x) * aspect_x;
                let dist_y = (v - stroke.y) * aspect_y;
                let falloff = 1.0 - (dist_x * dist_x + dist_y * dist_y) / radius_squared;
                if falloff <= 0.0 {
                    continue;
                }
                let amount = falloff * falloff * stroke.strength;
                let (push_u, push_v) = match stroke.mode {
                    1 => (-(u - stroke.x) * amount * RADIAL_RATE, -(v - stroke.y) * amount * RADIAL_RATE),
                    2 => ((u - stroke.x) * amount * RADIAL_RATE, (v - stroke.y) * amount * RADIAL_RATE),
                    _ => (-stroke.dx * amount, -stroke.dy * amount),
                };
                let (old_u, old_v) = sample(&previous, width, height, u + push_u, v + push_v);
                let index = (y * width + x) * 2;
                self.offsets[index] = old_u + push_u;
                self.offsets[index + 1] = old_v + push_v;
            }
        }
        self.touched = true;
    }

    /// Offsets as RG half-float bits for the GPU.
    pub fn to_half_bits(&self) -> Vec<u16> {
        self.offsets.iter().map(|&value| f16::from_f32(value).to_bits()).collect()
    }

    /// Applies the field to tightly packed RGB.
    pub fn warp_rgb<S: Sample>(&self, rgb: &[S], width: u32, height: u32) -> Vec<S> {
        if self.is_identity() {
            return rgb.to_vec();
        }
        let (w, h) = (width as usize, height as usize);
        let mut out = vec![S::default(); rgb.len()];
        for row in 0..h {
            let v = (row as f32 + 0.5) / h as f32;
            for col in 0..w {
                let u = (col as f32 + 0.5) / w as f32;
                let (du, dv) = self.offset_at(u, v);
                let px = ((u + du) * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
                let py = ((v + dv) * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
                let (x0, y0) = (px as usize, py as usize);
                let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
                let (tx, ty) = (px.fract(), py.fract());
                let target = (row * w + col) * 3;
                for channel in 0..3 {
                    let at = |x: usize, y: usize| rgb[(y * w + x) * 3 + channel].to_f32();
                    let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
                    let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
                    out[target + channel] = S::from_f32(top * (1.0 - ty) + bottom * ty);
                }
            }
        }
        out
    }
}

fn sample(offsets: &[f32], width: usize, height: usize, u: f32, v: f32) -> (f32, f32) {
    let px = (u * width as f32 - 0.5).clamp(0.0, (width - 1) as f32);
    let py = (v * height as f32 - 0.5).clamp(0.0, (height - 1) as f32);
    let (x0, y0) = (px as usize, py as usize);
    let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
    let (tx, ty) = (px.fract(), py.fract());
    let at = |x: usize, y: usize, channel: usize| offsets[(y * width + x) * 2 + channel];
    let mix = |channel: usize| {
        let top = at(x0, y0, channel) * (1.0 - tx) + at(x1, y0, channel) * tx;
        let bottom = at(x0, y1, channel) * (1.0 - tx) + at(x1, y1, channel) * tx;
        top * (1.0 - ty) + bottom * ty
    };
    (mix(0), mix(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: u32 = 256;

    fn stroke(dx: f32) -> WarpStroke {
        WarpStroke { x: 0.5, y: 0.5, dx, dy: 0.0, radius: 0.2, strength: 1.0, mode: 0 }
    }

    /// A vertical bright bar at x = 128.
    fn bar() -> Vec<u8> {
        let mut rgb = vec![0u8; (SIZE * SIZE * 3) as usize];
        for y in 0..SIZE as usize {
            for x in 124..132usize {
                rgb[(y * SIZE as usize + x) * 3] = 255;
            }
        }
        rgb
    }

    fn brightest_x(rgb: &[u8], row: usize) -> usize {
        (0..SIZE as usize).max_by_key(|x| rgb[(row * SIZE as usize + x) * 3]).unwrap()
    }

    #[test]
    fn an_empty_field_leaves_the_image_alone() {
        let field = WarpField::new(SIZE, SIZE);
        assert!(field.is_identity());
        assert_eq!(field.warp_rgb(&bar(), SIZE, SIZE), bar());
    }

    #[test]
    fn dragging_right_moves_content_right() {
        let mut field = WarpField::new(SIZE, SIZE);
        field.apply(&stroke(0.1));
        let out = field.warp_rgb(&bar(), SIZE, SIZE);
        let moved = brightest_x(&out, 128);
        assert!(moved > 140, "bar should move right at the brush centre, found at {moved}");
    }

    #[test]
    fn content_outside_the_brush_stays_put() {
        let mut field = WarpField::new(SIZE, SIZE);
        field.apply(&stroke(0.1));
        let out = field.warp_rgb(&bar(), SIZE, SIZE);
        assert_eq!(brightest_x(&out, 10), brightest_x(&bar(), 10));
    }

    #[test]
    fn opposite_strokes_nearly_cancel() {
        let mut field = WarpField::new(SIZE, SIZE);
        field.apply(&stroke(0.02));
        field.apply(&stroke(-0.02));
        let (du, _) = field.offset_at(0.5, 0.5);
        assert!(du.abs() < 2e-3, "residual offset {du}");
    }

    #[test]
    fn strength_scales_the_result() {
        let mut full = WarpField::new(SIZE, SIZE);
        full.apply(&stroke(0.05));
        let mut half = WarpField::new(SIZE, SIZE);
        half.apply(&WarpStroke { strength: 0.5, ..stroke(0.05) });
        assert!(half.offset_at(0.5, 0.5).0.abs() < full.offset_at(0.5, 0.5).0.abs());
    }

    #[test]
    fn bloat_reads_from_closer_to_the_centre() {
        let mut field = WarpField::new(SIZE, SIZE);
        field.apply(&WarpStroke { mode: 1, ..stroke(0.0) });
        assert!(field.offset_at(0.6, 0.5).0 < 0.0, "content right of centre spreads outward");
    }

    #[test]
    fn pucker_is_the_opposite_of_bloat() {
        let mut field = WarpField::new(SIZE, SIZE);
        field.apply(&WarpStroke { mode: 2, ..stroke(0.0) });
        assert!(field.offset_at(0.6, 0.5).0 > 0.0);
    }

    #[test]
    fn the_brush_is_round_on_a_wide_image() {
        let mut field = WarpField::new(1000, 500);
        field.apply(&WarpStroke { x: 0.5, y: 0.5, dx: 0.05, dy: 0.0, radius: 0.1, strength: 1.0, mode: 0 });
        // 0.05 of the long edge away: u changes by 0.05, v by 0.1.
        let horizontal = field.offset_at(0.55, 0.5).0;
        let vertical = field.offset_at(0.5, 0.6).0;
        assert!((horizontal - vertical).abs() < 0.002, "{horizontal} vs {vertical}");
    }
}
