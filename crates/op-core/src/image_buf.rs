use half::f16;

/// Scene-linear image with sRGB primaries, stored as interleaved RGB half floats.
///
/// RGB rather than RGBA because the full-resolution buffer is the largest allocation
/// in the app: at 60 MP the alpha channel alone would cost 120 MB of wasm32 heap.
/// RGBA is only produced for the small regions handed to the GPU.
#[derive(Clone, Debug)]
pub struct LinearImage {
    pub width: u32,
    pub height: u32,
    /// `f16` bit patterns, `width * height * 3` long.
    pub pixels: Vec<u16>,
}

impl LinearImage {
    pub fn new(width: u32, height: u32, pixels: Vec<u16>) -> Self {
        assert_eq!(pixels.len(), width as usize * height as usize * 3);
        Self { width, height, pixels }
    }

    /// Box-filtered copy whose long edge is at most `max_long_edge`, averaged in
    /// linear light. Uses an integer factor, so the result can be up to 2x smaller
    /// than requested; the proxy only drives the on-screen preview.
    pub fn proxy(&self, max_long_edge: u32) -> LinearImage {
        let long_edge = self.width.max(self.height);
        let factor = long_edge.div_ceil(max_long_edge.max(1)).max(1);
        if factor == 1 {
            return self.clone();
        }

        // A very elongated image can have a short edge smaller than `factor`.
        let (w, h) = ((self.width / factor).max(1), (self.height / factor).max(1));
        let f = factor as usize;
        let stride = self.width as usize * 3;
        let mut out = Vec::with_capacity(w as usize * h as usize * 3);

        for y in 0..h as usize {
            let mut sum = vec![([0.0f32; 3], 0u32); w as usize];
            for row in self.pixels[y * f * stride..].chunks_exact(stride).take(f) {
                for (block, (acc, count)) in row.chunks(f * 3).zip(sum.iter_mut()) {
                    for px in block.chunks_exact(3) {
                        acc[0] += f16::from_bits(px[0]).to_f32();
                        acc[1] += f16::from_bits(px[1]).to_f32();
                        acc[2] += f16::from_bits(px[2]).to_f32();
                        *count += 1;
                    }
                }
            }
            for (acc, count) in &sum {
                for channel in acc {
                    out.push(f16::from_f32(channel / *count as f32).to_bits());
                }
            }
        }
        LinearImage::new(w, h, out)
    }

    /// Resizes to `out_width` x `out_height` and returns a sRGB-encoded, normalised
    /// float tensor in NCHW order, the input segmentation models are trained on.
    pub fn srgb_nchw_tensor(
        &self,
        out_width: u32,
        out_height: u32,
        mean: [f32; 3],
        std: [f32; 3],
    ) -> Vec<f32> {
        let (out_w, out_h) = (out_width as usize, out_height as usize);
        let plane = out_w * out_h;
        let mut out = vec![0.0f32; 3 * plane];
        let (width, height) = (self.width as usize, self.height as usize);
        let sample = |x: usize, y: usize, c: usize| {
            f16::from_bits(self.pixels[(y * width + x) * 3 + c]).to_f32()
        };

        for row in 0..out_h {
            let fy = ((row as f32 + 0.5) * height as f32 / out_height as f32 - 0.5).max(0.0);
            let (y0, ty) = (fy as usize, fy.fract());
            let y1 = (y0 + 1).min(height - 1);
            for col in 0..out_w {
                let fx = ((col as f32 + 0.5) * width as f32 / out_width as f32 - 0.5).max(0.0);
                let (x0, tx) = (fx as usize, fx.fract());
                let x1 = (x0 + 1).min(width - 1);
                for c in 0..3 {
                    let top = sample(x0, y0, c) * (1.0 - tx) + sample(x1, y0, c) * tx;
                    let bottom = sample(x0, y1, c) * (1.0 - tx) + sample(x1, y1, c) * tx;
                    let linear = top * (1.0 - ty) + bottom * ty;
                    out[c * plane + row * out_w + col] = (linear_to_srgb(linear) - mean[c]) / std[c];
                }
            }
        }
        out
    }

    /// Copies a rectangle out as tightly packed RGBA half floats, with alpha 1.
    pub fn rgba_region(&self, x: u32, y: u32, w: u32, h: u32) -> Vec<u16> {
        assert!(x + w <= self.width && y + h <= self.height, "region out of bounds");
        let one = f16::from_f32(1.0).to_bits();
        let stride = self.width as usize * 3;
        let mut out = Vec::with_capacity(w as usize * h as usize * 4);
        for row in 0..h as usize {
            let start = (y as usize + row) * stride + x as usize * 3;
            for px in self.pixels[start..start + w as usize * 3].chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], one]);
            }
        }
        out
    }
}

fn linear_to_srgb(linear: f32) -> f32 {
    let c = linear.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_tensor_is_normalised_nchw() {
        let (mean, std) = ([0.5, 0.4, 0.3], [0.5, 0.25, 0.2]);
        let tensor = gray(40, 20, 0.18).srgb_nchw_tensor(8, 4, mean, std);
        assert_eq!(tensor.len(), 3 * 8 * 4);
        let srgb = linear_to_srgb(0.18);
        let (red, blue) = (tensor[0], tensor[2 * 8 * 4]);
        assert!((red - (srgb - 0.5) / 0.5).abs() < 1e-3);
        assert!((blue - (srgb - 0.3) / 0.2).abs() < 1e-3);
    }

    #[test]
    fn model_tensor_keeps_positions_at_a_non_square_size() {
        let (a, b) = (f16::from_f32(0.0).to_bits(), f16::from_f32(1.0).to_bits());
        let image = LinearImage::new(2, 1, vec![a, a, a, b, b, b]);
        let tensor = image.srgb_nchw_tensor(2, 1, [0.0; 3], [1.0; 3]);
        assert!(tensor[0] < tensor[1], "left should be darker than right");
    }

    fn gray(width: u32, height: u32, value: f32) -> LinearImage {
        let bits = f16::from_f32(value).to_bits();
        LinearImage::new(width, height, vec![bits; width as usize * height as usize * 3])
    }

    #[test]
    fn proxy_fits_long_edge_and_preserves_level() {
        let proxy = gray(1000, 500, 0.25).proxy(200);
        assert_eq!((proxy.width, proxy.height), (200, 100));
        assert_eq!(f16::from_bits(proxy.pixels[0]).to_f32(), 0.25);
    }

    #[test]
    fn proxy_of_small_image_is_unchanged() {
        let proxy = gray(100, 50, 0.5).proxy(200);
        assert_eq!((proxy.width, proxy.height), (100, 50));
    }

    #[test]
    fn proxy_averages_neighbouring_pixels() {
        let (a, b) = (f16::from_f32(0.0).to_bits(), f16::from_f32(1.0).to_bits());
        let image = LinearImage::new(2, 1, vec![a, a, a, b, b, b]);
        let proxy = image.proxy(1);
        assert_eq!(f16::from_bits(proxy.pixels[0]).to_f32(), 0.5);
    }

    #[test]
    fn rgba_region_reads_the_requested_window() {
        let values: Vec<u16> = (0..12).collect();
        let image = LinearImage::new(2, 2, values);
        let one = f16::from_f32(1.0).to_bits();
        assert_eq!(image.rgba_region(1, 1, 1, 1), vec![9, 10, 11, one]);
    }
}
