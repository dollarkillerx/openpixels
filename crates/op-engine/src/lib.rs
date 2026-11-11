//! The editing engine shared by the browser and desktop front ends: one open photo,
//! its edits, the preview and the export. Data crosses the boundary as JSON strings
//! and byte buffers, so each front end only adds a thin binding.
//!
//! Methods take `&mut self`; a front end must not start one while an async one is
//! still pending.

use op_core::{
    Adjustments, AutoTone, DevelopJob, ErasePatch, LensfunCalibration, Geometry, HealSpot, LinearImage, TileRecipe, ToneCurve, WarpField, WarpStroke, apply_erases, apply_heals,
    OutputFormat, Sample, ToneStats, auto_exposure, auto_finish, trim_exposure, composite_overlay, post_crop_vignette, encode_png, encode_tiff, export_exif, find_heal_source, fit_long_edge,
    neutral_white_balance, resize_rgb, IccProfile, Intent, OutputSpace, ProofOptions, apply_proof_lut, mark_uncalibrated, proof_lut,
};
use op_gpu::{Renderer, ViewTransform, encode_jpeg};
use serde::{Deserialize, Serialize};

/// Grid points per side of the soft proof table.
const PROOF_LUT_SIZE: u32 = 33;
/// Long edge of the renders that automatic tone measures.
const AUTO_TONE_LONG_EDGE: u32 = 512;
const EXPORT_TILE_SIZE: u32 = 2048;
/// The largest side of the full-resolution layer. Beyond this the proxy is detailed enough.
const MAX_DETAIL_SIDE: f32 = 4096.0;
/// Extra area developed around the view, so short pans do not need a new region.
const DETAIL_PADDING: f32 = 0.2;
const DETAIL_SAMPLES: usize = 8;

/// Why an engine call failed, as a message for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError(pub String);

impl EngineError {
    pub fn new(message: &str) -> Self {
        Self(message.to_string())
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EngineError {}

pub type Result<T, E = EngineError> = std::result::Result<T, E>;

fn to_error(error: impl std::fmt::Display) -> EngineError {
    EngineError(error.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageInfo {
    width: u32,
    height: u32,
    is_raw: bool,
    camera: Option<String>,
    base_exposure: Option<f32>,
    lens: Option<op_core::LensProfile>,
    lens_query: Option<op_core::LensQuery>,
    capture_time: Option<String>,
    camera_match: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyInfo {
    width: u32,
    height: u32,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ViewFlags {
    /// Non-zero while the crop is being edited, which shows the whole frame.
    crop_editing: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutputSettings {
    format: OutputFormat,
    quality: u8,
    long_edge: u32,
    #[serde(default)]
    bit_depth: u32,
    #[serde(default)]
    color_space: OutputSpace,
}

/// A watermark raster and where it goes on the finished image.
struct Overlay<'a> {
    rgba: &'a [u8],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Serialize)]
struct WhiteBalance {
    temperature: f32,
    tint: f32,
}

pub struct Engine {
    renderer: Renderer,
    /// The size of the surface the preview is drawn on.
    target_size: (u32, u32),
    /// The decoded image. For RAW this is only the half-size one, dropped once the
    /// proxy is built, because export re-develops from `raw_file` at full quality.
    working: Option<LinearImage>,
    raw_file: Option<Vec<u8>>,
    proxy: Option<LinearImage>,
    adjustments: Adjustments,
    geometry: Geometry,
    crop_editing: bool,
    heals: Vec<HealSpot>,
    erases: Vec<ErasePatch>,
    /// The full image with heals and erases applied, kept while viewing up close.
    full_view: Option<LinearImage>,
    healed_proxy: Option<LinearImage>,
    /// The soft proof table, so the histogram can show the proof too.
    proof: Option<Vec<f32>>,
    warp: Option<WarpField>,
    curve: ToneCurve,
    source_exif: Option<Vec<u8>>,
    job: Option<DevelopJob>,
    job_tiles: Vec<op_core::decode::DevelopTile>,
    developed: Option<LinearImage>,
    /// Screen fraction showing the unedited image; 0 is off.
    compare: f32,
    full_size: (u32, u32),
}

/// Develops one tile, on any thread. See `DevelopJob`.
pub fn process_develop_tile(input: &[f32], tile_json: &str, recipe_json: &str) -> Result<Vec<u16>> {
    let tile = serde_json::from_str(tile_json).map_err(to_error)?;
    let recipe: TileRecipe = serde_json::from_str(recipe_json).map_err(to_error)?;
    Ok(op_core::process_tile(input, &tile, &recipe))
}

impl Engine {
    /// An engine drawing with `renderer`, whose target format is the front end's surface.
    pub fn new(renderer: Renderer) -> Engine {
        Engine {
            renderer,
            target_size: (1, 1),
            working: None,
            raw_file: None,
            proxy: None,
            adjustments: Adjustments::default(),
            geometry: Geometry::default(),
            crop_editing: false,
            heals: Vec::new(),
            erases: Vec::new(),
            full_view: None,
            healed_proxy: None,
            warp: None,
            curve: ToneCurve::default(),
            proof: None,
            source_exif: None,
            job: None,
            job_tiles: Vec::new(),
            developed: None,
            compare: 0.0,
            full_size: (0, 0),
        }
    }

    pub fn renderer(&self) -> &Renderer {
        &self.renderer
    }

    /// Opens a file for editing. Returns JSON metadata.
    pub fn decode(&mut self, bytes: &[u8]) -> Result<String> {
        self.release_image();
        let opened = op_core::open(bytes).map_err(to_error)?;
        self.source_exif = Some(export_exif(bytes));
        self.finish_open(opened, Some(bytes))
    }

    /// Opens an image a browser-side codec already decoded. Returns JSON metadata.
    /// `source` is the original file, for its capture details.
    pub fn decode_rgba(&mut self, rgba: &[u8], width: u32, height: u32, source: &[u8]) -> Result<String> {
        self.decode_rgba_samples(op_core::RgbaSamples::Eight(rgba), width, height, None, source)
    }

    /// Like [`Self::decode_rgba`], for 8- or 16-bit samples in the space `profile` describes.
    pub fn decode_rgba_samples(
        &mut self,
        samples: op_core::RgbaSamples,
        width: u32,
        height: u32,
        profile: Option<&op_core::InputProfile>,
        source: &[u8],
    ) -> Result<String> {
        self.release_image();
        let opened = op_core::open_rgba(width, height, samples, profile).map_err(to_error)?;
        self.source_exif = Some(export_exif(source));
        self.finish_open(opened, None)
    }

    /// Builds and uploads the interactive proxy. Returns JSON with its size.
    pub fn build_proxy(&mut self, max_long_edge: u32) -> Result<String> {
        let image = self.working.as_ref().ok_or_else(|| EngineError::new("no image loaded"))?;
        let proxy = image.proxy(max_long_edge);
        self.renderer.set_preview(&proxy);
        self.warp = Some(WarpField::new(proxy.width, proxy.height));
        let info = ProxyInfo { width: proxy.width, height: proxy.height };
        self.proxy = Some(proxy);
        if self.raw_file.is_some() {
            self.working = None;
        }
        serde_json::to_string(&info).map_err(to_error)
    }

    /// Returns the proxy as an NCHW float tensor for the segmentation model.
    pub fn model_input(
        &self,
        width: u32,
        height: u32,
        mean: &[f32],
        std: &[f32],
    ) -> Result<Vec<f32>> {
        let proxy = self.proxy.as_ref().ok_or_else(|| EngineError::new("no image loaded"))?;
        let channels = |values: &[f32]| {
            <[f32; 3]>::try_from(values).map_err(|_| EngineError::new("expected 3 channel values"))
        };
        Ok(proxy.srgb_nchw_tensor(width, height, channels(mean)?, channels(std)?))
    }

    /// Follows a model's coarse mask to the proxy's own edges. Returns one byte per pixel.
    pub fn refine_model_mask(&self, width: u32, height: u32, mask: &[u8]) -> Result<Vec<u8>> {
        let proxy = self.proxy.as_ref().ok_or_else(|| EngineError::new("no image loaded"))?;
        Ok(proxy.refine_mask(mask, width, height).2)
    }

    /// Sets an image that replaces the sky or else the background, straight sRGB RGBA8.
    pub fn set_replace_image(&mut self, sky: bool, width: u32, height: u32, rgba: &[u8]) {
        if sky {
            self.renderer.set_sky_image(width, height, rgba);
        } else {
            self.renderer.set_background_image(width, height, rgba);
        }
    }

    /// The liquify field as interleaved offsets, or nothing if the image is not warped.
    pub fn warp_field(&self) -> Vec<f32> {
        self.warp.as_ref().filter(|field| !field.is_identity()).map_or_else(Vec::new, |field| field.offsets().to_vec())
    }

    /// Width and height of [`Self::warp_field`], as JSON.
    pub fn warp_field_size(&self) -> Result<String> {
        let (width, height) = self.warp.as_ref().map_or((0, 0), |field| (field.width, field.height));
        serde_json::to_string(&ProxyInfo { width, height }).map_err(to_error)
    }

    /// Whether the open image is ready for full-size reading.
    pub fn has_full_image(&self) -> bool {
        self.developed.is_some() || self.raw_file.is_none()
    }

    /// Sets where the sky is, `width * height` bytes covering the whole image.
    pub fn set_sky_mask(&mut self, width: u32, height: u32, mask: &[u8]) {
        match &self.proxy {
            Some(proxy) => {
                let (refined_w, refined_h, refined) = proxy.refine_mask(mask, width, height);
                self.renderer.set_sky_mask(refined_w, refined_h, &refined);
            }
            None => self.renderer.set_sky_mask(width, height, mask),
        }
    }

    /// Applies corrections from the Lensfun database, given as JSON, and returns the
    /// resulting lens profile as JSON.
    pub fn set_lens_calibration(&mut self, json: &str) -> Result<String> {
        let calibration: LensfunCalibration = serde_json::from_str(json).map_err(to_error)?;
        let profile = op_core::LensProfile::from_lensfun(&calibration);
        self.renderer.set_lens_profile(profile);
        serde_json::to_string(&profile).map_err(to_error)
    }

    /// Sets the subject mask, `width * height` bytes covering the whole image.
    pub fn set_mask(&mut self, width: u32, height: u32, mask: &[u8]) {
        match &self.proxy {
            Some(proxy) => {
                let (refined_w, refined_h, refined) = proxy.refine_mask(mask, width, height);
                self.renderer.set_mask(refined_w, refined_h, &refined);
            }
            None => self.renderer.set_mask(width, height, mask),
        }
    }

    /// The size of the surface the preview is drawn on, which decides the close-up layer.
    pub fn set_target_size(&mut self, width: u32, height: u32) {
        self.target_size = (width.max(1), height.max(1));
    }

    pub fn set_adjustments(&mut self, json: &str) -> Result<()> {
        // Everything is read before anything changes, so a bad edit leaves the last good one.
        let mut adjustments: Adjustments = serde_json::from_str(json).map_err(to_error)?;
        let curve: ToneCurve = serde_json::from_str(json).map_err(to_error)?;
        let geometry: Geometry = serde_json::from_str(json).map_err(to_error)?;
        let flags: ViewFlags = serde_json::from_str(json).map_err(to_error)?;
        if curve != self.curve {
            self.renderer.set_curve(&curve);
            self.curve = curve;
        }
        adjustments.curve_active = f32::from(u8::from(!self.curve.is_identity()));
        self.adjustments = adjustments;
        self.geometry = geometry;
        self.crop_editing = flags.crop_editing != 0.0;
        Ok(())
    }

    /// Draws the preview into `view`, a texture of the target size and format. The
    /// front end presents or reads it back.
    pub fn draw(&mut self, view: &wgpu::TextureView) -> Result<()> {
        let shown = if self.crop_editing { self.geometry.whole_frame() } else { self.geometry };
        if self.compare > 0.0
            && !self.renderer.comparison_ready()
            && let Some(proxy) = &self.proxy
        {
            self.renderer.prepare_comparison(proxy, self.healed_proxy.as_ref());
        }
        self.renderer
            .draw_preview(&self.adjustments, &shown, view, self.target_size, self.compare)
            .map_err(to_error)
    }

    /// Builds skin corrections from a facial skin mask at the proxy's size.
    pub fn set_skin_mask(&mut self, mask: &[u8], face_width: f32) -> Result<()> {
        let proxy = self.proxy.as_ref().ok_or_else(|| EngineError::new("no image loaded"))?;
        let retouch = op_core::build_skin_retouch(proxy, mask, face_width);
        self.renderer.set_skin_retouch(&retouch);
        Ok(())
    }

    /// Pushes the image from one stage position to another, both in 0..=1. Returns the
    /// stroke as JSON, or `null` if either position missed the image.
    #[allow(clippy::too_many_arguments)]
    pub fn liquify(
        &mut self,
        from_x: f32,
        from_y: f32,
        to_x: f32,
        to_y: f32,
        stage_width: u32,
        stage_height: u32,
        radius: f32,
        strength: f32,
        mode: u8,
    ) -> Result<String> {
        let size = (stage_width.max(1), stage_height.max(1));
        let from = self.renderer.canvas_to_image(&self.geometry, size, from_x, from_y);
        let to = self.renderer.canvas_to_image(&self.geometry, size, to_x, to_y);
        let (Some(from), Some(to), Some(field)) = (from, to, self.warp.as_mut()) else {
            return Ok("null".into());
        };
        let stroke =
            WarpStroke { x: from.0, y: from.1, dx: to.0 - from.0, dy: to.1 - from.1, radius, strength, mode };
        field.apply(&stroke);
        self.renderer.update_warp(field);
        serde_json::to_string(&stroke).map_err(to_error)
    }

    /// Rebuilds the liquify field from a list of strokes, given as JSON.
    pub fn set_warp_strokes(&mut self, json: &str) -> Result<()> {
        let strokes: Vec<WarpStroke> = serde_json::from_str(json).map_err(to_error)?;
        let proxy = self.proxy.as_ref().ok_or_else(|| EngineError::new("no image loaded"))?;
        let mut field = WarpField::new(proxy.width, proxy.height);
        for stroke in &strokes {
            field.apply(stroke);
        }
        self.renderer.update_warp(&field);
        self.warp = Some(field);
        Ok(())
    }

    /// Sets the creative look: `size`^3 RGB triples, or a size of 0 for none.
    pub fn set_look(&mut self, size: u32, rgb: &[f32]) {
        self.renderer.set_look((size > 0).then_some((size, rgb)));
    }

    /// Sets a film simulation in place of the look: a negative LUT and an output LUT, each
    /// `size`^3 RGB triples with red varying fastest, and the stock's grain as 16 RGB rows
    /// (or nothing). Returns JSON with the exposure that keeps mid grey as bright.
    #[allow(clippy::too_many_arguments)]
    pub fn set_film(&mut self, negative_size: u32, negative: &[f32], output_size: u32, output: &[f32], slide: bool, mono: bool, grain: &[f32]) -> Result<String> {
        let cube = |size: u32, data: &[f32]| {
            (size >= 2 && data.len() == (size as usize).pow(3) * 3).then(|| op_core::FilmLut { size, data: data.to_vec() }).ok_or_else(|| EngineError::new("film LUT has the wrong size"))
        };
        let film = op_core::Film { negative: cube(negative_size, negative)?, output: cube(output_size, output)?, slide, mono };
        let balance = film.balance();
        let mut rows = [[0.0; 3]; op_core::film::GRAIN_SAMPLES];
        if grain.len() == rows.len() * 3 {
            for (row, values) in rows.iter_mut().zip(grain.chunks_exact(3)) {
                *row = [values[0], values[1], values[2]];
            }
        }
        self.renderer.set_film((negative_size, negative), (output_size, output), op_gpu::FilmSettings { slide, mono, balance, grain: rows });
        Ok(serde_json::json!({ "balance": balance }).to_string())
    }

    /// Shows the preview as it would come out through an ICC profile. Returns JSON
    /// describing the profile. Exports are unaffected.
    pub fn set_proof(&mut self, icc: &[u8], perceptual: bool, simulate_paper: bool, gamut_warning: bool) -> Result<String> {
        let profile = IccProfile::parse(icc).map_err(to_error)?;
        let intent = if perceptual { Intent::Perceptual } else { Intent::Relative };
        let lut = proof_lut(&profile, ProofOptions { intent, simulate_paper }, PROOF_LUT_SIZE);
        self.renderer.set_proof(Some((PROOF_LUT_SIZE, &lut)), gamut_warning);
        self.proof = Some(lut);
        describe(&profile)
    }

    pub fn clear_proof(&mut self) {
        self.renderer.set_proof(None, false);
        self.proof = None;
    }

    /// The framed photo as RGB8 for the histogram, through the soft proof when one is on.
    pub async fn histogram_thumbnail(&mut self, max_long_edge: u32) -> Result<Vec<u8>> {
        let mut rgb = self.framed_thumbnail(max_long_edge).await?;
        if let Some(lut) = &self.proof {
            apply_proof_lut(lut, PROOF_LUT_SIZE, &mut rgb);
        }
        Ok(rgb)
    }

    /// Sets the local adjustment masks, one layer per RGBA channel, covering the image.
    pub fn set_local_mask(&mut self, width: u32, height: u32, rgba: &[u8]) {
        self.renderer.set_local_mask(width, height, rgba);
    }

    /// Replaces the list of healed spots, given as JSON.
    pub fn set_heals(&mut self, json: &str) -> Result<()> {
        self.heals = serde_json::from_str(json).map_err(to_error)?;
        self.forget_full_view();
        self.refresh_healed_preview();
        Ok(())
    }

    /// A rectangle given as fractions of the image, at the best resolution available and
    /// with earlier erases applied, as a 0..=1 sRGB NCHW tensor for the inpainting model.
    pub fn inpaint_input(&self, x: f32, y: f32, width: f32, height: f32, size: u32) -> Result<Vec<f32>> {
        let source = self.developed.as_ref().or(self.working.as_ref()).or(self.proxy.as_ref());
        let source = source.ok_or_else(|| EngineError::new("no image loaded"))?;
        let (full_w, full_h) = (source.width as f32, source.height as f32);
        let left = ((x * full_w).round() as u32).min(source.width - 1);
        let top = ((y * full_h).round() as u32).min(source.height - 1);
        let crop_w = ((width * full_w).round() as u32).clamp(1, source.width - left);
        let crop_h = ((height * full_h).round() as u32).clamp(1, source.height - top);
        let mut crop = source.crop(left, top, crop_w, crop_h);
        let (span_w, span_h) = (crop_w as f32, crop_h as f32);
        let moved: Vec<ErasePatch> = self
            .erases
            .iter()
            .map(|patch| ErasePatch {
                x: (patch.x * full_w - left as f32) / span_w,
                y: (patch.y * full_h - top as f32) / span_h,
                width: patch.width * full_w / span_w,
                height: patch.height * full_h / span_h,
                ..patch.clone()
            })
            .collect();
        apply_erases(&mut crop, &moved);
        Ok(crop.srgb_nchw_tensor(size, size, [0.0; 3], [1.0; 3]))
    }

    pub fn clear_erases(&mut self) {
        self.erases.clear();
    }

    /// Adds a patch to the list. Takes effect on the next `set_heals` or `refresh_erases`.
    #[allow(clippy::too_many_arguments)]
    pub fn push_erase(&mut self, x: f32, y: f32, width: f32, height: f32, size: u32, rgb: Vec<u8>, alpha: Vec<u8>) -> Result<()> {
        let pixels = size as usize * size as usize;
        if size == 0 || rgb.len() != pixels * 3 || alpha.len() != pixels {
            return Err(EngineError::new("erase patch has the wrong size"));
        }
        self.erases.push(ErasePatch { x, y, width, height, size, rgb, alpha });
        Ok(())
    }

    pub fn refresh_erases(&mut self) {
        self.forget_full_view();
        self.refresh_healed_preview();
    }

    /// Sets how the image is shown: `zoom` times its fitted size, around (x, y) in 0..=1.
    pub fn set_view(&mut self, zoom: f32, x: f32, y: f32) {
        self.renderer.set_view(ViewTransform { zoom, center: [x, y] });
    }

    pub fn set_clip_warnings(&mut self, highlights: bool, shadows: bool) {
        self.renderer.set_clip_warnings(highlights, shadows);
    }

    /// Whether the full-resolution layer should change, because the view moved or it
    /// is no longer needed. Removes it in the latter case.
    pub fn needs_detail(&mut self) -> bool {
        match self.wanted_detail() {
            Some((min, max)) => !self.renderer.detail_covers(min, max),
            None => {
                self.renderer.clear_detail();
                false
            }
        }
    }

    /// Develops the area in view at full resolution from now on.
    pub fn update_detail(&mut self) {
        let Some((min, max)) = self.wanted_detail() else { return };
        if (!self.erases.is_empty() || !self.heals.is_empty()) && self.full_view.is_none() {
            self.full_view = self.developed.as_ref().or(self.working.as_ref()).cloned();
            if let Some(image) = &mut self.full_view {
                apply_erases(image, &self.erases);
                apply_heals(image, &self.heals);
            }
        }
        let source = self.full_view.as_ref().or(self.developed.as_ref()).or(self.working.as_ref());
        let Some(source) = source else { return };
        let (full_w, full_h) = (source.width as f32, source.height as f32);
        // Padded, unless that would make the layer too large.
        let pad = |low: f32, high: f32, full: f32| {
            let margin = ((high - low) * DETAIL_PADDING).min(((MAX_DETAIL_SIDE / full - (high - low)) / 2.0).max(0.0));
            (((low - margin).max(0.0) * full).floor() as u32, ((high + margin).min(1.0) * full).ceil() as u32)
        };
        let (left, right) = pad(min[0], max[0], full_w);
        let (top, bottom) = pad(min[1], max[1], full_h);
        let (right, bottom) = (right.min(source.width), bottom.min(source.height));
        if right > left && bottom > top {
            self.renderer.set_detail(source, [left, top, right - left, bottom - top]);
        }
    }

    /// Sets the screen fraction, from the left, that shows the unedited image.
    pub fn set_compare(&mut self, split: f32) {
        self.compare = split;
    }

    /// Adds spots to the end of the list, given as JSON, without redoing the earlier ones.
    pub fn add_heals(&mut self, json: &str) -> Result<()> {
        let added: Vec<HealSpot> = serde_json::from_str(json).map_err(to_error)?;
        let proxy = self.proxy.as_ref().ok_or_else(|| EngineError::new("no image loaded"))?;
        let healed = self.healed_proxy.get_or_insert_with(|| proxy.clone());
        apply_heals(healed, &added);
        self.renderer.update_preview_pixels(healed);
        self.heals.extend(added);
        self.forget_full_view();
        Ok(())
    }

    /// The temperature and tint that make the colour at a stage position neutral, as
    /// JSON, or `null` if the position missed the image or its colour is unusable.
    pub fn white_balance_at(&self, stage_x: f32, stage_y: f32, stage_width: u32, stage_height: u32) -> Result<String> {
        const SAMPLE_RADIUS: u32 = 3;
        let size = (stage_width.max(1), stage_height.max(1));
        let Some((u, v)) = self.stage_to_source(size, stage_x, stage_y) else {
            return Ok("null".into());
        };
        let proxy = self.healed_proxy.as_ref().or(self.proxy.as_ref());
        let proxy = proxy.ok_or_else(|| EngineError::new("no image loaded"))?;
        let colour = proxy.mean_around(u, v, SAMPLE_RADIUS);
        let matched = self.renderer.camera_rows(&self.adjustments).map(|row| {
            row[0] * colour[0] + row[1] * colour[1] + row[2] * colour[2]
        });
        Ok(match neutral_white_balance(matched) {
            Some((temperature, tint)) => serde_json::to_string(&WhiteBalance { temperature, tint }).map_err(to_error)?,
            None => "null".into(),
        })
    }

    /// The hue in degrees of the colour at a stage position, or a negative number if it
    /// missed the image or the colour there is close to grey.
    pub fn hue_at(&self, stage_x: f32, stage_y: f32, stage_width: u32, stage_height: u32) -> f32 {
        const SAMPLE_RADIUS: u32 = 3;
        const MIN_SATURATION: f32 = 0.08;
        let size = (stage_width.max(1), stage_height.max(1));
        let (Some((u, v)), Some(proxy)) = (self.stage_to_source(size, stage_x, stage_y), self.proxy.as_ref()) else {
            return -1.0;
        };
        let [r, g, b] = proxy.mean_around(u, v, SAMPLE_RADIUS);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        if max <= 0.0 || (max - min) / max < MIN_SATURATION {
            return -1.0;
        }
        let delta = max - min;
        let sector = if max == r { ((g - b) / delta).rem_euclid(6.0) } else if max == g { (b - r) / delta + 2.0 } else { (r - g) / delta + 4.0 };
        sector * 60.0
    }

    /// Heals a spot at a position in 0..=1 on a stage of the given size, with a radius
    /// as a fraction of the long edge. Returns the spot as JSON, or `null` if the
    /// position missed the image.
    pub fn heal_at(
        &mut self,
        stage_x: f32,
        stage_y: f32,
        stage_width: u32,
        stage_height: u32,
        radius: f32,
    ) -> Result<String> {
        // The size comes from the caller so the position and the layout it was measured
        // against always agree, even while a resize is still on its way here.
        let size = (stage_width.max(1), stage_height.max(1));
        // Spots live on the unwarped image, while the stage shows the corrected one.
        let Some((u, v)) = self.stage_to_source(size, stage_x, stage_y) else {
            return Ok("null".into());
        };
        let current = self.healed_proxy.as_ref().or(self.proxy.as_ref());
        let current = current.ok_or_else(|| EngineError::new("no image loaded"))?;
        let Some(spot) = find_heal_source(current, u, v, radius) else {
            return Ok("null".into());
        };
        self.heals.push(spot);
        self.forget_full_view();
        self.refresh_healed_preview();
        serde_json::to_string(&spot).map_err(to_error)
    }

    /// Renders the proxy at thumbnail size, with the current adjustments, as RGB8.
    pub async fn thumbnail(&mut self, max_long_edge: u32) -> Result<Vec<u8>> {
        let proxy = self.healed_proxy.as_ref().or(self.proxy.as_ref());
        let proxy = proxy.ok_or_else(|| EngineError::new("no image loaded"))?;
        let thumbnail = proxy.proxy(max_long_edge);
        self.renderer
            .export_rgb8(&thumbnail, &self.adjustments, EXPORT_TILE_SIZE)
            .await
            .map_err(to_error)
    }

    /// Suggests tone sliders for the photo as framed, as JSON. Leaves the edit as it was.
    /// Measures without creative effects, so it reads the photo rather than undoing them.
    pub async fn auto_tone(&mut self) -> Result<String> {
        let original = self.adjustments;
        let suggestion = self.measure_auto_tone().await;
        self.adjustments = original;
        serde_json::to_string(&suggestion?.rounded()).map_err(to_error)
    }

    /// Renders the proxy with crop and rotation applied, as RGB8.
    pub async fn framed_thumbnail(&mut self, max_long_edge: u32) -> Result<Vec<u8>> {
        let rgb = self.thumbnail(max_long_edge).await?;
        let (width, height) = self.thumbnail_dimensions(max_long_edge)?;
        let corrected = self.renderer.lens_map(&self.adjustments, width, height).apply_rgb(&rgb, width, height);
        let warped = self.warped(corrected, width, height);
        let (mut framed, frame_width, frame_height) = self.geometry.apply_rgb(&warped, width, height);
        post_crop_vignette(&mut framed, frame_width, frame_height, self.adjustments.post_crop_vignette);
        Ok(framed)
    }

    /// Size of [`Self::framed_thumbnail`] for the same `max_long_edge`, as JSON.
    pub fn framed_thumbnail_size(&self, max_long_edge: u32) -> Result<String> {
        let (width, height) = self.thumbnail_dimensions(max_long_edge)?;
        let (width, height) = self.geometry.output_size(width, height);
        serde_json::to_string(&ProxyInfo { width, height }).map_err(to_error)
    }

    /// Size of [`Self::thumbnail`] for the same `max_long_edge`, as JSON.
    pub fn thumbnail_size(&self, max_long_edge: u32) -> Result<String> {
        let (width, height) = self.thumbnail_dimensions(max_long_edge)?;
        serde_json::to_string(&ProxyInfo { width, height }).map_err(to_error)
    }

    /// Starts developing the open RAW at full quality. Returns JSON with the recipe and
    /// tiles for `process_develop_tile`, or `null` if there is nothing to split up.
    pub fn begin_raw_develop(&mut self) -> Result<String> {
        let Some(file) = &self.raw_file else { return Ok("null".into()) };
        let Some(job) = DevelopJob::start(file).map_err(to_error)? else { return Ok("null".into()) };
        self.job_tiles = job.tiles();
        let plan = serde_json::json!({ "recipe": job.recipe, "tiles": self.job_tiles });
        self.job = Some(job);
        Ok(plan.to_string())
    }

    /// The sensor samples for a tile of the job in progress.
    pub fn raw_tile_input(&self, index: usize) -> Result<Vec<f32>> {
        let job = self.job.as_ref().ok_or_else(|| EngineError::new("no develop in progress"))?;
        Ok(job.tile_input(&self.job_tiles[index]))
    }

    pub fn store_raw_tile(&mut self, index: usize, rgb: &[u16]) -> Result<()> {
        let job = self.job.as_mut().ok_or_else(|| EngineError::new("no develop in progress"))?;
        job.store_tile(&self.job_tiles[index], rgb);
        Ok(())
    }

    pub fn finish_raw_develop(&mut self) -> Result<()> {
        let job = self.job.take().ok_or_else(|| EngineError::new("no develop in progress"))?;
        self.developed = Some(job.finish());
        Ok(())
    }

    /// The size of the file `export_image` will write, as JSON. A RAW's development
    /// should already have finished, as it can differ from the announced size by a pixel.
    pub fn export_size(&self, long_edge: u32) -> Result<String> {
        let (width, height) = match (&self.developed, &self.working) {
            (Some(image), _) | (None, Some(image)) if self.raw_file.is_none() || self.developed.is_some() => (image.width, image.height),
            _ => self.full_size,
        };
        let (width, height) = self.geometry.output_size(width, height);
        let (width, height) = fit_long_edge(width, height, long_edge);
        serde_json::to_string(&ProxyInfo { width, height }).map_err(to_error)
    }

    /// Develops the full-resolution image and returns the encoded file. `overlay` is
    /// straight-alpha RGBA drawn on the finished image; it can be empty.
    pub async fn export_image(
        &mut self,
        settings: String,
        overlay: Vec<u8>,
        overlay_x: i32,
        overlay_y: i32,
        overlay_width: u32,
        overlay_height: u32,
    ) -> Result<Vec<u8>> {
        let settings: OutputSettings = serde_json::from_str(&settings).map_err(to_error)?;
        // Export needs the memory more than the close-up view does.
        self.forget_full_view();
        let mut developed = match (self.developed.take(), &self.raw_file) {
            (Some(image), _) => Some(image),
            (None, Some(file)) => Some(op_core::develop_raw_full(file).map_err(to_error)?),
            (None, None) if !self.heals.is_empty() || !self.erases.is_empty() => self.working.clone(),
            (None, None) => None,
        };
        if let Some(image) = &mut developed {
            apply_erases(image, &self.erases);
            apply_heals(image, &self.heals);
        }
        let image = developed
            .as_ref()
            .or(self.working.as_ref())
            .ok_or_else(|| EngineError::new("no image loaded"))?;
        let (image_width, image_height) = (image.width, image.height);
        let overlay = Overlay { rgba: &overlay, x: overlay_x, y: overlay_y, width: overlay_width, height: overlay_height };

        let space = settings.color_space;
        let wide = settings.bit_depth == 16 && settings.format != OutputFormat::Jpeg;
        let icc = space.icc_profile();
        let mut exif = self.source_exif.clone();
        if space != OutputSpace::Srgb
            && let Some(exif) = &mut exif
        {
            mark_uncalibrated(exif);
        }
        let exif = exif.as_deref();

        if wide || space != OutputSpace::Srgb {
            // The develop pass encodes into the space itself, so colours outside sRGB that
            // the photo holds survive; 8-bit files are rounded once, from 16 bits.
            let adjustments = Adjustments { output_space: space.shader_code(), ..self.adjustments };
            let rgb = self.renderer.export_rgb16(image, &adjustments, EXPORT_TILE_SIZE).await.map_err(to_error)?;
            // The source is no longer needed, and the steps below each allocate a copy.
            drop(developed);
            let (finished, width, height) = self.finish(rgb, image_width, image_height, settings.long_edge, &overlay);
            if wide {
                return encode_lossless(settings.format, &finished, width, height, &icc, exif);
            }
            let converted: Vec<u8> = finished.iter().map(|&v| ((u32::from(v) * 255 + 32767) / 65535) as u8).collect();
            drop(finished);
            return match settings.format {
                OutputFormat::Jpeg => encode_jpeg(&converted, width, height, settings.quality.clamp(1, 100), &icc, exif.map(<[u8]>::to_vec)).map_err(to_error),
                format => encode_lossless(format, &converted, width, height, &icc, exif),
            };
        }

        let rgb = self.renderer.export_rgb8(image, &self.adjustments, EXPORT_TILE_SIZE).await.map_err(to_error)?;
        drop(developed);
        let (finished, width, height) = self.finish(rgb, image_width, image_height, settings.long_edge, &overlay);
        match settings.format {
            OutputFormat::Jpeg => encode_jpeg(&finished, width, height, settings.quality.clamp(1, 100), &icc, exif.map(<[u8]>::to_vec)).map_err(to_error),
            format => encode_lossless(format, &finished, width, height, &icc, exif),
        }
    }
}

impl Engine {
    /// Drops the previous image first so two large buffers never coexist.
    fn release_image(&mut self) {
        self.working = None;
        self.raw_file = None;
        self.heals.clear();
        self.erases.clear();
        self.healed_proxy = None;
        // The next photo's sky is unknown, and the last one's must not show on it.
        self.renderer.set_sky_mask(1, 1, &[0]);
        // Erasing develops a RAW ahead of export; that must not outlive its photo.
        self.developed = None;
        self.forget_full_view();
    }

    fn finish_open(&mut self, opened: op_core::Opened, bytes: Option<&[u8]>) -> Result<String> {
        self.full_size = (opened.width, opened.height);
        let info = ImageInfo {
            width: opened.width,
            height: opened.height,
            is_raw: opened.is_raw,
            camera: opened.camera,
            base_exposure: opened.base_exposure,
            lens: opened.lens,
            lens_query: opened.lens_query,
            capture_time: self.source_exif.as_deref().and_then(op_core::capture_time),
            camera_match: opened.camera_matrix.is_some(),
        };
        self.renderer.set_base_exposure(opened.base_exposure);
        self.renderer.set_camera_data(opened.lens, opened.camera_matrix);
        self.working = Some(opened.working);
        if opened.is_raw {
            self.raw_file = bytes.map(<[u8]>::to_vec);
        }
        serde_json::to_string(&info).map_err(to_error)
    }

    /// Follows a stage position back through framing, liquify and lens correction, the
    /// reverse of how the image reaches the stage.
    /// The source UV shown at a stage position in 0..=1, through framing, liquify and lens
    /// correction, as JSON `[u, v]`, or `null` if the position missed the image.
    pub fn source_at(&self, stage_x: f32, stage_y: f32, stage_width: u32, stage_height: u32) -> String {
        match self.stage_to_source((stage_width.max(1), stage_height.max(1)), stage_x, stage_y) {
            Some((u, v)) if (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v) => format!("[{u},{v}]"),
            _ => "null".into(),
        }
    }

    fn stage_to_source(&self, size: (u32, u32), stage_x: f32, stage_y: f32) -> Option<(f32, f32)> {
        let (u, v) = self.renderer.canvas_to_image(&self.geometry, size, stage_x, stage_y)?;
        let (du, dv) = self.warp.as_ref().map_or((0.0, 0.0), |field| field.offset_at(u, v));
        let (width, height) = self.proxy.as_ref().map_or((1, 1), |proxy| (proxy.width, proxy.height));
        Some(self.renderer.lens_map(&self.adjustments, width, height).source_uv(u + du, v + dv, 1))
    }

    fn forget_full_view(&mut self) {
        self.full_view = None;
        self.renderer.clear_detail();
    }

    /// The rectangle of source UV in view, when the proxy is too coarse to show it.
    fn wanted_detail(&self) -> Option<([f32; 2], [f32; 2])> {
        let shown = if self.crop_editing { self.geometry.whole_frame() } else { self.geometry };
        let target = self.target_size;
        let area = self.renderer.visible_area(&shown, target)?;
        let (proxy_w, proxy_h) = self.renderer.preview_size()?;
        let map = shown.uv_map(proxy_w, proxy_h);
        let lens = self.renderer.lens_map(&self.adjustments, proxy_w, proxy_h);
        let (mut min, mut max) = ([1.0f32; 2], [0.0f32; 2]);
        for row in 0..=DETAIL_SAMPLES {
            for col in 0..=DETAIL_SAMPLES {
                let s = area.start[0] + area.span[0] * col as f32 / DETAIL_SAMPLES as f32;
                let t = area.start[1] + area.span[1] * row as f32 / DETAIL_SAMPLES as f32;
                let u = map.origin[0] + s * map.axis_s[0] + t * map.axis_t[0];
                let v = map.origin[1] + s * map.axis_s[1] + t * map.axis_t[1];
                let (du, dv) = self.warp.as_ref().map_or((0.0, 0.0), |field| field.offset_at(u, v));
                let (x, y) = lens.source_uv(u + du, v + dv, 1);
                min = [min[0].min(x.clamp(0.0, 1.0)), min[1].min(y.clamp(0.0, 1.0))];
                max = [max[0].max(x.clamp(0.0, 1.0)), max[1].max(y.clamp(0.0, 1.0))];
            }
        }
        // Worth doing once a screen pixel covers less than one proxy pixel.
        let canvas_pixels = target.0 as f32 * area.quad[0];
        let proxy_pixels = area.span[0] * shown.output_size(proxy_w, proxy_h).0 as f32;
        let (full_w, full_h) = (self.full_size.0 as f32, self.full_size.1 as f32);
        let too_large = (max[0] - min[0]) * full_w > MAX_DETAIL_SIDE || (max[1] - min[1]) * full_h > MAX_DETAIL_SIDE;
        let larger_than_proxy = self.full_size.0 > proxy_w || self.full_size.1 > proxy_h;
        (proxy_pixels < canvas_pixels && !too_large && larger_than_proxy && max[0] > min[0] && max[1] > min[1]).then_some((min, max))
    }

    fn warped<S: Sample>(&self, rgb: Vec<S>, width: u32, height: u32) -> Vec<S> {
        match &self.warp {
            Some(field) if !field.is_identity() => field.warp_rgb(&rgb, width, height),
            _ => rgb,
        }
    }

    /// Applies, in the order the blit undoes them, the lens correction, liquify and
    /// framing, then the delivery size and the watermark.
    fn finish<S: Sample>(&self, rgb: Vec<S>, width: u32, height: u32, long_edge: u32, overlay: &Overlay) -> (Vec<S>, u32, u32) {
        // Each stage drops its input, so a large export holds two copies at most.
        let lens = self.renderer.lens_map(&self.adjustments, width, height);
        let corrected = if lens.is_identity() {
            rgb
        } else {
            let corrected = lens.apply_rgb(&rgb, width, height);
            drop(rgb);
            corrected
        };
        let warped = self.warped(corrected, width, height);
        let (framed, frame_width, frame_height) = if self.geometry == Geometry::default() {
            (warped, width, height)
        } else {
            let framed = self.geometry.apply_rgb(&warped, width, height);
            drop(warped);
            framed
        };
        let (out_width, out_height) = fit_long_edge(frame_width, frame_height, long_edge);
        let mut finished = resize_rgb(framed, frame_width, frame_height, out_width, out_height);
        post_crop_vignette(&mut finished, out_width, out_height, self.adjustments.post_crop_vignette);
        composite_overlay(&mut finished, out_width, out_height, overlay.rgba, overlay.x, overlay.y, overlay.width, overlay.height);
        (finished, out_width, out_height)
    }


    fn refresh_healed_preview(&mut self) {
        let Some(proxy) = &self.proxy else { return };
        if self.heals.is_empty() && self.erases.is_empty() {
            self.renderer.update_preview_pixels(proxy);
            self.healed_proxy = None;
            return;
        }
        let mut healed = proxy.clone();
        apply_erases(&mut healed, &self.erases);
        apply_heals(&mut healed, &self.heals);
        self.renderer.update_preview_pixels(&healed);
        self.healed_proxy = Some(healed);
    }

    async fn tone_stats(&mut self) -> Result<ToneStats> {
        Ok(ToneStats::from_rgb8(&self.framed_thumbnail(AUTO_TONE_LONG_EDGE).await?))
    }

    async fn measure_auto_tone(&mut self) -> Result<AutoTone> {
        let adj = &mut self.adjustments;
        (adj.exposure, adj.contrast, adj.highlights, adj.shadows) = (0.0, 0.0, 0.0, 0.0);
        (adj.whites, adj.blacks, adj.vibrance) = (0.0, 0.0, 0.0);
        (adj.vignette, adj.post_crop_vignette, adj.grain, adj.lut_amount, adj.curve_active) = (0.0, 0.0, 0.0, 0.0, 0.0);
        (adj.local, adj.local_overlay, adj.mask_overlay) = ([[0.0; 4]; 4], 0.0, 0.0);

        let stats = self.tone_stats().await?;
        self.adjustments.exposure = auto_exposure(&stats);
        let stats = self.tone_stats().await?;
        let mut tone = auto_finish(&stats, self.adjustments.exposure);

        let adj = &mut self.adjustments;
        (adj.contrast, adj.highlights, adj.shadows) = (tone.contrast, tone.highlights, tone.shadows);
        (adj.whites, adj.blacks, adj.vibrance) = (tone.whites, tone.blacks, tone.vibrance);
        let stats = self.tone_stats().await?;
        tone.exposure = trim_exposure(tone.exposure, &stats);
        Ok(tone)
    }

    fn thumbnail_dimensions(&self, max_long_edge: u32) -> Result<(u32, u32)> {
        let proxy = self.proxy.as_ref().ok_or_else(|| EngineError::new("no image loaded"))?;
        let thumbnail = proxy.proxy(max_long_edge);
        Ok((thumbnail.width, thumbnail.height))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileInfo {
    name: String,
    /// "RGB", "CMYK" or "GRAY".
    space: &'static str,
    is_output: bool,
}

fn describe(profile: &IccProfile) -> Result<String> {
    let space = match profile.space {
        op_core::ColourSpace::Rgb => "RGB",
        op_core::ColourSpace::Cmyk => "CMYK",
        op_core::ColourSpace::Gray => "GRAY",
    };
    serde_json::to_string(&ProfileInfo { name: profile.name.clone(), space, is_output: profile.is_output }).map_err(to_error)
}

/// Checks an ICC file can be used for soft proofing, and describes it as JSON.
pub fn describe_profile(icc: &[u8]) -> Result<String> {
    describe(&IccProfile::parse(icc).map_err(to_error)?)
}

fn encode_lossless<S: Sample>(format: OutputFormat, rgb: &[S], width: u32, height: u32, icc: &[u8], exif: Option<&[u8]>) -> Result<Vec<u8>> {
    match format {
        OutputFormat::Png => encode_png(rgb, width, height, icc, exif).map_err(to_error),
        _ => Ok(encode_tiff(rgb, width, height, icc, exif)),
    }
}
