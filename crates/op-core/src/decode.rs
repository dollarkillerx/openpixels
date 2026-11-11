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
use crate::input_profile::InputProfile;
use crate::lens::LensProfile;
use crate::demosaic::{BayerPattern, Cfa, TILE_MARGIN, demosaic_block};
use crate::look::{DEFAULT_BASE_EXPOSURE, estimate_base_exposure, fit_camera_matrix};

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
    /// Lens corrections stored in the file.
    pub lens: Option<LensProfile>,
    /// What to look up in a lens database, for RAW files that carry no corrections.
    pub lens_query: Option<LensQuery>,
    /// Colour matrix that matches the camera's own JPEG, when one helps.
    pub camera_matrix: Option<[[f32; 3]; 3]>,
}

/// Identifies a shot for looking up its lens corrections.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LensQuery {
    pub camera: String,
    pub lens: String,
    pub focal: f32,
    pub aperture: Option<f32>,
}

/// Opens a RAW, JPEG or PNG file from memory for editing.
pub fn open(bytes: &[u8]) -> Result<Opened, DecodeError> {
    if flashraw::supports(bytes).is_some() {
        open_raw(bytes)
    } else {
        let working = decode_raster(bytes)?;
        let (width, height) = (working.width, working.height);
        Ok(Opened { working, is_raw: false, camera: None, width, height, base_exposure: None, lens: None, lens_query: None, camera_matrix: None })
    }
}

/// Opens a finished image already decoded to sRGB RGBA8, as from a browser-side codec.
pub fn open_rgba8(width: u32, height: u32, rgba: &[u8]) -> Result<Opened, DecodeError> {
    open_rgba(width, height, RgbaSamples::Eight(rgba), None)
}

/// RGBA from a system codec, 8 or 16 bits per channel; 16-bit samples are native-endian.
pub enum RgbaSamples<'a> {
    Eight(&'a [u8]),
    Sixteen(&'a [u8]),
}

/// Opens a finished image decoded by another codec, in the colour space `profile`
/// describes, or sRGB without one.
pub fn open_rgba(width: u32, height: u32, samples: RgbaSamples, profile: Option<&InputProfile>) -> Result<Opened, DecodeError> {
    let count = width as usize * height as usize * 4;
    let pixels = match samples {
        RgbaSamples::Eight(rgba) => {
            if rgba.len() != count {
                return Err(DecodeError::UnexpectedRawLayout);
            }
            let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            linearise_rgb8(&rgb, profile)
        }
        RgbaSamples::Sixteen(bytes) => {
            if bytes.len() != count * 2 {
                return Err(DecodeError::UnexpectedRawLayout);
            }
            let rgb: Vec<u16> = bytes.chunks_exact(8).flat_map(|p| [0, 2, 4].map(|at| u16::from_ne_bytes([p[at], p[at + 1]]))).collect();
            linearise_rgb16(&rgb, profile)
        }
    };
    let working = LinearImage::new(width, height, pixels);
    Ok(Opened { working, is_raw: false, camera: None, width, height, base_exposure: None, lens: None, lens_query: None, camera_matrix: None })
}

fn linearise_rgb8(rgb: &[u8], profile: Option<&InputProfile>) -> Vec<u16> {
    match profile {
        Some(profile) => profile.linearise_u8(rgb),
        None => {
            let table = srgb_to_linear_table();
            rgb.iter().map(|&v| table[v as usize]).collect()
        }
    }
}

fn linearise_rgb16(rgb: &[u16], profile: Option<&InputProfile>) -> Vec<u16> {
    match profile {
        Some(profile) => profile.linearise_u16(rgb),
        None => {
            let table = srgb16_to_linear_table();
            rgb.iter().map(|&v| table[v as usize]).collect()
        }
    }
}

fn open_raw(bytes: &[u8]) -> Result<Opened, DecodeError> {
    let raw = FlashRaw::open(bytes)?;
    let camera = raw.metadata().camera().to_string();
    // Developed the way export develops, highlight reconstruction included, so the preview
    // matches the file; layouts without their own demosaicing use the decoder's half size.
    let (working, (width, height)) = match DevelopJob::start(bytes)? {
        Some(job) => (job.binned(), job.size()),
        None => {
            let working = decode_linear(&raw, Demosaic::HalfSize)?;
            // Half-size output rounds down, so this can be one pixel short of the final size.
            let size = (working.width * 2, working.height * 2);
            (working, size)
        }
    };
    let lens = LensProfile::from_file(bytes);
    let lens_query = lens.is_none().then(|| lens_query(&raw, bytes, &camera)).flatten();
    let pair = embedded_comparison(&raw, &working);
    let base_exposure = pair
        .as_ref()
        .and_then(|(linear, shown)| estimate_base_exposure(linear, shown))
        .unwrap_or(DEFAULT_BASE_EXPOSURE);
    let camera_matrix = pair.and_then(|(linear, shown)| fit_camera_matrix(&linear, &shown, base_exposure));
    Ok(Opened {
        working,
        is_raw: true,
        camera: Some(camera),
        width,
        height,
        base_exposure: Some(base_exposure),
        lens,
        lens_query,
        camera_matrix,
    })
}

fn lens_query(raw: &FlashRaw<'_>, bytes: &[u8], camera: &str) -> Option<LensQuery> {
    let exposure = &raw.metadata().exposure;
    Some(LensQuery {
        camera: camera.to_string(),
        lens: crate::exif::lens_model(bytes)?,
        focal: exposure.focal_length.filter(|focal| *focal > 0.0)?,
        aperture: exposure.f_number.filter(|aperture| *aperture > 0.0),
    })
}

/// Exposure offset that matches the JPEG embedded in the RAW file. Uses the smallest
/// preview that is still detailed enough, since decoding a full-size one is slow.
pub fn camera_matched_exposure(raw: &FlashRaw<'_>, working: &LinearImage) -> Option<f32> {
    let (linear, shown) = embedded_comparison(raw, working)?;
    estimate_base_exposure(&linear, &shown)
}

/// The linear image and the camera's embedded JPEG at one small size, tightly packed RGB8.
fn embedded_comparison(raw: &FlashRaw<'_>, working: &LinearImage) -> Option<(LinearImage, Vec<u8>)> {
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
    Some((linear, shown.into_raw()))
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
    /// Where each white-balanced camera channel clips: its white balance gain.
    #[serde(default = "unit_clip")]
    pub clip: [f32; 3],
}

fn unit_clip() -> [f32; 3] {
    [1.0; 3]
}

/// How far below its clip level a channel starts being treated as clipped, so the
/// reconstruction fades in instead of leaving an edge.
const RECONSTRUCT_FROM: f32 = 0.9;

/// Highlight reconstruction for white-balanced camera RGB. Where one or two channels hit
/// the sensor's limit, the true value was at least that much; each clipped channel is
/// raised towards what the unclipped ones suggest for a near-neutral highlight, which keeps
/// detail and colour in skies and skin that would otherwise go flat or tinted. Where all
/// three clipped nothing is known, and the pixel becomes a neutral as bright as its
/// brightest channel, rather than the pink that unequal clip levels would give.
pub fn reconstruct_highlights(rgb: [f32; 3], clip: [f32; 3]) -> [f32; 3] {
    let v: [f32; 3] = std::array::from_fn(|c| rgb[c].clamp(0.0, clip[c]));
    let weight: [f32; 3] = std::array::from_fn(|c| {
        let t = ((v[c] / clip[c] - RECONSTRUCT_FROM) / (1.0 - RECONSTRUCT_FROM)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    });
    let peak = v[0].max(v[1]).max(v[2]);
    let mut out = v;
    for c in 0..3 {
        if weight[c] == 0.0 {
            continue;
        }
        // The other channels, each counting as much as it is unclipped.
        let (mut total, mut trust) = (0.0, 0.0);
        for other in (0..3).filter(|&other| other != c) {
            let unclipped = 1.0 - weight[other];
            total += v[other] * unclipped;
            trust += unclipped;
        }
        let estimate = if trust > 1e-3 { total / trust } else { peak };
        // Blends from what was measured to the estimate, and never lowers the measurement.
        let all_clipped = 1.0 - trust / 2.0;
        let target = estimate * (1.0 - all_clipped) + peak * all_clipped;
        out[c] = v[c] + weight[c] * (target - v[c]).max(0.0);
    }
    out
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

        // Made when the first tile is stored, so a job used only for the preview never holds it.
        let output = Vec::new();
        Ok(Some(Self {
            recipe: TileRecipe { cfa, matrix, clip: [gains[0], gains[1], gains[2]] },
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
        if self.output.is_empty() {
            self.output = vec![0u16; crop_width * self.crop.height as usize * 3];
        }
        for row in 0..tile.height {
            let source = row * tile.width * 3;
            let target = ((tile.y - self.crop.y as usize + row) * crop_width + tile.x - self.crop.x as usize) * 3;
            self.output[target..target + tile.width * 3].copy_from_slice(&rgb[source..source + tile.width * 3]);
        }
    }

    /// The finished image's size, after orientation.
    pub fn size(&self) -> (u32, u32) {
        let (width, height) = (self.crop.width, self.crop.height);
        if matches!(self.orientation, Orientation::Rotate90 | Orientation::Rotate270 | Orientation::Transpose | Orientation::Transverse) {
            (height, width)
        } else {
            (width, height)
        }
    }

    /// A quick, smaller development for the editor's preview: each block of photosites (2 x 2
    /// for Bayer, 3 x 3 for X-Trans) becomes one pixel of their average per colour, with the
    /// same highlight reconstruction and colour matrix as the full development.
    pub fn binned(&self) -> LinearImage {
        let factor = match self.recipe.cfa {
            Cfa::Bayer(_) => 2,
            Cfa::XTrans(_) => 3,
        };
        let channel = |x: usize, y: usize| match &self.recipe.cfa {
            Cfa::Bayer(pattern) => pattern.0[y & 1][x & 1] as usize,
            Cfa::XTrans(pattern) => pattern[y % 6][x % 6] as usize,
        };
        // Blocks line up with the pattern, so every one holds each colour.
        let (x0, y0) = ((self.crop.x as usize).div_ceil(factor) * factor, (self.crop.y as usize).div_ceil(factor) * factor);
        let (x1, y1) = ((self.crop.x + self.crop.width) as usize / factor * factor, (self.crop.y + self.crop.height) as usize / factor * factor);
        let (width, height) = ((x1.saturating_sub(x0)) / factor, (y1.saturating_sub(y0)) / factor);
        let mut pixels = Vec::with_capacity(width * height * 3);
        for by in 0..height {
            for bx in 0..width {
                let (mut sums, mut counts) = ([0.0f32; 3], [0u32; 3]);
                for y in y0 + by * factor..y0 + (by + 1) * factor {
                    let start = x0 + bx * factor;
                    let row = &self.mosaic[y * self.sensor_width + start..][..factor];
                    for (x, &value) in (start..).zip(row) {
                        let c = channel(x, y).min(2);
                        sums[c] += value;
                        counts[c] += 1;
                    }
                }
                let rgb: [f32; 3] = std::array::from_fn(|c| sums[c] / counts[c].max(1) as f32);
                let [r, g, b] = reconstruct_highlights(rgb, self.recipe.clip);
                for matrix_row in self.recipe.matrix {
                    pixels.push(f16::from_f32(matrix_row[0] * r + matrix_row[1] * g + matrix_row[2] * b).to_bits());
                }
            }
        }
        orient(LinearImage::new(width as u32, height as u32, pixels), self.orientation)
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
            let [r, g, b] = reconstruct_highlights([red[row + x], green[row + x], blue[row + x]], recipe.clip);
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
    // A tagged Display P3 or Adobe RGB file would look dull if read as sRGB.
    let profile = decoder.icc_profile().ok().flatten().and_then(|icc| InputProfile::from_icc(&icc));
    let mut decoded = DynamicImage::from_decoder(decoder)?;
    decoded.apply_orientation(orientation);

    // 16-bit files keep their precision, which matters once tones are pushed around.
    if decoded.color().bytes_per_pixel() / decoded.color().channel_count() == 2 {
        let rgb = decoded.into_rgb16();
        let (width, height) = rgb.dimensions();
        return Ok(LinearImage::new(width, height, linearise_rgb16(rgb.as_raw(), profile.as_ref())));
    }

    let rgb = decoded.into_rgb8();
    let (width, height) = rgb.dimensions();
    Ok(LinearImage::new(width, height, linearise_rgb8(rgb.as_raw(), profile.as_ref())))
}

/// 8-bit sRGB code value to linear half-float bit pattern.
pub(crate) fn srgb_to_linear_table() -> [u16; 256] {
    std::array::from_fn(|i| {
        let c = i as f32 / 255.0;
        let linear = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        f16::from_f32(linear).to_bits()
    })
}

/// 16-bit sRGB code value to linear half-float bit pattern.
fn srgb16_to_linear_table() -> Vec<u16> {
    (0..=u16::MAX)
        .map(|code| {
            let c = f32::from(code) / 65535.0;
            let linear = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
            f16::from_f32(linear).to_bits()
        })
        .collect()
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

    #[test]
    fn sixteen_bit_files_keep_values_between_eight_bit_steps() {
        // One 8-bit step is 257 here, so these two would be the same in 8 bits.
        let raw: Vec<u16> = [30000u16, 30000, 30000, 30100, 30100, 30100].to_vec();
        let image = image::ImageBuffer::<image::Rgb<u16>, _>::from_raw(2, 1, raw).unwrap();
        for format in [image::ImageFormat::Png, image::ImageFormat::Tiff] {
            let mut file = Cursor::new(Vec::new());
            image.write_to(&mut file, format).unwrap();
            let decoded = open(file.get_ref()).unwrap().working;
            assert_ne!(decoded.pixels[0], decoded.pixels[3], "{format:?}");
        }
    }

    use super::*;

    #[test]
    fn a_tagged_display_p3_file_keeps_its_wider_red() {
        use image::ImageEncoder;
        let mut file = Vec::new();
        let mut encoder = image::codecs::png::PngEncoder::new(&mut file);
        encoder.set_icc_profile(crate::OutputSpace::P3.icc_profile()).unwrap();
        encoder.write_image(&[255, 0, 0, 255, 255, 255], 2, 1, image::ExtendedColorType::Rgb8).unwrap();
        let decoded = open(&file).unwrap().working;
        let linear: Vec<f32> = decoded.pixels.iter().map(|&bits| f16::from_bits(bits).to_f32()).collect();
        assert!(linear[0] > 1.15 && linear[1] < -0.02, "P3 red read as {:?}", &linear[..3]);
        assert!(linear[3..].iter().all(|v| (v - 1.0).abs() < 2e-3), "white {:?}", &linear[3..]);
        // Untagged, the same codes are sRGB red.
        let mut plain = Cursor::new(Vec::new());
        image::RgbImage::from_raw(1, 1, vec![255, 0, 0]).unwrap().write_to(&mut plain, image::ImageFormat::Png).unwrap();
        let red = open(plain.get_ref()).unwrap().working.pixels;
        assert_eq!(red.iter().map(|&b| f16::from_bits(b).to_f32()).collect::<Vec<_>>(), [1.0, 0.0, 0.0]);
    }

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
    fn rgba_input_must_match_its_declared_size() {
        assert!(open_rgba8(2, 2, &[0; 15]).is_err());
        let opened = open_rgba8(1, 1, &[255, 0, 0, 255]).unwrap();
        assert_eq!((opened.width, opened.is_raw), (1, false));
        assert_eq!(f16::from_bits(opened.working.pixels[0]).to_f32(), 1.0);
    }

    #[test]
    fn unknown_bytes_are_an_error() {
        assert!(open(b"not an image").is_err());
    }

    #[test]
    fn highlight_reconstruction_fills_in_clipped_channels_and_leaves_the_rest() {
        let clip = [2.0, 1.0, 1.6];
        // Nowhere near clipping: untouched.
        assert_eq!(reconstruct_highlights([0.4, 0.3, 0.2], clip), [0.4, 0.3, 0.2]);
        // Green clipped in a bright blue sky: raised towards red and blue, never lowered.
        let sky = reconstruct_highlights([0.8, 1.0, 1.5], clip);
        assert!(sky[1] >= 1.0 && sky[0] == 0.8, "{sky:?}");
        // Green clipped with red and blue above it: the highlight rises towards neutral.
        let bright = reconstruct_highlights([1.4, 1.0, 1.3], clip);
        assert!(bright[1] > 1.2, "{bright:?}");
        // Everything clipped: a neutral, not the pink of unequal clip levels.
        let white = reconstruct_highlights([2.0, 1.0, 1.6], clip);
        assert!((white[0] - white[1]).abs() < 1e-4 && (white[1] - white[2]).abs() < 1e-4, "{white:?}");
        // Values past the clip level are noise from interpolation and are held to it.
        assert_eq!(reconstruct_highlights([0.1, 0.1, 9.0], clip)[2], 1.6);
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
