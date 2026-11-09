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

    /// Upsamples a low-resolution matte to this image's size with a guided filter
    /// (He, Sun & Tang, 2010), so its edges follow the image's own edges and fine
    /// structure such as hair. Returns `(width, height, mask)`.
    pub fn refine_mask(&self, mask: &[u8], mask_width: u32, mask_height: u32) -> (u32, u32, Vec<u8>) {
        const RADIUS: usize = 8;
        const EPSILON: f32 = 1e-4;
        assert_eq!(mask.len(), mask_width as usize * mask_height as usize);

        let (width, height) = (self.width as usize, self.height as usize);
        let guide: Vec<f32> = self
            .pixels
            .chunks_exact(3)
            .map(|rgb| {
                let [r, g, b] = [rgb[0], rgb[1], rgb[2]].map(|bits| f16::from_bits(bits).to_f32());
                linear_to_srgb(0.2126 * r + 0.7152 * g + 0.0722 * b)
            })
            .collect();
        let input = resize_plane_bilinear(mask, mask_width as usize, mask_height as usize, width, height);

        let box_of = |plane: &[f32]| box_filter(plane, width, height, RADIUS);
        let product = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).collect::<Vec<f32>>();

        let mean_guide = box_of(&guide);
        let mean_input = box_of(&input);
        let correlation = box_of(&product(&guide, &input));
        let variance = box_of(&product(&guide, &guide));

        let mut slope = vec![0.0f32; width * height];
        let mut offset = vec![0.0f32; width * height];
        for i in 0..slope.len() {
            let spread = (variance[i] - mean_guide[i] * mean_guide[i]).max(0.0);
            slope[i] = (correlation[i] - mean_guide[i] * mean_input[i]) / (spread + EPSILON);
            offset[i] = mean_input[i] - slope[i] * mean_guide[i];
        }
        let mean_slope = box_of(&slope);
        let mean_offset = box_of(&offset);

        let refined = guide
            .iter()
            .zip(mean_slope.iter().zip(&mean_offset))
            .map(|(value, (a, b))| ((a * value + b).clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect();
        (self.width, self.height, refined)
    }

    /// A heavily blurred luminance image, as `(width, height, half-float bits)`, that
    /// the GPU samples by position to tell local contrast from overall brightness.
    pub fn blurred_luma(&self, long_edge: u32) -> (u32, u32, Vec<u16>) {
        let small = self.proxy(long_edge);
        let luma = small
            .pixels
            .chunks_exact(3)
            .map(|rgb| {
                let [r, g, b] = [rgb[0], rgb[1], rgb[2]].map(|bits| f16::from_bits(bits).to_f32());
                f16::from_f32(0.2126 * r + 0.7152 * g + 0.0722 * b).to_bits()
            })
            .collect();
        (small.width, small.height, luma)
    }

    /// The haze estimate for dehazing, as `(width, height, half-float bits, airlight)`:
    /// a smoothed dark channel and the colour of the haziest part of the image.
    pub fn haze_map(&self, long_edge: u32) -> (u32, u32, Vec<u16>, [f32; 3]) {
        const MIN_RADIUS: isize = 3;
        const BLUR_RADIUS: isize = 3;
        const HAZIEST_FRACTION: usize = 1000;

        let small = self.proxy(long_edge);
        let (width, height) = (small.width as usize, small.height as usize);
        let colours: Vec<[f32; 3]> = small
            .pixels
            .chunks_exact(3)
            .map(|rgb| [rgb[0], rgb[1], rgb[2]].map(|bits| f16::from_bits(bits).to_f32()))
            .collect();
        let dark: Vec<f32> = colours.iter().map(|c| c[0].min(c[1]).min(c[2])).collect();

        let window = |values: &[f32], radius: isize, fold: &dyn Fn(&mut dyn Iterator<Item = f32>) -> f32| -> Vec<f32> {
            let mut out = Vec::with_capacity(width * height);
            for y in 0..height as isize {
                for x in 0..width as isize {
                    let mut around = (-radius..=radius).flat_map(|dy| {
                        (-radius..=radius).map(move |dx| {
                            let sx = (x + dx).clamp(0, width as isize - 1) as usize;
                            let sy = (y + dy).clamp(0, height as isize - 1) as usize;
                            values[sy * width + sx]
                        })
                    });
                    out.push(fold(&mut around));
                }
            }
            out
        };
        let minimum = window(&dark, MIN_RADIUS, &|items| items.fold(f32::INFINITY, f32::min));
        let samples = ((2 * BLUR_RADIUS + 1) * (2 * BLUR_RADIUS + 1)) as f32;
        let smooth = window(&minimum, BLUR_RADIUS, &|items| items.sum::<f32>() / samples);

        let mut haziest: Vec<usize> = (0..colours.len()).collect();
        haziest.sort_unstable_by(|&a, &b| minimum[b].total_cmp(&minimum[a]));
        haziest.truncate((colours.len() / HAZIEST_FRACTION).max(1));
        let mut airlight = [0.0f32; 3];
        for &index in &haziest {
            for channel in 0..3 {
                airlight[channel] += colours[index][channel] / haziest.len() as f32;
            }
        }
        let bits = smooth.iter().map(|&value| f16::from_f32(value).to_bits()).collect();
        (small.width, small.height, bits, airlight)
    }

    /// The mean colour of a small square around a position in 0..=1.
    pub fn mean_around(&self, u: f32, v: f32, radius: u32) -> [f32; 3] {
        let (w, h) = (self.width as i64, self.height as i64);
        let (cx, cy) = (((u * w as f32) as i64).clamp(0, w - 1), ((v * h as f32) as i64).clamp(0, h - 1));
        let radius = i64::from(radius);
        let (mut total, mut count) = ([0.0f32; 3], 0.0f32);
        for y in (cy - radius).max(0)..=(cy + radius).min(h - 1) {
            for x in (cx - radius).max(0)..=(cx + radius).min(w - 1) {
                let start = (y * w + x) as usize * 3;
                for (sum, &bits) in total.iter_mut().zip(&self.pixels[start..start + 3]) {
                    *sum += f16::from_bits(bits).to_f32();
                }
                count += 1.0;
            }
        }
        total.map(|value| value / count)
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

/// Bilinear resize of an 8-bit plane to floats in 0..=1.
fn resize_plane_bilinear(plane: &[u8], width: usize, height: usize, out_w: usize, out_h: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(out_w * out_h);
    let sample = |x: usize, y: usize| f32::from(plane[y * width + x]) / 255.0;
    for row in 0..out_h {
        let fy = ((row as f32 + 0.5) * height as f32 / out_h as f32 - 0.5).max(0.0);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(height - 1);
        for col in 0..out_w {
            let fx = ((col as f32 + 0.5) * width as f32 / out_w as f32 - 0.5).max(0.0);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(width - 1);
            let top = sample(x0, y0) * (1.0 - tx) + sample(x1, y0) * tx;
            let bottom = sample(x0, y1) * (1.0 - tx) + sample(x1, y1) * tx;
            out.push(top * (1.0 - ty) + bottom * ty);
        }
    }
    out
}

/// Mean over a square window, in time independent of the radius. The window shrinks
/// at the image edges.
pub(crate) fn box_filter(plane: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let along = |input: &[f32], len: usize, stride: usize, lines: usize, step: usize| -> Vec<f32> {
        let mut output = vec![0.0f32; input.len()];
        for line in 0..lines {
            let at = |i: usize| line * stride + i * step;
            let mut sum = 0.0f32;
            let mut count = 0usize;
            for i in 0..len.min(radius + 1) {
                sum += input[at(i)];
                count += 1;
            }
            for i in 0..len {
                output[at(i)] = sum / count as f32;
                if i + radius + 1 < len {
                    sum += input[at(i + radius + 1)];
                    count += 1;
                }
                if i >= radius {
                    sum -= input[at(i - radius)];
                    count -= 1;
                }
            }
        }
        output
    };
    let horizontal = along(plane, width, width, height, 1);
    along(&horizontal, height, 1, width, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_filter_averages_and_handles_edges() {
        let plane = [0.0, 0.0, 3.0, 0.0, 0.0];
        let out = box_filter(&plane, 5, 1, 1);
        let expected = [0.0, 1.0, 1.0, 1.0, 0.0];
        for (value, wanted) in out.iter().zip(expected) {
            assert!((value - wanted).abs() < 1e-5, "{out:?}");
        }
    }

    #[test]
    fn refined_mask_snaps_to_the_image_edge() {
        // A hard vertical edge at x = 100, and a coarse matte that blurs across it.
        let (width, height) = (200u32, 40u32);
        let (dark, bright) = (f16::from_f32(0.02).to_bits(), f16::from_f32(0.8).to_bits());
        let mut pixels = Vec::new();
        for _ in 0..height {
            for x in 0..width {
                pixels.extend([if x < 100 { dark } else { bright }; 3]);
            }
        }
        let image = LinearImage::new(width, height, pixels);
        let coarse: Vec<u8> = (0..10u32 * 25)
            .map(|i| {
                let x = (i % 25) as f32;
                (((x - 12.5) / 3.0 + 0.5).clamp(0.0, 1.0) * 255.0) as u8
            })
            .collect();

        let (w, _, refined) = image.refine_mask(&coarse, 25, 10);
        let middle_row = &refined[20 * w as usize..21 * w as usize];
        let refined_step = f32::from(middle_row[104]) - f32::from(middle_row[96]);

        let upsampled = resize_plane_bilinear(&coarse, 25, 10, 200, 40);
        let coarse_step = (upsampled[20 * 200 + 104] - upsampled[20 * 200 + 96]) * 255.0;
        assert!(
            refined_step > coarse_step + 20.0,
            "transition across the edge should steepen: refined {refined_step}, coarse {coarse_step}"
        );
    }

    #[test]
    fn blurred_luma_fits_the_requested_size() {
        let (width, height, luma) = gray(400, 200, 0.5).blurred_luma(100);
        assert_eq!((width, height), (100, 50));
        assert_eq!(luma.len(), 100 * 50);
        assert_eq!(f16::from_bits(luma[0]).to_f32(), 0.5);
    }

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
