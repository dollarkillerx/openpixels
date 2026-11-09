use flashraw::{DecodeOptions, FlashRaw, Orientation, OutputFormat};
use half::f16;
use image::{DynamicImage, ImageDecoder, ImageReader};
use std::io::Cursor;

use crate::LinearImage;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("RAW decode failed: {0}")]
    Raw(#[from] flashraw::Error),
    #[error("image decode failed: {0}")]
    Image(#[from] image::ImageError),
    #[error("RAW decoder returned an unexpected pixel layout")]
    UnexpectedRawLayout,
}

pub struct Decoded {
    pub image: LinearImage,
    pub is_raw: bool,
    /// Camera name for RAW files.
    pub camera: Option<String>,
}

/// Decodes a RAW, JPEG or PNG file from memory into scene-linear RGB.
pub fn decode(bytes: &[u8]) -> Result<Decoded, DecodeError> {
    if flashraw::supports(bytes).is_some() {
        decode_raw(bytes)
    } else {
        decode_raster(bytes)
    }
}

fn decode_raw(bytes: &[u8]) -> Result<Decoded, DecodeError> {
    let raw = FlashRaw::open(bytes)?;
    let camera = raw.metadata().camera().to_string();
    let frame = raw.decode(DecodeOptions {
        output: OutputFormat::LinearRgbF16,
        ..Default::default()
    })?;

    let (width, height, orientation) = (frame.width(), frame.height(), frame.orientation);
    let flashraw::RawPixelData::F16(pixels) = frame.data else {
        return Err(DecodeError::UnexpectedRawLayout);
    };
    if frame.cpp != 3 {
        return Err(DecodeError::UnexpectedRawLayout);
    }

    let image = orient(LinearImage::new(width, height, pixels), orientation);
    Ok(Decoded { image, is_raw: true, camera: Some(camera) })
}

fn decode_raster(bytes: &[u8]) -> Result<Decoded, DecodeError> {
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
    let image = LinearImage::new(width, height, pixels);
    Ok(Decoded { image, is_raw: false, camera: None })
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
        assert!(decode(b"not an image").is_err());
    }
}
