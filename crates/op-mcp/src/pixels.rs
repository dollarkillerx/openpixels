//! Small image helpers for what agents get back: JPEG previews, crops and measurements.

use serde_json::{Value, json};

/// An RGB8 image.
pub struct Rgb {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Rgb {
    /// The part of the image inside a rectangle given as fractions of it.
    pub fn region(&self, x: f64, y: f64, width: f64, height: f64) -> Rgb {
        let (w, h) = (f64::from(self.width), f64::from(self.height));
        let left = ((x.clamp(0.0, 1.0) * w).floor() as u32).min(self.width - 1);
        let top = ((y.clamp(0.0, 1.0) * h).floor() as u32).min(self.height - 1);
        let right = ((((x + width).clamp(0.0, 1.0)) * w).ceil() as u32).clamp(left + 1, self.width);
        let bottom = ((((y + height).clamp(0.0, 1.0)) * h).ceil() as u32).clamp(top + 1, self.height);
        let mut data = Vec::with_capacity(((right - left) * (bottom - top) * 3) as usize);
        for row in top..bottom {
            let start = ((row * self.width + left) * 3) as usize;
            data.extend_from_slice(&self.data[start..start + ((right - left) * 3) as usize]);
        }
        Rgb { width: right - left, height: bottom - top, data }
    }

    /// Side by side with a thin gap, top-aligned.
    pub fn beside(&self, other: &Rgb) -> Rgb {
        const GAP: u32 = 8;
        let height = self.height.max(other.height);
        let width = self.width + GAP + other.width;
        let mut data = vec![32u8; (width * height * 3) as usize];
        for (image, offset) in [(self, 0), (other, self.width + GAP)] {
            for row in 0..image.height {
                let from = (row * image.width * 3) as usize;
                let to = ((row * width + offset) * 3) as usize;
                data[to..to + (image.width * 3) as usize].copy_from_slice(&image.data[from..from + (image.width * 3) as usize]);
            }
        }
        Rgb { width, height, data }
    }

    pub fn jpeg(&self, quality: u8) -> Result<Vec<u8>, String> {
        op_gpu::encode_jpeg(&self.data, self.width, self.height, quality, &op_core::srgb_icc_profile(), None).map_err(|e| e.to_string())
    }

    /// Brightness and colour measurements an agent can reason about without looking.
    pub fn measure(&self) -> Value {
        const BINS: usize = 16;
        let pixels = (self.width * self.height) as usize;
        let mut luma = Vec::with_capacity(pixels);
        let (mut sums, mut chroma) = ([0f64; 3], 0f64);
        let (mut bright, mut dark) = (0usize, 0usize);
        let mut histogram = [0usize; BINS];
        for pixel in self.data.chunks_exact(3) {
            let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
            for (sum, value) in sums.iter_mut().zip([r, g, b]) {
                *sum += f64::from(value);
            }
            let y = 0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b);
            luma.push(y as u8);
            histogram[(y as usize * BINS / 256).min(BINS - 1)] += 1;
            let (max, min) = (r.max(g).max(b), r.min(g).min(b));
            if max >= 254 {
                bright += 1;
            }
            if max <= 2 {
                dark += 1;
            }
            if max > 0 {
                chroma += f64::from(max - min) / f64::from(max);
            }
        }
        luma.sort_unstable();
        let percentile = |p: f64| luma[((pixels - 1) as f64 * p).round() as usize];
        let mean = sums.map(|sum| sum / pixels as f64);
        let grey = (mean[0] + mean[1] + mean[2]) / 3.0;
        let share = |count: usize| (count as f64 / pixels as f64 * 1000.0).round() / 10.0;
        json!({
            "size": [self.width, self.height],
            "lumaPercentiles": { "p1": percentile(0.01), "p5": percentile(0.05), "p25": percentile(0.25), "median": percentile(0.5), "p75": percentile(0.75), "p95": percentile(0.95), "p99": percentile(0.99) },
            "clippedHighlightsPercent": share(bright),
            "crushedShadowsPercent": share(dark),
            "meanRgb": mean.map(|v| v.round()),
            // Positive is warmer than neutral, negative cooler; likewise magenta versus green.
            "warmth": ((mean[0] - mean[2]) / grey.max(1.0) * 100.0).round() / 100.0,
            "magenta": (((mean[0] + mean[2]) / 2.0 - mean[1]) / grey.max(1.0) * 100.0).round() / 100.0,
            "meanSaturation": (chroma / pixels as f64 * 100.0).round() / 100.0,
            "lumaHistogram16": histogram.map(|count| (count as f64 / pixels as f64 * 1000.0).round() / 10.0),
            "scale": "8-bit sRGB values, 0 to 255; percentages of all pixels"
        })
    }
}

/// Standard base64, for image content in MCP results.
pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = u32::from(chunk[0]) << 16 | chunk.get(1).map_or(0, |&b| u32::from(b) << 8) | chunk.get(2).map_or(0, |&b| u32::from(b));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xFF, 0xFE]), "//4=");
    }

    #[test]
    fn regions_and_measurements() {
        let mut data = Vec::new();
        for y in 0..4u8 {
            for x in 0..8u8 {
                data.extend([x * 36, y * 80, 255]);
            }
        }
        let image = Rgb { width: 8, height: 4, data };
        let right = image.region(0.5, 0.0, 0.5, 0.5);
        assert_eq!((right.width, right.height), (4, 2));
        assert_eq!(right.data[0], 4 * 36);
        let stats = image.measure();
        assert_eq!(stats["clippedHighlightsPercent"], json!(100.0));
        assert!(stats["warmth"].as_f64().unwrap() < 0.0, "a blue image reads as cool");
        let pair = image.beside(&right);
        assert_eq!((pair.width, pair.height), (8 + 8 + 4, 4));
    }
}
