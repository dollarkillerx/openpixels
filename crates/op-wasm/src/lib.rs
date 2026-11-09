//! Browser bindings. Everything here runs inside one dedicated Worker, which must
//! not call a method while an async one is still pending.

use op_core::{Adjustments, Geometry, HealSpot, LinearImage, apply_heals, find_heal_source};
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
        })
    }

    /// Opens a file for editing. Returns JSON metadata.
    pub fn decode(&mut self, bytes: &[u8]) -> Result<String, JsError> {
        // Drop the previous image first so two large buffers never coexist.
        self.working = None;
        self.raw_file = None;
        self.heals.clear();
        self.healed_proxy = None;
        let opened = op_core::open(bytes).map_err(js_error)?;
        let info = ImageInfo {
            width: opened.width,
            height: opened.height,
            is_raw: opened.is_raw,
            camera: opened.camera,
            base_exposure: opened.base_exposure,
        };
        self.renderer.set_base_exposure(opened.base_exposure);
        self.working = Some(opened.working);
        if opened.is_raw {
            self.raw_file = Some(bytes.to_vec());
        }
        serde_json::to_string(&info).map_err(js_error)
    }

    /// Builds and uploads the interactive proxy. Returns JSON with its size.
    pub fn build_proxy(&mut self, max_long_edge: u32) -> Result<String, JsError> {
        let image = self.working.as_ref().ok_or_else(|| JsError::new("no image loaded"))?;
        let proxy = image.proxy(max_long_edge);
        self.renderer.set_preview(&proxy);
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
        self.renderer
            .draw_preview(&self.adjustments, &shown, &view, (self.config.width, self.config.height))
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

    /// Replaces the list of healed spots, given as JSON.
    pub fn set_heals(&mut self, json: &str) -> Result<(), JsError> {
        self.heals = serde_json::from_str(json).map_err(js_error)?;
        self.refresh_healed_preview();
        Ok(())
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
        Ok(self.geometry.apply_rgb8(&rgb, width, height).0)
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

    /// Develops the full-resolution image and returns JPEG bytes.
    pub async fn export_jpeg(&mut self, quality: u8) -> Result<Vec<u8>, JsError> {
        let mut developed = match &self.raw_file {
            Some(file) => Some(op_core::develop_raw_full(file).map_err(js_error)?),
            None if !self.heals.is_empty() => self.working.clone(),
            None => None,
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
        let (framed, width, height) = self.geometry.apply_rgb8(&rgb, image.width, image.height);
        encode_jpeg(&framed, width, height, quality).map_err(js_error)
    }
}

impl Engine {
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
