use flashraw::{
    CfaColor, ColorMatrices, Demosaic, DecodeOptions, FlashRaw, Orientation, OutputFormat, PreviewEncoding,
    RawFrame,
};
use half::f16;
use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageDecoder, ImageReader};
use std::io::Cursor;

use crate::LinearImage;
use crate::demosaic::{BayerPattern, demosaic};
use crate::look::{DEFAULT_BASE_EXPOSURE, estimate_base_exposure};

const DEMOSAIC_TILE_SIZE: usize = 1024;
const EXPOSURE_MATCH_LONG_EDGE: u32 = 480;

/// XYZ (D65) from linear sRGB.
const XYZ_FROM_SRGB: [[f32; 3]; 3] = [
    [0.412_453, 0.357_580, 0.180_423],
    [0.212_671, 0.715_160, 0.072_169],
    [0.019_334, 0.119_193, 0.950_227],
];

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("RAW decode failed: {0}")]
    Raw(#[from] flashraw::Error),
    #[error("image decode failed: {0}")]
    Image(#[from] image::ImageError),
    #[error("RAW decoder returned an unexpected pixel layout")]
    UnexpectedRawLayout,
}

pub struct Opened {
    /// What the editor works from: a half-size image for RAW, which is plenty for a
    /// downscaled preview and avoids full demosaicing, or the whole image otherwise.
    pub working: LinearImage,
    pub is_raw: bool,
    /// Camera name for RAW files.
    pub camera: Option<String>,
    /// Size of the finished image.
    pub width: u32,
    pub height: u32,
    /// Exposure offset for the default RAW look, in EV. `None` for finished images,
    /// which already have a tone curve.
    pub base_exposure: Option<f32>,
}

/// Opens a RAW, JPEG or PNG file from memory for editing.
pub fn open(bytes: &[u8]) -> Result<Opened, DecodeError> {
    if flashraw::supports(bytes).is_some() {
        open_raw(bytes)
    } else {
        let working = decode_raster(bytes)?;
        let (width, height) = (working.width, working.height);
        Ok(Opened { working, is_raw: false, camera: None, width, height, base_exposure: None })
    }
}

fn open_raw(bytes: &[u8]) -> Result<Opened, DecodeError> {
    let raw = FlashRaw::open(bytes)?;
    let camera = raw.metadata().camera().to_string();
    let working = decode_linear(&raw, Demosaic::HalfSize)?;
    // Half-size output rounds down, so this can be one pixel short of the final size.
    let (width, height) = (working.width * 2, working.height * 2);
    let base_exposure = camera_matched_exposure(&raw, &working).unwrap_or(DEFAULT_BASE_EXPOSURE);
    Ok(Opened { working, is_raw: true, camera: Some(camera), width, height, base_exposure: Some(base_exposure) })
}

/// Exposure offset that matches the JPEG embedded in the RAW file. Uses the smallest
/// preview that is still detailed enough, since decoding a full-size one is slow.
pub fn camera_matched_exposure(raw: &FlashRaw<'_>, working: &LinearImage) -> Option<f32> {
    let preview = raw
        .previews()
        .iter()
        .filter(|p| p.encoding == PreviewEncoding::Jpeg && p.width.max(p.height) >= EXPOSURE_MATCH_LONG_EDGE)
        .min_by_key(|p| p.width.max(p.height))
        .or_else(|| raw.preview())?;
    let bytes = raw.preview_bytes(preview).ok()?;
    let mut shown = image::load_from_memory(bytes).ok()?.into_rgb8();

    let image_is_landscape = working.width > working.height;
    if (shown.width() > shown.height()) != image_is_landscape {
        shown = match raw.metadata().orientation {
            Orientation::Rotate270 => imageops::rotate270(&shown),
            _ => imageops::rotate90(&shown),
        };
    }

    let linear = working.proxy(EXPOSURE_MATCH_LONG_EDGE);
    let shown = imageops::resize(&shown, linear.width, linear.height, FilterType::Triangle);
    estimate_base_exposure(&linear, shown.as_raw())
}

/// Develops a RAW file at full resolution for export, with the highest-quality
/// demosaicing available for its sensor.
pub fn develop_raw_full(bytes: &[u8]) -> Result<LinearImage, DecodeError> {
    let raw = FlashRaw::open(bytes)?;
    let frame = raw.decode(DecodeOptions {
        output: OutputFormat::NormalizedF32,
        ..Default::default()
    })?;
    match bayer_pattern(&frame) {
        Some(pattern) => develop_bayer(frame, pattern),
        // X-Trans and other layouts use the decoder's own interpolation for now.
        None => decode_linear(&raw, Demosaic::Bilinear),
    }
}

/// Decoder-developed linear RGB, oriented for display.
pub fn decode_linear(raw: &FlashRaw<'_>, demosaic: Demosaic) -> Result<LinearImage, DecodeError> {
    let frame = raw.decode(DecodeOptions {
        output: OutputFormat::LinearRgbF16,
        demosaic,
        ..Default::default()
    })?;
    let (width, height, orientation) = (frame.width(), frame.height(), frame.orientation);
    if frame.cpp != 3 {
        return Err(DecodeError::UnexpectedRawLayout);
    }
    let flashraw::RawPixelData::F16(pixels) = frame.data else {
        return Err(DecodeError::UnexpectedRawLayout);
    };
    Ok(orient(LinearImage::new(width, height, pixels), orientation))
}

fn bayer_pattern(frame: &RawFrame) -> Option<BayerPattern> {
    if !frame.cfa?.is_bayer() || frame.cpp != 1 {
        return None;
    }
    let channel = |x: u32, y: u32| match frame.color_at(x, y) {
        Some(CfaColor::Red) => Some(0),
        Some(CfaColor::Green) => Some(1),
        Some(CfaColor::Blue) => Some(2),
        _ => None,
    };
    Some(BayerPattern([[channel(0, 0)?, channel(1, 0)?], [channel(0, 1)?, channel(1, 1)?]]))
}

fn develop_bayer(frame: RawFrame, pattern: BayerPattern) -> Result<LinearImage, DecodeError> {
    let (width, height) = (frame.width() as usize, frame.height() as usize);
    let crop = frame.crop;
    let matrix = camera_to_srgb(&frame.color_matrices);
    let orientation = frame.orientation;
    let gains = match frame.white_balance {
        Some(balance) if !frame.white_balance_applied => balance.coeffs,
        _ => [1.0; 4],
    };

    let flashraw::RawPixelData::F32(mut mosaic) = frame.data else {
        return Err(DecodeError::UnexpectedRawLayout);
    };
    // Balancing first gives the colour-difference gradients neutral greys to work on.
    for (y, row) in mosaic.chunks_exact_mut(width).enumerate() {
        let phase = pattern.0[y & 1];
        for (x, sample) in row.iter_mut().enumerate() {
            *sample *= gains[phase[x & 1] as usize];
        }
    }

    let window = (crop.x as usize, crop.y as usize, crop.width as usize, crop.height as usize);
    let rgb = demosaic(&mosaic, width, height, pattern, window, DEMOSAIC_TILE_SIZE);
    drop(mosaic);

    let mut pixels = Vec::with_capacity(rgb.len());
    for px in rgb.chunks_exact(3) {
        // Clipping here lets saturated photosites render as white, not tinted.
        let [r, g, b] = [px[0].clamp(0.0, 1.0), px[1].clamp(0.0, 1.0), px[2].clamp(0.0, 1.0)];
        for row in matrix {
            pixels.push(f16::from_f32(row[0] * r + row[1] * g + row[2] * b).to_bits());
        }
    }
    Ok(orient(LinearImage::new(crop.width, crop.height, pixels), orientation))
}

fn matrix_product(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j])
    })
}

fn matrix_inverse(m: &[[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let cofactor = |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let determinant =
        m[0][0] * cofactor(1, 1, 2, 2) - m[0][1] * cofactor(1, 0, 2, 2) + m[0][2] * cofactor(1, 0, 2, 1);
    if determinant.abs() < 1e-9 {
        return None;
    }
    let inverse = 1.0 / determinant;
    Some([
        [cofactor(1, 1, 2, 2) * inverse, cofactor(0, 2, 2, 1) * inverse, cofactor(0, 1, 1, 2) * inverse],
        [cofactor(1, 2, 2, 0) * inverse, cofactor(0, 0, 2, 2) * inverse, cofactor(0, 2, 1, 0) * inverse],
        [cofactor(1, 0, 2, 1) * inverse, cofactor(0, 1, 2, 0) * inverse, cofactor(0, 0, 1, 1) * inverse],
    ])
}

/// Camera RGB to linear sRGB, with each camera channel normalised so that a white
/// balanced neutral stays neutral.
fn camera_to_srgb(matrices: &ColorMatrices) -> [[f32; 3]; 3] {
    const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let Some(xyz_to_camera) = matrices.preferred().filter(|m| m.values.len() >= 9) else {
        return IDENTITY;
    };
    let v = &xyz_to_camera.values;
    let camera_from_xyz = [[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]];
    let mut camera_from_srgb = matrix_product(&camera_from_xyz, &XYZ_FROM_SRGB);
    for row in &mut camera_from_srgb {
        let sum: f32 = row.iter().sum();
        if sum.abs() > 1e-6 {
            row.iter_mut().for_each(|value| *value /= sum);
        }
    }
    matrix_inverse(&camera_from_srgb).unwrap_or(IDENTITY)
}

fn decode_raster(bytes: &[u8]) -> Result<LinearImage, DecodeError> {
    let mut decoder = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(image::ImageError::IoError)?
        .into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut decoded = DynamicImage::from_decoder(decoder)?;
    decoded.apply_orientation(orientation);

    let rgb = decoded.into_rgb8();
    let (width, height) = rgb.dimensions();
    let table = srgb_to_linear_table();
    let pixels = rgb.as_raw().iter().map(|&v| table[v as usize]).collect();
    Ok(LinearImage::new(width, height, pixels))
}

/// 8-bit sRGB code value to linear half-float bit pattern.
fn srgb_to_linear_table() -> [u16; 256] {
    std::array::from_fn(|i| {
        let c = i as f32 / 255.0;
        let linear = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        f16::from_f32(linear).to_bits()
    })
}

/// Applies the EXIF orientation by mapping each output pixel back to its source.
fn orient(image: LinearImage, orientation: Orientation) -> LinearImage {
    if orientation == Orientation::Normal {
        return image;
    }

    let (w, h) = (image.width as usize, image.height as usize);
    let (out_w, out_h) = if orientation.swaps_dimensions() { (h, w) } else { (w, h) };
    let mut out = Vec::with_capacity(image.pixels.len());

    for y in 0..out_h {
        for x in 0..out_w {
            let (sx, sy) = match orientation {
                Orientation::FlipH => (w - 1 - x, y),
                Orientation::Rotate180 => (w - 1 - x, h - 1 - y),
                Orientation::FlipV => (x, h - 1 - y),
                Orientation::Transpose => (y, x),
                Orientation::Rotate90 => (y, h - 1 - x),
                Orientation::Transverse => (w - 1 - y, h - 1 - x),
                Orientation::Rotate270 => (w - 1 - y, x),
                _ => (x, y),
            };
            let start = (sy * w + sx) * 3;
            out.extend_from_slice(&image.pixels[start..start + 3]);
        }
    }
    LinearImage::new(out_w as u32, out_h as u32, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2x1 image whose red channel holds the source index of each pixel.
    fn two_pixels() -> LinearImage {
        LinearImage::new(2, 1, vec![0, 0, 0, 1, 0, 0])
    }

    fn reds(image: &LinearImage) -> Vec<u16> {
        image.pixels.chunks_exact(3).map(|p| p[0]).collect()
    }

    #[test]
    fn rotate90_turns_a_row_into_a_column() {
        let out = orient(two_pixels(), Orientation::Rotate90);
        assert_eq!((out.width, out.height), (1, 2));
        assert_eq!(reds(&out), vec![0, 1]);
    }

    #[test]
    fn rotate270_is_the_inverse_direction() {
        let out = orient(two_pixels(), Orientation::Rotate270);
        assert_eq!((out.width, out.height), (1, 2));
        assert_eq!(reds(&out), vec![1, 0]);
    }

    #[test]
    fn flip_h_reverses_the_row() {
        assert_eq!(reds(&orient(two_pixels(), Orientation::FlipH)), vec![1, 0]);
    }

    #[test]
    fn srgb_table_hits_the_endpoints() {
        let table = srgb_to_linear_table();
        assert_eq!(f16::from_bits(table[0]).to_f32(), 0.0);
        assert_eq!(f16::from_bits(table[255]).to_f32(), 1.0);
    }

    #[test]
    fn unknown_bytes_are_an_error() {
        assert!(open(b"not an image").is_err());
    }

    #[test]
    fn matrix_inverse_recovers_identity() {
        let inverse = matrix_inverse(&XYZ_FROM_SRGB).unwrap();
        let identity = matrix_product(&XYZ_FROM_SRGB, &inverse);
        for (i, row) in identity.iter().enumerate() {
            for (j, value) in row.iter().enumerate() {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!((value - expected).abs() < 1e-4, "[{i}][{j}] = {value}");
            }
        }
    }
}
