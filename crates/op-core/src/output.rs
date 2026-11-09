//! Turns a finished RGB8 image into a delivered file: resizing, the watermark, and the
//! encoders other than JPEG.

use std::io::Cursor;

use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::codecs::tiff::TiffEncoder;
use image::imageops::{self, FilterType};
use image::{ExtendedColorType, ImageEncoder, ImageResult, RgbImage};
use serde::Deserialize;

use crate::srgb_icc_profile;

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
fn reduce(rgb: &[u8], width: u32, height: u32, factor: u32) -> (Vec<u8>, u32, u32) {
    let (out_w, out_h) = (width / factor, height / factor);
    let (stride, f) = (width as usize * 3, factor as usize);
    let mut out = Vec::with_capacity(out_w as usize * out_h as usize * 3);
    let samples = (f * f) as u32;
    for y in 0..out_h as usize {
        let mut totals = vec![[0u32; 3]; out_w as usize];
        for row in rgb[y * f * stride..].chunks_exact(stride).take(f) {
            for (block, total) in row.chunks_exact(f * 3).zip(totals.iter_mut()) {
                for pixel in block.chunks_exact(3) {
                    for channel in 0..3 {
                        total[channel] += u32::from(pixel[channel]);
                    }
                }
            }
        }
        out.extend(totals.iter().flat_map(|total| total.map(|value| ((value + samples / 2) / samples) as u8)));
    }
    (out, out_w, out_h)
}

/// Resizes tightly packed RGB8. Large reductions start with a block average, because a
/// filter as wide as the reduction would be slow and no sharper.
pub fn resize_rgb8(rgb: Vec<u8>, width: u32, height: u32, out_width: u32, out_height: u32) -> Vec<u8> {
    if (width, height) == (out_width, out_height) {
        return rgb;
    }
    let factor = (width / out_width).min(height / out_height) / 2;
    let (pixels, w, h) = if factor >= 2 { reduce(&rgb, width, height, factor) } else { (rgb, width, height) };
    let image = RgbImage::from_raw(w, h, pixels).expect("buffer matches its stated size");
    imageops::resize(&image, out_width, out_height, FilterType::Lanczos3).into_raw()
}

/// Draws straight-alpha RGBA over RGB8 at a position, clipped to the image.
#[allow(clippy::too_many_arguments)]
pub fn composite_overlay(rgb: &mut [u8], width: u32, height: u32, overlay: &[u8], x: i32, y: i32, overlay_width: u32, overlay_height: u32) {
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
                let blended = u32::from(source[channel]) * alpha + u32::from(target[channel]) * (255 - alpha);
                target[channel] = ((blended + 127) / 255) as u8;
            }
        }
    }
}

pub fn encode_png(rgb: &[u8], width: u32, height: u32) -> ImageResult<Vec<u8>> {
    let mut out = Vec::new();
    // Fast compression: on a large image the better settings take many times longer.
    let mut encoder = PngEncoder::new_with_quality(&mut out, CompressionType::Fast, PngFilter::Adaptive);
    encoder.set_icc_profile(srgb_icc_profile()).map_err(image::ImageError::Unsupported)?;
    encoder.write_image(rgb, width, height, ExtendedColorType::Rgb8)?;
    Ok(out)
}

pub fn encode_tiff(rgb: &[u8], width: u32, height: u32) -> ImageResult<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    TiffEncoder::new(&mut out).write_image(rgb, width, height, ExtendedColorType::Rgb8)?;
    Ok(out.into_inner())
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
            let out = resize_rgb8(rgb.clone(), 1000, 600, w, h);
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
        let png = image::load_from_memory(&encode_png(&rgb, 16, 16).unwrap()).unwrap();
        assert_eq!(png.into_rgb8().into_raw(), rgb);
        let tiff = image::load_from_memory(&encode_tiff(&rgb, 16, 16).unwrap()).unwrap();
        assert_eq!(tiff.into_rgb8().into_raw(), rgb);
    }
}
