//! Browser bindings. Everything here runs inside one dedicated Worker, which must
//! not call a method while an async one is still pending.

use op_core::{Adjustments, LinearImage};
use op_gpu::{Renderer, encode_jpeg};
use serde::Serialize;
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
        })
    }

    /// Opens a file for editing. Returns JSON metadata.
    pub fn decode(&mut self, bytes: &[u8]) -> Result<String, JsError> {
        // Drop the previous image first so two large buffers never coexist.
        self.working = None;
        self.raw_file = None;
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
        self.renderer.set_mask(width, height, mask);
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    pub fn set_adjustments(&mut self, json: &str) -> Result<(), JsError> {
        self.adjustments = serde_json::from_str(json).map_err(js_error)?;
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
        self.renderer
            .draw_preview(&self.adjustments, &view, (self.config.width, self.config.height))
            .map_err(js_error)?;
        self.renderer.present(frame);
        self.renderer.wait_idle().await.map_err(js_error)
    }

    /// Develops the full-resolution image and returns JPEG bytes.
    pub async fn export_jpeg(&mut self, quality: u8) -> Result<Vec<u8>, JsError> {
        let developed = match &self.raw_file {
            Some(file) => Some(op_core::develop_raw_full(file).map_err(js_error)?),
            None => None,
        };
        let image = developed
            .as_ref()
            .or(self.working.as_ref())
            .ok_or_else(|| JsError::new("no image loaded"))?;
        let rgb = self
            .renderer
            .export_rgb8(image, &self.adjustments, EXPORT_TILE_SIZE)
            .await
            .map_err(js_error)?;
        encode_jpeg(&rgb, image.width, image.height, quality).map_err(js_error)
    }
}
