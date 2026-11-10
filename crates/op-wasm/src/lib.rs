//! Browser bindings: the shared engine behind wasm-bindgen, drawing into an
//! OffscreenCanvas. Everything runs inside one dedicated Worker, which must not call a
//! method while an async one is still pending.

use op_gpu::Renderer;
use wasm_bindgen::prelude::*;
use web_sys::OffscreenCanvas;

fn js_error(error: impl std::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
}

#[wasm_bindgen]
pub struct Engine {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    inner: op_engine::Engine,
}

/// Develops one tile on a worker thread. See `DevelopJob`.
#[wasm_bindgen]
pub fn process_develop_tile(input: &[f32], tile_json: &str, recipe_json: &str) -> Result<Vec<u16>, JsError> {
    op_engine::process_develop_tile(input, tile_json, recipe_json).map_err(js_error)
}

/// Checks an ICC file can be used for soft proofing, and describes it as JSON.
#[wasm_bindgen]
pub fn describe_profile(icc: &[u8]) -> Result<String, JsError> {
    op_engine::describe_profile(icc).map_err(js_error)
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

        let mut inner = op_engine::Engine::new(Renderer::new(device.clone(), queue, format));
        inner.set_target_size(width, height);
        Ok(Engine { surface, config, device, inner })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
        self.inner.set_target_size(width, height);
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
        self.inner.draw(&view).map_err(js_error)?;
        self.inner.renderer().present(frame);
        self.inner.renderer().wait_idle().await.map_err(js_error)
    }

    pub fn decode(&mut self, bytes: &[u8]) -> Result<String, JsError> {
        self.inner.decode(bytes).map_err(js_error)
    }

    pub fn decode_rgba(&mut self, rgba: &[u8], width: u32, height: u32, source: &[u8]) -> Result<String, JsError> {
        self.inner.decode_rgba(rgba, width, height, source).map_err(js_error)
    }

    pub fn build_proxy(&mut self, max_long_edge: u32) -> Result<String, JsError> {
        self.inner.build_proxy(max_long_edge).map_err(js_error)
    }

    pub fn model_input(&self, width: u32, height: u32, mean: &[f32], std: &[f32]) -> Result<Vec<f32>, JsError> {
        self.inner.model_input(width, height, mean, std).map_err(js_error)
    }

    pub fn refine_model_mask(&self, width: u32, height: u32, mask: &[u8]) -> Result<Vec<u8>, JsError> {
        self.inner.refine_model_mask(width, height, mask).map_err(js_error)
    }

    pub fn set_replace_image(&mut self, sky: bool, width: u32, height: u32, rgba: &[u8]) {
        self.inner.set_replace_image(sky, width, height, rgba)
    }

    pub fn warp_field(&self) -> Vec<f32> {
        self.inner.warp_field()
    }

    pub fn warp_field_size(&self) -> Result<String, JsError> {
        self.inner.warp_field_size().map_err(js_error)
    }

    pub fn has_full_image(&self) -> bool {
        self.inner.has_full_image()
    }

    pub fn set_sky_mask(&mut self, width: u32, height: u32, mask: &[u8]) {
        self.inner.set_sky_mask(width, height, mask)
    }

    pub fn set_lens_calibration(&mut self, json: &str) -> Result<String, JsError> {
        self.inner.set_lens_calibration(json).map_err(js_error)
    }

    pub fn set_mask(&mut self, width: u32, height: u32, mask: &[u8]) {
        self.inner.set_mask(width, height, mask)
    }

    pub fn set_adjustments(&mut self, json: &str) -> Result<(), JsError> {
        self.inner.set_adjustments(json).map_err(js_error)
    }

    pub fn set_skin_mask(&mut self, mask: &[u8], face_width: f32) -> Result<(), JsError> {
        self.inner.set_skin_mask(mask, face_width).map_err(js_error)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn liquify(&mut self, from_x: f32, from_y: f32, to_x: f32, to_y: f32, stage_width: u32, stage_height: u32, radius: f32, strength: f32, mode: u8) -> Result<String, JsError> {
        self.inner.liquify(from_x, from_y, to_x, to_y, stage_width, stage_height, radius, strength, mode).map_err(js_error)
    }

    pub fn set_warp_strokes(&mut self, json: &str) -> Result<(), JsError> {
        self.inner.set_warp_strokes(json).map_err(js_error)
    }

    pub fn set_look(&mut self, size: u32, rgb: &[f32]) {
        self.inner.set_look(size, rgb)
    }

    pub fn set_proof(&mut self, icc: &[u8], perceptual: bool, simulate_paper: bool, gamut_warning: bool) -> Result<String, JsError> {
        self.inner.set_proof(icc, perceptual, simulate_paper, gamut_warning).map_err(js_error)
    }

    pub fn clear_proof(&mut self) {
        self.inner.clear_proof()
    }

    pub async fn histogram_thumbnail(&mut self, max_long_edge: u32) -> Result<Vec<u8>, JsError> {
        self.inner.histogram_thumbnail(max_long_edge).await.map_err(js_error)
    }

    pub fn set_local_mask(&mut self, width: u32, height: u32, rgba: &[u8]) {
        self.inner.set_local_mask(width, height, rgba)
    }

    pub fn set_heals(&mut self, json: &str) -> Result<(), JsError> {
        self.inner.set_heals(json).map_err(js_error)
    }

    pub fn inpaint_input(&self, x: f32, y: f32, width: f32, height: f32, size: u32) -> Result<Vec<f32>, JsError> {
        self.inner.inpaint_input(x, y, width, height, size).map_err(js_error)
    }

    pub fn clear_erases(&mut self) {
        self.inner.clear_erases()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_erase(&mut self, x: f32, y: f32, width: f32, height: f32, size: u32, rgb: Vec<u8>, alpha: Vec<u8>) -> Result<(), JsError> {
        self.inner.push_erase(x, y, width, height, size, rgb, alpha).map_err(js_error)
    }

    pub fn refresh_erases(&mut self) {
        self.inner.refresh_erases()
    }

    pub fn set_view(&mut self, zoom: f32, x: f32, y: f32) {
        self.inner.set_view(zoom, x, y)
    }

    pub fn set_clip_warnings(&mut self, highlights: bool, shadows: bool) {
        self.inner.set_clip_warnings(highlights, shadows)
    }

    pub fn needs_detail(&mut self) -> bool {
        self.inner.needs_detail()
    }

    pub fn update_detail(&mut self) {
        self.inner.update_detail()
    }

    pub fn set_compare(&mut self, split: f32) {
        self.inner.set_compare(split)
    }

    pub fn add_heals(&mut self, json: &str) -> Result<(), JsError> {
        self.inner.add_heals(json).map_err(js_error)
    }

    pub fn white_balance_at(&self, stage_x: f32, stage_y: f32, stage_width: u32, stage_height: u32) -> Result<String, JsError> {
        self.inner.white_balance_at(stage_x, stage_y, stage_width, stage_height).map_err(js_error)
    }

    pub fn hue_at(&self, stage_x: f32, stage_y: f32, stage_width: u32, stage_height: u32) -> f32 {
        self.inner.hue_at(stage_x, stage_y, stage_width, stage_height)
    }

    pub fn heal_at(&mut self, stage_x: f32, stage_y: f32, stage_width: u32, stage_height: u32, radius: f32) -> Result<String, JsError> {
        self.inner.heal_at(stage_x, stage_y, stage_width, stage_height, radius).map_err(js_error)
    }

    pub async fn thumbnail(&mut self, max_long_edge: u32) -> Result<Vec<u8>, JsError> {
        self.inner.thumbnail(max_long_edge).await.map_err(js_error)
    }

    pub async fn auto_tone(&mut self) -> Result<String, JsError> {
        self.inner.auto_tone().await.map_err(js_error)
    }

    pub async fn framed_thumbnail(&mut self, max_long_edge: u32) -> Result<Vec<u8>, JsError> {
        self.inner.framed_thumbnail(max_long_edge).await.map_err(js_error)
    }

    pub fn framed_thumbnail_size(&self, max_long_edge: u32) -> Result<String, JsError> {
        self.inner.framed_thumbnail_size(max_long_edge).map_err(js_error)
    }

    pub fn thumbnail_size(&self, max_long_edge: u32) -> Result<String, JsError> {
        self.inner.thumbnail_size(max_long_edge).map_err(js_error)
    }

    pub fn begin_raw_develop(&mut self) -> Result<String, JsError> {
        self.inner.begin_raw_develop().map_err(js_error)
    }

    pub fn raw_tile_input(&self, index: usize) -> Result<Vec<f32>, JsError> {
        self.inner.raw_tile_input(index).map_err(js_error)
    }

    pub fn store_raw_tile(&mut self, index: usize, rgb: &[u16]) -> Result<(), JsError> {
        self.inner.store_raw_tile(index, rgb).map_err(js_error)
    }

    pub fn finish_raw_develop(&mut self) -> Result<(), JsError> {
        self.inner.finish_raw_develop().map_err(js_error)
    }

    pub fn export_size(&self, long_edge: u32) -> Result<String, JsError> {
        self.inner.export_size(long_edge).map_err(js_error)
    }

    pub async fn export_image(&mut self, settings: String, overlay: Vec<u8>, overlay_x: i32, overlay_y: i32, overlay_width: u32, overlay_height: u32) -> Result<Vec<u8>, JsError> {
        self.inner.export_image(settings, overlay, overlay_x, overlay_y, overlay_width, overlay_height).await.map_err(js_error)
    }
}
