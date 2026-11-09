//! Browser bindings. Everything here runs inside one dedicated Worker, which must
//! not call a method while an async one is still pending.

use op_core::{
    Adjustments, DevelopJob, Geometry, HealSpot, LinearImage, TileRecipe, ToneCurve, WarpField, WarpStroke, apply_heals,
    OutputFormat, composite_overlay, encode_png, encode_tiff, export_exif, find_heal_source, fit_long_edge,
    neutral_white_balance, resize_rgb8,
};
use op_gpu::{Renderer, encode_jpeg};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use web_sys::OffscreenCanvas;

const EXPORT_TILE_SIZE: u32 = 2048;

fn js_error(error: impl std::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
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
}

#[derive(Serialize)]
struct WhiteBalance {
    temperature: f32,
    tint: f32,
}

#[wasm_bindgen]
pub struct Engine {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    renderer: Renderer,
    /// The decoded image. For RAW this is only the half-size one, dropped once the
    /// proxy is built, because export re-develops from `raw_file` at full quality.
    working: Option<LinearImage>,
    raw_file: Option<Vec<u8>>,
    proxy: Option<LinearImage>,
    adjustments: Adjustments,
    geometry: Geometry,
    crop_editing: bool,
    heals: Vec<HealSpot>,
    healed_proxy: Option<LinearImage>,
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

/// Develops one tile on a worker thread. See `DevelopJob`.
#[wasm_bindgen]
pub fn process_develop_tile(input: &[f32], tile_json: &str, recipe_json: &str) -> Result<Vec<u16>, JsError> {
    let tile = serde_json::from_str(tile_json).map_err(js_error)?;
    let recipe: TileRecipe = serde_json::from_str(recipe_json).map_err(js_error)?;
    Ok(op_core::process_tile(input, &tile, &recipe))
}

#[wasm_bindgen]
impl Engine {
    pub async fn create(canvas: OffscreenCanvas) -> Result<Engine, JsError> {
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::OffscreenCanvas(canvas))
            .map_err(js_error)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(js_error)?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("openpixels"),
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .map_err(js_error)?;

        let capabilities = surface.get_capabilities(&adapter);
        // The develop shader already outputs sRGB-encoded values.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .ok_or_else(|| JsError::new("no non-sRGB canvas format available"))?;
        let mut config = surface
            .get_default_config(&adapter, width, height)
            .ok_or_else(|| JsError::new("canvas is not compatible with this GPU"))?;
        config.format = format;
        surface.configure(&device, &config);

        let renderer = Renderer::new(device.clone(), queue, format);
        Ok(Engine {
            surface,
            config,
            device,
            renderer,
            working: None,
            raw_file: None,
            proxy: None,
            adjustments: Adjustments::default(),
            geometry: Geometry::default(),
            crop_editing: false,
            heals: Vec::new(),
            healed_proxy: None,
            warp: None,
            curve: ToneCurve::default(),
            source_exif: None,
            job: None,
            job_tiles: Vec::new(),
            developed: None,
            compare: 0.0,
            full_size: (0, 0),
        })
    }

    /// Opens a file for editing. Returns JSON metadata.
    pub fn decode(&mut self, bytes: &[u8]) -> Result<String, JsError> {
        self.release_image();
        let opened = op_core::open(bytes).map_err(js_error)?;
        self.source_exif = Some(export_exif(bytes));
        self.finish_open(opened, Some(bytes))
    }

    /// Opens an image a browser-side codec already decoded. Returns JSON metadata.
    pub fn decode_rgba(&mut self, rgba: &[u8], width: u32, height: u32) -> Result<String, JsError> {
        self.release_image();
        let opened = op_core::open_rgba8(width, height, rgba).map_err(js_error)?;
        self.source_exif = None;
        self.finish_open(opened, None)
    }

    /// Builds and uploads the interactive proxy. Returns JSON with its size.
    pub fn build_proxy(&mut self, max_long_edge: u32) -> Result<String, JsError> {
        let image = self.working.as_ref().ok_or_else(|| JsError::new("no image loaded"))?;
        let proxy = image.proxy(max_long_edge);
        self.renderer.set_preview(&proxy);
        self.warp = Some(WarpField::new(proxy.width, proxy.height));
        let info = ProxyInfo { width: proxy.width, height: proxy.height };
        self.proxy = Some(proxy);
        if self.raw_file.is_some() {
            self.working = None;
        }
        serde_json::to_string(&info).map_err(js_error)
    }

    /// Returns the proxy as an NCHW float tensor for the segmentation model.
    pub fn model_input(
        &self,
        width: u32,
        height: u32,
        mean: &[f32],
        std: &[f32],
    ) -> Result<Vec<f32>, JsError> {
        let proxy = self.proxy.as_ref().ok_or_else(|| JsError::new("no image loaded"))?;
        let channels = |values: &[f32]| {
            <[f32; 3]>::try_from(values).map_err(|_| JsError::new("expected 3 channel values"))
        };
        Ok(proxy.srgb_nchw_tensor(width, height, channels(mean)?, channels(std)?))
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

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    pub fn set_adjustments(&mut self, json: &str) -> Result<(), JsError> {
        self.adjustments = serde_json::from_str(json).map_err(js_error)?;
        let curve: ToneCurve = serde_json::from_str(json).map_err(js_error)?;
        if curve != self.curve {
            self.renderer.set_curve(&curve);
            self.curve = curve;
        }
        self.adjustments.curve_active = f32::from(u8::from(!self.curve.is_identity()));
        self.geometry = serde_json::from_str(json).map_err(js_error)?;
        let flags: ViewFlags = serde_json::from_str(json).map_err(js_error)?;
        self.crop_editing = flags.crop_editing != 0.0;
        Ok(())
    }

    /// Draws the proxy and resolves when the GPU has finished the frame.
    pub async fn render(&mut self) -> Result<(), JsError> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            // Transient (hidden tab, resize in flight). The next render recovers.
            _ => return Ok(()),
        };
        let view = frame.texture.create_view(&Default::default());
        let shown = if self.crop_editing { self.geometry.whole_frame() } else { self.geometry };
        if self.compare > 0.0
            && !self.renderer.comparison_ready()
            && let Some(proxy) = &self.proxy
        {
            self.renderer.prepare_comparison(proxy, self.healed_proxy.as_ref());
        }
        self.renderer
            .draw_preview(&self.adjustments, &shown, &view, (self.config.width, self.config.height), self.compare)
            .map_err(js_error)?;
        self.renderer.present(frame);
        self.renderer.wait_idle().await.map_err(js_error)
    }

    /// Builds skin corrections from a facial skin mask at the proxy's size.
    pub fn set_skin_mask(&mut self, mask: &[u8], face_width: f32) -> Result<(), JsError> {
        let proxy = self.proxy.as_ref().ok_or_else(|| JsError::new("no image loaded"))?;
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
    ) -> Result<String, JsError> {
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
        serde_json::to_string(&stroke).map_err(js_error)
    }

    /// Rebuilds the liquify field from a list of strokes, given as JSON.
    pub fn set_warp_strokes(&mut self, json: &str) -> Result<(), JsError> {
        let strokes: Vec<WarpStroke> = serde_json::from_str(json).map_err(js_error)?;
        let proxy = self.proxy.as_ref().ok_or_else(|| JsError::new("no image loaded"))?;
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

    /// Sets the local adjustment masks, one layer per RGBA channel, covering the image.
    pub fn set_local_mask(&mut self, width: u32, height: u32, rgba: &[u8]) {
        self.renderer.set_local_mask(width, height, rgba);
    }

    /// Replaces the list of healed spots, given as JSON.
    pub fn set_heals(&mut self, json: &str) -> Result<(), JsError> {
        self.heals = serde_json::from_str(json).map_err(js_error)?;
        self.refresh_healed_preview();
        Ok(())
    }

    /// Sets the screen fraction, from the left, that shows the unedited image.
    pub fn set_compare(&mut self, split: f32) {
        self.compare = split;
    }

    /// Adds spots to the end of the list, given as JSON, without redoing the earlier ones.
    pub fn add_heals(&mut self, json: &str) -> Result<(), JsError> {
        let added: Vec<HealSpot> = serde_json::from_str(json).map_err(js_error)?;
        let proxy = self.proxy.as_ref().ok_or_else(|| JsError::new("no image loaded"))?;
        let healed = self.healed_proxy.get_or_insert_with(|| proxy.clone());
        apply_heals(healed, &added);
        self.renderer.update_preview_pixels(healed);
        self.heals.extend(added);
        Ok(())
    }

    /// The temperature and tint that make the colour at a stage position neutral, as
    /// JSON, or `null` if the position missed the image or its colour is unusable.
    pub fn white_balance_at(&self, stage_x: f32, stage_y: f32, stage_width: u32, stage_height: u32) -> Result<String, JsError> {
        const SAMPLE_RADIUS: u32 = 3;
        let size = (stage_width.max(1), stage_height.max(1));
        let Some((u, v)) = self.renderer.canvas_to_image(&self.geometry, size, stage_x, stage_y) else {
            return Ok("null".into());
        };
        let proxy = self.healed_proxy.as_ref().or(self.proxy.as_ref());
        let proxy = proxy.ok_or_else(|| JsError::new("no image loaded"))?;
        let (u, v) = self.renderer.lens_map(&self.adjustments, proxy.width, proxy.height).source_uv(u, v, 1);
        let colour = proxy.mean_around(u, v, SAMPLE_RADIUS);
        let matched = self.renderer.camera_rows(self.adjustments.camera_profile).map(|row| {
            row[0] * colour[0] + row[1] * colour[1] + row[2] * colour[2]
        });
        Ok(match neutral_white_balance(matched) {
            Some((temperature, tint)) => serde_json::to_string(&WhiteBalance { temperature, tint }).map_err(js_error)?,
            None => "null".into(),
        })
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
    ) -> Result<String, JsError> {
        // The size comes from the caller so the position and the layout it was measured
        // against always agree, even while a resize is still on its way here.
        let size = (stage_width.max(1), stage_height.max(1));
        let Some((u, v)) = self.renderer.canvas_to_image(&self.geometry, size, stage_x, stage_y) else {
            return Ok("null".into());
        };
        // Spots live on the unwarped image, while the stage shows the corrected one.
        let (proxy_width, proxy_height) = self.proxy.as_ref().map_or((1, 1), |proxy| (proxy.width, proxy.height));
        let (u, v) = self.renderer.lens_map(&self.adjustments, proxy_width, proxy_height).source_uv(u, v, 1);
        let current = self.healed_proxy.as_ref().or(self.proxy.as_ref());
        let current = current.ok_or_else(|| JsError::new("no image loaded"))?;
        let Some(spot) = find_heal_source(current, u, v, radius) else {
            return Ok("null".into());
        };
        self.heals.push(spot);
        self.refresh_healed_preview();
        serde_json::to_string(&spot).map_err(js_error)
    }

    /// Renders the proxy at thumbnail size, with the current adjustments, as RGB8.
    pub async fn thumbnail(&mut self, max_long_edge: u32) -> Result<Vec<u8>, JsError> {
        let proxy = self.healed_proxy.as_ref().or(self.proxy.as_ref());
        let proxy = proxy.ok_or_else(|| JsError::new("no image loaded"))?;
        let thumbnail = proxy.proxy(max_long_edge);
        self.renderer
            .export_rgb8(&thumbnail, &self.adjustments, EXPORT_TILE_SIZE)
            .await
            .map_err(js_error)
    }

    /// Renders the proxy with crop and rotation applied, as RGB8.
    pub async fn framed_thumbnail(&mut self, max_long_edge: u32) -> Result<Vec<u8>, JsError> {
        let rgb = self.thumbnail(max_long_edge).await?;
        let (width, height) = self.thumbnail_dimensions(max_long_edge)?;
        let corrected = self.renderer.lens_map(&self.adjustments, width, height).apply_rgb8(&rgb, width, height);
        let warped = self.warped(corrected, width, height);
        Ok(self.geometry.apply_rgb8(&warped, width, height).0)
    }

    /// Size of [`Self::framed_thumbnail`] for the same `max_long_edge`, as JSON.
    pub fn framed_thumbnail_size(&self, max_long_edge: u32) -> Result<String, JsError> {
        let (width, height) = self.thumbnail_dimensions(max_long_edge)?;
        let (width, height) = self.geometry.output_size(width, height);
        serde_json::to_string(&ProxyInfo { width, height }).map_err(js_error)
    }

    /// Size of [`Self::thumbnail`] for the same `max_long_edge`, as JSON.
    pub fn thumbnail_size(&self, max_long_edge: u32) -> Result<String, JsError> {
        let (width, height) = self.thumbnail_dimensions(max_long_edge)?;
        serde_json::to_string(&ProxyInfo { width, height }).map_err(js_error)
    }

    /// Starts developing the open RAW at full quality. Returns JSON with the recipe and
    /// tiles for `process_develop_tile`, or `null` if there is nothing to split up.
    pub fn begin_raw_develop(&mut self) -> Result<String, JsError> {
        let Some(file) = &self.raw_file else { return Ok("null".into()) };
        let Some(job) = DevelopJob::start(file).map_err(js_error)? else { return Ok("null".into()) };
        self.job_tiles = job.tiles();
        let plan = serde_json::json!({ "recipe": job.recipe, "tiles": self.job_tiles });
        self.job = Some(job);
        Ok(plan.to_string())
    }

    /// The sensor samples for a tile of the job in progress.
    pub fn raw_tile_input(&self, index: usize) -> Result<Vec<f32>, JsError> {
        let job = self.job.as_ref().ok_or_else(|| JsError::new("no develop in progress"))?;
        Ok(job.tile_input(&self.job_tiles[index]))
    }

    pub fn store_raw_tile(&mut self, index: usize, rgb: &[u16]) -> Result<(), JsError> {
        let job = self.job.as_mut().ok_or_else(|| JsError::new("no develop in progress"))?;
        job.store_tile(&self.job_tiles[index], rgb);
        Ok(())
    }

    pub fn finish_raw_develop(&mut self) -> Result<(), JsError> {
        let job = self.job.take().ok_or_else(|| JsError::new("no develop in progress"))?;
        self.developed = Some(job.finish());
        Ok(())
    }

    /// The size of the file `export_image` will write, as JSON. A RAW's development
    /// should already have finished, as it can differ from the announced size by a pixel.
    pub fn export_size(&self, long_edge: u32) -> Result<String, JsError> {
        let (width, height) = match (&self.developed, &self.working) {
            (Some(image), _) | (None, Some(image)) if self.raw_file.is_none() || self.developed.is_some() => (image.width, image.height),
            _ => self.full_size,
        };
        let (width, height) = self.geometry.output_size(width, height);
        let (width, height) = fit_long_edge(width, height, long_edge);
        serde_json::to_string(&ProxyInfo { width, height }).map_err(js_error)
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
    ) -> Result<Vec<u8>, JsError> {
        let settings: OutputSettings = serde_json::from_str(&settings).map_err(js_error)?;
        let mut developed = match (self.developed.take(), &self.raw_file) {
            (Some(image), _) => Some(image),
            (None, Some(file)) => Some(op_core::develop_raw_full(file).map_err(js_error)?),
            (None, None) if !self.heals.is_empty() => self.working.clone(),
            (None, None) => None,
        };
        if let Some(image) = &mut developed {
            apply_heals(image, &self.heals);
        }
        let image = developed
            .as_ref()
            .or(self.working.as_ref())
            .ok_or_else(|| JsError::new("no image loaded"))?;
        let rgb = self
            .renderer
            .export_rgb8(image, &self.adjustments, EXPORT_TILE_SIZE)
            .await
            .map_err(js_error)?;
        let (image_width, image_height) = (image.width, image.height);
        // In the order the blit undoes them: lens, then liquify, then framing.
        let corrected = self.renderer.lens_map(&self.adjustments, image_width, image_height).apply_rgb8(&rgb, image_width, image_height);
        let warped = self.warped(corrected, image_width, image_height);
        let (framed, frame_width, frame_height) = self.geometry.apply_rgb8(&warped, image_width, image_height);
        drop(warped);
        let (width, height) = fit_long_edge(frame_width, frame_height, settings.long_edge);
        let mut finished = resize_rgb8(framed, frame_width, frame_height, width, height);
        composite_overlay(&mut finished, width, height, &overlay, overlay_x, overlay_y, overlay_width, overlay_height);
        match settings.format {
            OutputFormat::Jpeg => encode_jpeg(&finished, width, height, settings.quality.clamp(1, 100), self.source_exif.clone()).map_err(js_error),
            OutputFormat::Png => encode_png(&finished, width, height).map_err(js_error),
            OutputFormat::Tiff => encode_tiff(&finished, width, height).map_err(js_error),
        }
    }
}

impl Engine {
    /// Drops the previous image first so two large buffers never coexist.
    fn release_image(&mut self) {
        self.working = None;
        self.raw_file = None;
        self.heals.clear();
        self.healed_proxy = None;
    }

    fn finish_open(&mut self, opened: op_core::Opened, bytes: Option<&[u8]>) -> Result<String, JsError> {
        self.full_size = (opened.width, opened.height);
        let info = ImageInfo {
            width: opened.width,
            height: opened.height,
            is_raw: opened.is_raw,
            camera: opened.camera,
            base_exposure: opened.base_exposure,
            lens: opened.lens,
            camera_match: opened.camera_matrix.is_some(),
        };
        self.renderer.set_base_exposure(opened.base_exposure);
        self.renderer.set_camera_data(opened.lens, opened.camera_matrix);
        self.working = Some(opened.working);
        if opened.is_raw {
            self.raw_file = bytes.map(<[u8]>::to_vec);
        }
        serde_json::to_string(&info).map_err(js_error)
    }

    fn warped(&self, rgb: Vec<u8>, width: u32, height: u32) -> Vec<u8> {
        match &self.warp {
            Some(field) if !field.is_identity() => field.warp_rgb8(&rgb, width, height),
            _ => rgb,
        }
    }

    fn refresh_healed_preview(&mut self) {
        let Some(proxy) = &self.proxy else { return };
        if self.heals.is_empty() {
            self.renderer.update_preview_pixels(proxy);
            self.healed_proxy = None;
            return;
        }
        let mut healed = proxy.clone();
        apply_heals(&mut healed, &self.heals);
        self.renderer.update_preview_pixels(&healed);
        self.healed_proxy = Some(healed);
    }

    fn thumbnail_dimensions(&self, max_long_edge: u32) -> Result<(u32, u32), JsError> {
        let proxy = self.proxy.as_ref().ok_or_else(|| JsError::new("no image loaded"))?;
        let thumbnail = proxy.proxy(max_long_edge);
        Ok((thumbnail.width, thumbnail.height))
    }
}
