use flashraw::{
    CfaColor, ColorMatrices, Demosaic, DecodeOptions, FlashRaw, Orientation, OutputFormat, PreviewEncoding,
    RawFrame,
};
use half::f16;
use serde::{Deserialize, Serialize};
use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageDecoder, ImageReader};
use std::io::Cursor;

use crate::LinearImage;
use crate::demosaic::{BayerPattern, Cfa, TILE_MARGIN, demosaic_block};
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
    let Some(mut job) = DevelopJob::start(bytes)? else {
        // Unusual layouts use the decoder's own interpolation.
        return decode_linear(&FlashRaw::open(bytes)?, Demosaic::Bilinear);
    };
    for tile in job.tiles() {
        let input = job.tile_input(&tile);
        let rgb = process_tile(&input, &tile, &job.recipe);
        job.store_tile(&tile, &rgb);
    }
    Ok(job.finish())
}

/// What a worker needs to develop any tile of a job.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TileRecipe {
    pub cfa: Cfa,
    pub matrix: [[f32; 3]; 3],
}

/// One piece of a job, with the larger block of sensor data it needs as context.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevelopTile {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub block_x: usize,
    pub block_y: usize,
    pub block_width: usize,
    pub block_height: usize,
}

/// A full-resolution RAW development, split so its tiles can run anywhere.
pub struct DevelopJob {
    pub recipe: TileRecipe,
    mosaic: Vec<f32>,
    sensor_width: usize,
    sensor_height: usize,
    crop: flashraw::Rect,
    orientation: Orientation,
    output: Vec<u16>,
}

impl DevelopJob {
    /// `None` if the sensor layout has no dedicated demosaicing.
    pub fn start(bytes: &[u8]) -> Result<Option<Self>, DecodeError> {
        let raw = FlashRaw::open(bytes)?;
        let frame = raw.decode(DecodeOptions { output: OutputFormat::NormalizedF32, ..Default::default() })?;
        let Some(cfa) = cfa_of(&frame) else { return Ok(None) };

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
            for (x, sample) in row.iter_mut().enumerate() {
                let channel = match &cfa {
                    Cfa::Bayer(pattern) => pattern.0[y & 1][x & 1],
                    Cfa::XTrans(pattern) => pattern[y % 6][x % 6],
                };
                *sample *= gains[channel as usize];
            }
        }

        let output = vec![0u16; crop.width as usize * crop.height as usize * 3];
        Ok(Some(Self {
            recipe: TileRecipe { cfa, matrix },
            mosaic,
            sensor_width: width,
            sensor_height: height,
            crop,
            orientation,
            output,
        }))
    }

    pub fn tiles(&self) -> Vec<DevelopTile> {
        let (crop_x, crop_y) = (self.crop.x as usize, self.crop.y as usize);
        let (crop_w, crop_h) = (self.crop.width as usize, self.crop.height as usize);
        let mut tiles = Vec::new();
        for y in (crop_y..crop_y + crop_h).step_by(DEMOSAIC_TILE_SIZE) {
            for x in (crop_x..crop_x + crop_w).step_by(DEMOSAIC_TILE_SIZE) {
                let width = DEMOSAIC_TILE_SIZE.min(crop_x + crop_w - x);
                let height = DEMOSAIC_TILE_SIZE.min(crop_y + crop_h - y);
                let (block_x, block_y) = (x.saturating_sub(TILE_MARGIN), y.saturating_sub(TILE_MARGIN));
                let block_width = (x + width + TILE_MARGIN).min(self.sensor_width) - block_x;
                let block_height = (y + height + TILE_MARGIN).min(self.sensor_height) - block_y;
                tiles.push(DevelopTile { x, y, width, height, block_x, block_y, block_width, block_height });
            }
        }
        tiles
    }

    /// The sensor samples a tile's block needs, tightly packed.
    pub fn tile_input(&self, tile: &DevelopTile) -> Vec<f32> {
        let mut input = Vec::with_capacity(tile.block_width * tile.block_height);
        for row in 0..tile.block_height {
            let start = (tile.block_y + row) * self.sensor_width + tile.block_x;
            input.extend_from_slice(&self.mosaic[start..start + tile.block_width]);
        }
        input
    }

    /// Stores a finished tile: interleaved RGB half-float bits, `width * height * 3` long.
    pub fn store_tile(&mut self, tile: &DevelopTile, rgb: &[u16]) {
        let crop_width = self.crop.width as usize;
        for row in 0..tile.height {
            let source = row * tile.width * 3;
            let target = ((tile.y - self.crop.y as usize + row) * crop_width + tile.x - self.crop.x as usize) * 3;
            self.output[target..target + tile.width * 3].copy_from_slice(&rgb[source..source + tile.width * 3]);
        }
    }

    pub fn finish(self) -> LinearImage {
        orient(LinearImage::new(self.crop.width, self.crop.height, self.output), self.orientation)
    }
}

/// Develops one tile from its block of sensor samples. Runs without the job, so it can
/// be sent to another thread.
pub fn process_tile(input: &[f32], tile: &DevelopTile, recipe: &TileRecipe) -> Vec<u16> {
    let (width, height) = (tile.block_width, tile.block_height);
    let [red, green, blue] = demosaic_block(input, width, height, &recipe.cfa, (tile.block_x, tile.block_y));
    let mut out = Vec::with_capacity(tile.width * tile.height * 3);
    for y in 0..tile.height {
        let row = (tile.y - tile.block_y + y) * width + tile.x - tile.block_x;
        for x in 0..tile.width {
            // Clipping here lets saturated photosites render as white, not tinted.
            let [r, g, b] = [red[row + x], green[row + x], blue[row + x]].map(|v| v.clamp(0.0, 1.0));
            for matrix_row in recipe.matrix {
                out.push(f16::from_f32(matrix_row[0] * r + matrix_row[1] * g + matrix_row[2] * b).to_bits());
            }
        }
    }
    out
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

fn cfa_of(frame: &RawFrame) -> Option<Cfa> {
    let pattern = frame.cfa?;
    if frame.cpp != 1 {
        return None;
    }
    let channel = |x: u32, y: u32| match frame.color_at(x, y) {
        Some(CfaColor::Red) => Some(0),
        Some(CfaColor::Green) => Some(1),
        Some(CfaColor::Blue) => Some(2),
        _ => None,
    };
    if pattern.is_bayer() {
        return Some(Cfa::Bayer(BayerPattern([[channel(0, 0)?, channel(1, 0)?], [channel(0, 1)?, channel(1, 1)?]])));
    }
    if pattern.width() == 6 && pattern.height() == 6 {
        let mut cells = [[0u8; 6]; 6];
        for (y, row) in cells.iter_mut().enumerate() {
            for (x, cell) in row.iter_mut().enumerate() {
                *cell = channel(x as u32, y as u32)?;
            }
        }
        return Some(Cfa::XTrans(cells));
    }
    None
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
