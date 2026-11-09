//! Crop and rotation, shared by the on-screen preview and the export.
//!
//! The image is turned by quarter turns, then rotated by a small angle about its
//! centre, and the crop is a rectangle in the resulting frame. Every output position
//! therefore maps to the source through one affine transform.

use serde::Deserialize;

use crate::output::Sample;

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Geometry {
    /// Crop rectangle in the turned frame, as fractions of its size.
    pub crop_x: f32,
    pub crop_y: f32,
    pub crop_w: f32,
    pub crop_h: f32,
    /// Clockwise rotation in degrees.
    pub rotate: f32,
    /// Clockwise quarter turns.
    pub quarter: i32,
}

impl Default for Geometry {
    fn default() -> Self {
        Self { crop_x: 0.0, crop_y: 0.0, crop_w: 1.0, crop_h: 1.0, rotate: 0.0, quarter: 0 }
    }
}

/// Source UV of an output position `(s, t)` in 0..=1 is `origin + s * axis_s + t * axis_t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UvMap {
    pub origin: [f32; 2],
    pub axis_s: [f32; 2],
    pub axis_t: [f32; 2],
}

impl Geometry {
    /// The same transform showing the whole frame, used while editing the crop.
    pub fn whole_frame(self) -> Self {
        Self { crop_x: 0.0, crop_y: 0.0, crop_w: 1.0, crop_h: 1.0, ..self }
    }

    fn turns(self) -> i32 {
        self.quarter.rem_euclid(4)
    }

    fn frame_size(self, width: u32, height: u32) -> (f32, f32) {
        if self.turns() % 2 == 1 { (height as f32, width as f32) } else { (width as f32, height as f32) }
    }

    /// Output size in pixels for a source of `width` x `height`.
    pub fn output_size(self, width: u32, height: u32) -> (u32, u32) {
        let (frame_w, frame_h) = self.frame_size(width, height);
        (((frame_w * self.crop_w).round() as u32).max(1), ((frame_h * self.crop_h).round() as u32).max(1))
    }

    pub fn uv_map(self, width: u32, height: u32) -> UvMap {
        let (frame_w, frame_h) = self.frame_size(width, height);
        let (sin, cos) = self.rotate.to_radians().sin_cos();
        let (src_w, src_h) = (width as f32, height as f32);
        let turns = self.turns();

        let to_uv = |s: f32, t: f32| -> [f32; 2] {
            let x = (self.crop_x + s * self.crop_w - 0.5) * frame_w;
            let y = (self.crop_y + t * self.crop_h - 0.5) * frame_h;
            let (unrotated_x, unrotated_y) = (x * cos + y * sin, -x * sin + y * cos);
            let (source_x, source_y) = match turns {
                1 => (unrotated_y, -unrotated_x),
                2 => (-unrotated_x, -unrotated_y),
                3 => (-unrotated_y, unrotated_x),
                _ => (unrotated_x, unrotated_y),
            };
            [source_x / src_w + 0.5, source_y / src_h + 0.5]
        };

        let origin = to_uv(0.0, 0.0);
        let end_s = to_uv(1.0, 0.0);
        let end_t = to_uv(0.0, 1.0);
        UvMap {
            origin,
            axis_s: [end_s[0] - origin[0], end_s[1] - origin[1]],
            axis_t: [end_t[0] - origin[0], end_t[1] - origin[1]],
        }
    }

    /// Applies the transform to tightly packed RGB. Areas outside the source are black.
    pub fn apply_rgb<S: Sample>(self, rgb: &[S], width: u32, height: u32) -> (Vec<S>, u32, u32) {
        if self == Self::default() {
            return (rgb.to_vec(), width, height);
        }
        let (out_w, out_h) = self.output_size(width, height);
        let map = self.uv_map(width, height);
        let (w, h) = (width as usize, height as usize);
        let mut out = vec![S::default(); out_w as usize * out_h as usize * 3];

        for row in 0..out_h {
            let t = (row as f32 + 0.5) / out_h as f32;
            let start = row as usize * out_w as usize * 3;
            for (col, target) in out[start..start + out_w as usize * 3].chunks_exact_mut(3).enumerate() {
                let s = (col as f32 + 0.5) / out_w as f32;
                let px = (map.origin[0] + s * map.axis_s[0] + t * map.axis_t[0]) * width as f32 - 0.5;
                let py = (map.origin[1] + s * map.axis_s[1] + t * map.axis_t[1]) * height as f32 - 0.5;
                if px < -0.5 || py < -0.5 || px > w as f32 - 0.5 || py > h as f32 - 0.5 {
                    continue;
                }
                let (px, py) = (px.clamp(0.0, (w - 1) as f32), py.clamp(0.0, (h - 1) as f32));
                let (x0, y0) = (px as usize, py as usize);
                let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
                let (tx, ty) = (px.fract(), py.fract());
                for channel in 0..3 {
                    let at = |x: usize, y: usize| rgb[(y * w + x) * 3 + channel].to_f32();
                    let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
                    let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
                    target[channel] = S::from_f32(top * (1.0 - ty) + bottom * ty);
                }
            }
        }
        (out, out_w, out_h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4x2 image whose red channel encodes position.
    fn sample() -> Vec<u8> {
        (0..8u8).flat_map(|v| [v * 10, 0, 0]).collect()
    }

    fn reds(rgb: &[u8]) -> Vec<u8> {
        rgb.chunks_exact(3).map(|p| p[0]).collect()
    }

    #[test]
    fn identity_is_unchanged() {
        let (out, w, h) = Geometry::default().apply_rgb(&sample(), 4, 2);
        assert_eq!((w, h), (4, 2));
        assert_eq!(out, sample());
    }

    #[test]
    fn crop_selects_the_requested_pixels() {
        let geometry = Geometry { crop_x: 0.5, crop_w: 0.5, ..Default::default() };
        let (out, w, h) = geometry.apply_rgb(&sample(), 4, 2);
        assert_eq!((w, h), (2, 2));
        assert_eq!(reds(&out), vec![20, 30, 60, 70]);
    }

    #[test]
    fn quarter_turns_rotate_clockwise() {
        let geometry = Geometry { quarter: 1, ..Default::default() };
        let (out, w, h) = geometry.apply_rgb(&sample(), 4, 2);
        assert_eq!((w, h), (2, 4));
        // Clockwise: the bottom-left source pixel lands top-left.
        assert_eq!(reds(&out), vec![40, 0, 50, 10, 60, 20, 70, 30]);
    }

    #[test]
    fn two_quarter_turns_reverse_the_image() {
        let geometry = Geometry { quarter: 2, ..Default::default() };
        let (out, _, _) = geometry.apply_rgb(&sample(), 4, 2);
        assert_eq!(reds(&out), vec![70, 60, 50, 40, 30, 20, 10, 0]);
    }

    #[test]
    fn negative_turns_are_counter_clockwise() {
        let left = Geometry { quarter: -1, ..Default::default() };
        let right = Geometry { quarter: 3, ..Default::default() };
        assert_eq!(left.apply_rgb(&sample(), 4, 2), right.apply_rgb(&sample(), 4, 2));
    }

    #[test]
    fn rotation_leaves_black_outside_the_source() {
        let bright = vec![200u8; 64 * 64 * 3];
        let geometry = Geometry { rotate: 45.0, ..Default::default() };
        let (out, w, h) = geometry.apply_rgb(&bright, 64, 64);
        assert_eq!((w, h), (64, 64));
        assert_eq!(out[0], 0, "corner is outside the rotated image");
        assert_eq!(out[(32 * 64 + 32) * 3], 200, "centre stays inside");
    }

    #[test]
    fn uv_map_of_the_identity_is_the_unit_square() {
        let map = Geometry::default().uv_map(100, 50);
        assert_eq!(map.origin, [0.0, 0.0]);
        assert_eq!(map.axis_s, [1.0, 0.0]);
        assert_eq!(map.axis_t, [0.0, 1.0]);
    }

    #[test]
    fn missing_fields_mean_the_full_frame() {
        let geometry: Geometry = serde_json::from_str(r#"{"exposure": 1.0}"#).unwrap();
        assert_eq!(geometry, Geometry::default());
    }
}
