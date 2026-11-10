//! Turns a finished image into a delivered file: resizing, the watermark, and the
//! encoders other than JPEG. Everything here works at 8 or 16 bits per channel.

use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::imageops::{self, FilterType};
use image::{ExtendedColorType, ImageBuffer, ImageEncoder, ImageResult, Rgb};
use serde::Deserialize;

use crate::exif::{EXIF_PREFIX, write_tiff};
use crate::srgb_icc_profile;

// The encoders reinterpret sample memory as little-endian bytes.
const _: () = assert!(cfg!(target_endian = "little"));

/// One channel of a developed, sRGB-encoded pixel.
pub trait Sample: bytemuck::Pod + Default + Send + Sync + 'static {
    const BITS: u16;
    const COLOR: ExtendedColorType;

    fn to_f32(self) -> f32;
    /// Rounds to the nearest code value.
    fn from_f32(value: f32) -> Self;
    fn to_u32(self) -> u32;
    fn from_u32(value: u32) -> Self;
    /// The same level expressed at this depth.
    fn from_u8(value: u8) -> Self;
    fn lanczos(rgb: Vec<Self>, width: u32, height: u32, out_width: u32, out_height: u32) -> Vec<Self>;
}

macro_rules! impl_sample {
    ($type:ty, $bits:literal, $color:ident, $scale:literal) => {
        impl Sample for $type {
            const BITS: u16 = $bits;
            const COLOR: ExtendedColorType = ExtendedColorType::$color;

            fn to_f32(self) -> f32 {
                f32::from(self)
            }

            fn from_f32(value: f32) -> Self {
                value.round() as Self
            }

            fn to_u32(self) -> u32 {
                u32::from(self)
            }

            fn from_u32(value: u32) -> Self {
                value as Self
            }

            fn from_u8(value: u8) -> Self {
                Self::from(value) * $scale
            }

            fn lanczos(rgb: Vec<Self>, width: u32, height: u32, out_width: u32, out_height: u32) -> Vec<Self> {
                let image = ImageBuffer::<Rgb<Self>, Vec<Self>>::from_raw(width, height, rgb).expect("buffer matches its stated size");
                imageops::resize(&image, out_width, out_height, FilterType::Lanczos3).into_raw()
            }
        }
    };
}

impl_sample!(u8, 8, Rgb8, 1);
impl_sample!(u16, 16, Rgb16, 257);

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Jpeg,
    Png,
    Tiff,
}

/// Size after limiting the long edge to `long_edge`, where 0 means no limit. Never enlarges.
pub fn fit_long_edge(width: u32, height: u32, long_edge: u32) -> (u32, u32) {
    let longest = width.max(height);
    if long_edge == 0 || longest <= long_edge {
        return (width, height);
    }
    let scale = f64::from(long_edge) / f64::from(longest);
    let shrink = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
    (shrink(width), shrink(height))
}

/// Averages blocks of `factor` pixels.
fn reduce<S: Sample>(rgb: &[S], width: u32, height: u32, factor: u32) -> (Vec<S>, u32, u32) {
    let (out_w, out_h) = (width / factor, height / factor);
    let (stride, f) = (width as usize * 3, factor as usize);
    let mut out = Vec::with_capacity(out_w as usize * out_h as usize * 3);
    let samples = (f * f) as u64;
    for y in 0..out_h as usize {
        let mut totals = vec![[0u64; 3]; out_w as usize];
        for row in rgb[y * f * stride..].chunks_exact(stride).take(f) {
            for (block, total) in row.chunks_exact(f * 3).zip(totals.iter_mut()) {
                for pixel in block.chunks_exact(3) {
                    for channel in 0..3 {
                        total[channel] += u64::from(pixel[channel].to_u32());
                    }
                }
            }
        }
        out.extend(totals.iter().flat_map(|total| total.map(|value| S::from_u32(((value + samples / 2) / samples) as u32))));
    }
    (out, out_w, out_h)
}

/// Resizes tightly packed RGB. Large reductions start with a block average, because a
/// filter as wide as the reduction would be slow and no sharper.
pub fn resize_rgb<S: Sample>(rgb: Vec<S>, width: u32, height: u32, out_width: u32, out_height: u32) -> Vec<S> {
    if (width, height) == (out_width, out_height) {
        return rgb;
    }
    let factor = (width / out_width).min(height / out_height) / 2;
    let (pixels, w, h) = if factor >= 2 { reduce(&rgb, width, height, factor) } else { (rgb, width, height) };
    S::lanczos(pixels, w, h, out_width, out_height)
}

/// Darkens or lightens the corners of the finished, framed image. Keep in step with
/// the same effect in blit.wgsl.
pub fn post_crop_vignette<S: Sample>(rgb: &mut [S], width: u32, height: u32, amount: f32) {
    if amount == 0.0 {
        return;
    }
    let full = S::from_u32(if S::BITS == 16 { 65535 } else { 255 }).to_f32();
    let (w, h) = (width as usize, height as usize);
    for (row, line) in rgb.chunks_exact_mut(w * 3).enumerate().take(h) {
        let v = ((row as f32 + 0.5) / h as f32 - 0.5) * 2.0;
        for (col, pixel) in line.chunks_exact_mut(3).enumerate() {
            let u = ((col as f32 + 0.5) / w as f32 - 0.5) * 2.0;
            let t = ((u.hypot(v) - 0.4) / 1.0).clamp(0.0, 1.0);
            let gain = 1.0 + amount * t * t * (3.0 - 2.0 * t);
            for value in pixel.iter_mut() {
                *value = S::from_f32((value.to_f32() * gain).clamp(0.0, full));
            }
        }
    }
}

/// Draws straight-alpha RGBA8 over RGB at a position, clipped to the image.
#[allow(clippy::too_many_arguments)]
pub fn composite_overlay<S: Sample>(rgb: &mut [S], width: u32, height: u32, overlay: &[u8], x: i32, y: i32, overlay_width: u32, overlay_height: u32) {
    if overlay.len() != overlay_width as usize * overlay_height as usize * 4 {
        return;
    }
    let (w, ow) = (i64::from(width), i64::from(overlay_width));
    for row in 0..i64::from(overlay_height) {
        let target_y = i64::from(y) + row;
        if target_y < 0 || target_y >= i64::from(height) {
            continue;
        }
        for col in 0..ow {
            let target_x = i64::from(x) + col;
            if target_x < 0 || target_x >= w {
                continue;
            }
            let source = &overlay[((row * ow + col) * 4) as usize..][..4];
            let alpha = u32::from(source[3]);
            let target = &mut rgb[((target_y * w + target_x) * 3) as usize..][..3];
            for channel in 0..3 {
                let blended = S::from_u8(source[channel]).to_u32() * alpha + target[channel].to_u32() * (255 - alpha);
                target[channel] = S::from_u32((blended + 127) / 255);
            }
        }
    }
}

/// `exif` is an APP1 payload from [`crate::export_exif`].
pub fn encode_png<S: Sample>(rgb: &[S], width: u32, height: u32, exif: Option<&[u8]>) -> ImageResult<Vec<u8>> {
    let mut out = Vec::new();
    // Fast compression: on a large image the better settings take many times longer.
    let mut encoder = PngEncoder::new_with_quality(&mut out, CompressionType::Fast, PngFilter::Adaptive);
    encoder.set_icc_profile(srgb_icc_profile()).map_err(image::ImageError::Unsupported)?;
    if let Some(block) = exif.and_then(|block| block.strip_prefix(EXIF_PREFIX)) {
        encoder.set_exif_metadata(block.to_vec()).map_err(image::ImageError::Unsupported)?;
    }
    encoder.write_image(bytemuck::cast_slice(rgb), width, height, S::COLOR)?;
    Ok(out)
}

/// An uncompressed TIFF. `exif` is an APP1 payload from [`crate::export_exif`].
pub fn encode_tiff<S: Sample>(rgb: &[S], width: u32, height: u32, exif: Option<&[u8]>) -> Vec<u8> {
    write_tiff(bytemuck::cast_slice(rgb), width, height, S::BITS, &srgb_icc_profile(), exif)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_long_edge_limit_keeps_the_shape_and_never_enlarges() {
        assert_eq!(fit_long_edge(6000, 4000, 3000), (3000, 2000));
        assert_eq!(fit_long_edge(4000, 6000, 3000), (2000, 3000));
        assert_eq!(fit_long_edge(1000, 500, 3000), (1000, 500));
        assert_eq!(fit_long_edge(1000, 500, 0), (1000, 500));
    }

    #[test]
    fn resizing_preserves_a_flat_colour_at_any_ratio() {
        let rgb = [10u8, 120, 240].repeat(1000 * 600);
        for (w, h) in [(500, 300), (100, 60), (37, 22)] {
            let out = resize_rgb(rgb.clone(), 1000, 600, w, h);
            assert_eq!(out.len(), w as usize * h as usize * 3);
            for pixel in out.chunks_exact(3) {
                assert!(pixel[0].abs_diff(10) <= 1 && pixel[1].abs_diff(120) <= 1 && pixel[2].abs_diff(240) <= 1, "{pixel:?}");
            }
        }
    }

    #[test]
    fn an_overlay_blends_by_alpha_and_clips_at_the_edges() {
        let mut rgb = vec![100u8; 4 * 2 * 3];
        // Opaque white, then half-transparent black.
        let overlay = [255, 255, 255, 255, 0, 0, 0, 128];
        composite_overlay(&mut rgb, 4, 2, &overlay, 2, 1, 2, 1);
        assert_eq!(&rgb[(4 + 2) * 3..(4 + 2) * 3 + 3], &[255, 255, 255]);
        assert!(rgb[(4 + 3) * 3].abs_diff(50) <= 1, "got {}", rgb[(4 + 3) * 3]);
        assert_eq!(rgb[0], 100);
        composite_overlay(&mut rgb, 4, 2, &overlay, 3, 1, 2, 1);
        composite_overlay(&mut rgb, 4, 2, &overlay, -1, -1, 2, 1);
    }

    #[test]
    fn png_and_tiff_round_trip_the_pixels() {
        let rgb: Vec<u8> = (0..3 * 16 * 16).map(|i| (i % 251) as u8).collect();
        let png = image::load_from_memory(&encode_png(&rgb, 16, 16, None).unwrap()).unwrap();
        assert_eq!(png.into_rgb8().into_raw(), rgb);
        let tiff = image::load_from_memory(&encode_tiff(&rgb, 16, 16, None)).unwrap();
        assert_eq!(tiff.into_rgb8().into_raw(), rgb);
    }

    #[test]
    fn sixteen_bit_files_keep_every_code_value() {
        // Odd values have no 8-bit equivalent, so a hidden 8-bit step would change them.
        let rgb: Vec<u16> = (0..3 * 32 * 32u32).map(|i| (i * 21 + 1) as u16).collect();
        let png = image::load_from_memory(&encode_png(&rgb, 32, 32, None).unwrap()).unwrap();
        assert_eq!(png.into_rgb16().into_raw(), rgb);
        let tiff = image::load_from_memory(&encode_tiff(&rgb, 32, 32, None)).unwrap();
        assert_eq!(tiff.into_rgb16().into_raw(), rgb);
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|window| window == needle)
    }

    #[test]
    fn delivered_files_carry_the_colour_profile_and_capture_details() {
        let exif = crate::export_exif(&crate::exif::tests::jpeg_with_exif());
        let rgb = [40u16; 3 * 8 * 8];
        let png = encode_png(&rgb, 8, 8, Some(&exif)).unwrap();
        assert!(contains(&png, b"iCCP") && contains(&png, b"eXIf") && contains(&png, b"ABCD"));

        let tiff = encode_tiff(&rgb, 8, 8, Some(&exif));
        assert!(contains(&tiff, b"ABCD"));
        assert!(contains(&tiff, &srgb_icc_profile()));
        let decoded = image::load_from_memory(&tiff).unwrap();
        assert_eq!(decoded.into_rgb16().into_raw(), rgb);
    }

    #[test]
    fn a_post_crop_vignette_darkens_corners_and_leaves_the_middle() {
        let mut rgb = vec![200u8; 3 * 101 * 101];
        post_crop_vignette(&mut rgb, 101, 101, -0.5);
        assert_eq!(rgb[(50 * 101 + 50) * 3], 200);
        assert!(rgb[0] < 130, "corner is {}", rgb[0]);
    }

    #[test]
    fn a_sixteen_bit_overlay_covers_the_full_range() {
        let mut rgb = vec![1000u16; 3];
        composite_overlay(&mut rgb, 1, 1, &[255, 255, 255, 255], 0, 0, 1, 1);
        assert_eq!(rgb, [65535; 3]);
    }

    #[test]
    fn sixteen_bit_resizing_keeps_a_level_eight_bits_cannot_hold() {
        let rgb = [33333u16; 3 * 400 * 300].to_vec();
        for (w, h) in [(200, 150), (37, 28)] {
            let out = resize_rgb(rgb.clone(), 400, 300, w, h);
            assert!(out.iter().all(|&value| value.abs_diff(33333) <= 2), "resized to {w}x{h}");
        }
    }
}
