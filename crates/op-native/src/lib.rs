//! Desktop bindings: the shared engine behind a C ABI for the Avalonia front end.
//!
//! One entry point, [`op_engine_call`], takes a method name, JSON arguments and an
//! optional byte buffer, and returns JSON and an optional byte buffer. Keeping the
//! surface this small means the C# side declares a handful of functions, and new
//! engine features need no new exports. The preview is drawn off screen and read back.
//!
//! An engine must be used from one thread at a time.

use std::panic::{AssertUnwindSafe, catch_unwind};

use op_engine::{EngineError, Result};
use op_gpu::Renderer;
use serde_json::{Value, json};

/// The format the preview is read back in: straight RGBA8, already sRGB-encoded.
const PREVIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct NativeEngine {
    device: wgpu::Device,
    queue: wgpu::Queue,
    engine: op_engine::Engine,
    target: Option<Target>,
}

/// The off-screen preview texture and the buffer it is copied into.
struct Target {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    readback: wgpu::Buffer,
    row_pitch: u32,
}

/// A byte buffer owned by Rust and handed to the caller, which returns it with
/// [`op_buffer_free`].
#[repr(C)]
pub struct OpBuffer {
    pub ptr: *mut u8,
    pub len: usize,
}

impl OpBuffer {
    const EMPTY: OpBuffer = OpBuffer { ptr: std::ptr::null_mut(), len: 0 };

    fn from_vec(bytes: Vec<u8>) -> OpBuffer {
        let mut boxed = bytes.into_boxed_slice();
        let buffer = OpBuffer { ptr: boxed.as_mut_ptr(), len: boxed.len() };
        std::mem::forget(boxed);
        buffer
    }
}

fn error(message: impl std::fmt::Display) -> EngineError {
    EngineError(message.to_string())
}

impl NativeEngine {
    pub fn new() -> Result<NativeEngine> {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() })
                .await
                .map_err(error)?;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor { label: Some("openpixels"), required_limits: adapter.limits(), ..Default::default() })
                .await
                .map_err(error)?;
            let engine = op_engine::Engine::new(Renderer::new(device.clone(), queue.clone(), PREVIEW_FORMAT));
            Ok(NativeEngine { device, queue, engine, target: None })
        })
    }

    /// Runs one method. Returns JSON and, for some methods, bytes.
    pub fn call(&mut self, method: &str, args: &Value, data: &[u8]) -> Result<(Value, Vec<u8>)> {
        let e = &mut self.engine;
        let num = |key: &str| args.get(key).and_then(Value::as_f64).ok_or_else(|| error(format!("{method}: missing {key}")));
        let int = |key: &str| num(key).map(|v| v as u32);
        let flag = |key: &str| args.get(key).and_then(Value::as_bool).unwrap_or(false);
        let raw = || args.to_string();
        let parsed = |text: String| serde_json::from_str::<Value>(&text).map_err(error);
        let floats = |bytes: &[u8]| -> Result<Vec<f32>> {
            if !bytes.len().is_multiple_of(4) {
                return Err(error(format!("{method}: data is not f32")));
            }
            Ok(bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
        };
        let float_bytes = |values: Vec<f32>| bytemuck::cast_slice::<f32, u8>(&values).to_vec();
        let none = Value::Null;
        Ok(match method {
            "decode" => (parsed(e.decode(data)?)?, vec![]),
            "decodeRgba" => {
                // RGBA pixels, then optionally the original file, for its metadata.
                let (width, height) = (int("width")?, int("height")?);
                let pixels = (width as usize) * (height as usize) * 4;
                let (rgba, source) = data.split_at_checked(pixels).ok_or_else(|| error("decodeRgba: data too short"))?;
                (parsed(e.decode_rgba(rgba, width, height, source)?)?, vec![])
            }
            "buildProxy" => (parsed(e.build_proxy(int("maxLongEdge")?)?)?, vec![]),
            "setAdjustments" => {
                e.set_adjustments(&raw())?;
                (none, vec![])
            }
            "render" => {
                let rgba = self.render(int("width")?, int("height")?)?;
                (none, rgba)
            }
            "setView" => {
                e.set_view(num("zoom")? as f32, num("x")? as f32, num("y")? as f32);
                (none, vec![])
            }
            "setClipWarnings" => {
                e.set_clip_warnings(flag("highlights"), flag("shadows"));
                (none, vec![])
            }
            "setCompare" => {
                e.set_compare(num("split")? as f32);
                (none, vec![])
            }
            "needsDetail" => (json!(e.needs_detail()), vec![]),
            "updateDetail" => {
                e.update_detail();
                (none, vec![])
            }
            "hasFullImage" => (json!(e.has_full_image()), vec![]),
            "developFull" => {
                self.develop_full()?;
                (none, vec![])
            }
            "thumbnail" => (none, pollster::block_on(e.thumbnail(int("maxLongEdge")?))?),
            "thumbnailSize" => (parsed(e.thumbnail_size(int("maxLongEdge")?)?)?, vec![]),
            "framedThumbnail" => (none, pollster::block_on(e.framed_thumbnail(int("maxLongEdge")?))?),
            "framedThumbnailSize" => (parsed(e.framed_thumbnail_size(int("maxLongEdge")?)?)?, vec![]),
            "histogramThumbnail" => (none, pollster::block_on(e.histogram_thumbnail(int("maxLongEdge")?))?),
            "autoTone" => (parsed(pollster::block_on(e.auto_tone())?)?, vec![]),
            "whiteBalanceAt" => (parsed(e.white_balance_at(num("x")? as f32, num("y")? as f32, int("stageWidth")?, int("stageHeight")?)?)?, vec![]),
            "hueAt" => (json!(e.hue_at(num("x")? as f32, num("y")? as f32, int("stageWidth")?, int("stageHeight")?)), vec![]),
            "healAt" => (parsed(e.heal_at(num("x")? as f32, num("y")? as f32, int("stageWidth")?, int("stageHeight")?, num("radius")? as f32)?)?, vec![]),
            "setHeals" => {
                e.set_heals(&raw())?;
                (none, vec![])
            }
            "addHeals" => {
                e.add_heals(&raw())?;
                (none, vec![])
            }
            "liquify" => (
                parsed(e.liquify(
                    num("fromX")? as f32,
                    num("fromY")? as f32,
                    num("toX")? as f32,
                    num("toY")? as f32,
                    int("stageWidth")?,
                    int("stageHeight")?,
                    num("radius")? as f32,
                    num("strength")? as f32,
                    int("mode")? as u8,
                )?)?,
                vec![],
            ),
            "setWarpStrokes" => {
                e.set_warp_strokes(&raw())?;
                (none, vec![])
            }
            "warpField" => (parsed(e.warp_field_size()?)?, float_bytes(e.warp_field())),
            "setLook" => {
                e.set_look(int("size")?, &floats(data)?);
                (none, vec![])
            }
            "setLocalMask" => {
                e.set_local_mask(int("width")?, int("height")?, data);
                (none, vec![])
            }
            "setMask" => {
                e.set_mask(int("width")?, int("height")?, data);
                (none, vec![])
            }
            "setSkyMask" => {
                e.set_sky_mask(int("width")?, int("height")?, data);
                (none, vec![])
            }
            "setReplaceImage" => {
                e.set_replace_image(flag("sky"), int("width")?, int("height")?, data);
                (none, vec![])
            }
            "setSkinMask" => {
                e.set_skin_mask(data, num("faceWidth")? as f32)?;
                (none, vec![])
            }
            "modelInput" => {
                let channel = |key: &str| -> Result<Vec<f32>> {
                    args.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).map(|v| v as f32).collect()).ok_or_else(|| error(format!("{method}: missing {key}")))
                };
                (none, float_bytes(e.model_input(int("width")?, int("height")?, &channel("mean")?, &channel("std")?)?))
            }
            "refineModelMask" => (none, e.refine_model_mask(int("width")?, int("height")?, data)?),
            "setLensCalibration" => (parsed(e.set_lens_calibration(&raw())?)?, vec![]),
            "setProof" => (parsed(e.set_proof(data, flag("perceptual"), flag("paper"), flag("gamut"))?)?, vec![]),
            "clearProof" => {
                e.clear_proof();
                (none, vec![])
            }
            "describeProfile" => (parsed(op_engine::describe_profile(data)?)?, vec![]),
            "inpaintInput" => (none, float_bytes(e.inpaint_input(num("x")? as f32, num("y")? as f32, num("width")? as f32, num("height")? as f32, int("size")?)?)),
            "clearErases" => {
                e.clear_erases();
                (none, vec![])
            }
            "pushErase" => {
                // RGB then alpha, for a square patch of `size`.
                let size = int("size")?;
                let rgb_len = (size as usize).pow(2) * 3;
                let (rgb, alpha) = data.split_at_checked(rgb_len).ok_or_else(|| error("pushErase: data too short"))?;
                e.push_erase(num("x")? as f32, num("y")? as f32, num("width")? as f32, num("height")? as f32, size, rgb.to_vec(), alpha.to_vec())?;
                (none, vec![])
            }
            "refreshErases" => {
                e.refresh_erases();
                (none, vec![])
            }
            "exportSize" => (parsed(e.export_size(int("longEdge")?)?)?, vec![]),
            "exportImage" => {
                let settings = args.get("settings").ok_or_else(|| error("exportImage: missing settings"))?.to_string();
                let bytes = pollster::block_on(e.export_image(
                    settings,
                    data.to_vec(),
                    num("overlayX").unwrap_or(0.0) as i32,
                    num("overlayY").unwrap_or(0.0) as i32,
                    num("overlayWidth").unwrap_or(0.0) as u32,
                    num("overlayHeight").unwrap_or(0.0) as u32,
                ))?;
                (none, bytes)
            }
            _ => return Err(error(format!("unknown method {method}"))),
        })
    }

    /// Draws the preview at `width` x `height` and returns it as RGBA8.
    fn render(&mut self, width: u32, height: u32) -> Result<Vec<u8>> {
        let (width, height) = (width.max(1), height.max(1));
        if self.target.as_ref().is_none_or(|t| t.width != width || t.height != height) {
            self.target = Some(self.create_target(width, height));
        }
        let target = self.target.as_ref().expect("created above");
        self.engine.set_target_size(width, height);
        let view = target.texture.create_view(&Default::default());
        self.engine.draw(&view)?;

        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &target.readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(target.row_pitch), rows_per_image: Some(height) },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let slice = target.readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        receiver.recv().map_err(error)?.map_err(error)?;
        let mapped = slice.get_mapped_range().map_err(error)?;
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for row in mapped.chunks(target.row_pitch as usize).take(height as usize) {
            rgba.extend_from_slice(&row[..(width * 4) as usize]);
        }
        drop(mapped);
        target.readback.unmap();
        Ok(rgba)
    }

    fn create_target(&self, width: u32, height: u32) -> Target {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("preview"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PREVIEW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let row_pitch = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preview readback"),
            size: u64::from(row_pitch) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Target { width, height, texture, readback, row_pitch }
    }

    /// Develops the open RAW at full quality across all cores.
    fn develop_full(&mut self) -> Result<()> {
        let plan: Value = serde_json::from_str(&self.engine.begin_raw_develop()?).map_err(error)?;
        if plan.is_null() {
            return Ok(());
        }
        let recipe = plan["recipe"].to_string();
        let tiles: Vec<String> = plan["tiles"].as_array().map(|t| t.iter().map(Value::to_string).collect()).unwrap_or_default();
        let threads = std::thread::available_parallelism().map_or(4, usize::from);
        for (batch, chunk) in tiles.chunks(threads).enumerate() {
            let first = batch * threads;
            let inputs = (0..chunk.len()).map(|i| self.engine.raw_tile_input(first + i)).collect::<Result<Vec<_>>>()?;
            let outputs = std::thread::scope(|scope| {
                let handles: Vec<_> = inputs
                    .iter()
                    .zip(chunk)
                    .map(|(input, tile)| scope.spawn(|| op_engine::process_develop_tile(input, tile, &recipe)))
                    .collect();
                handles.into_iter().map(|h| h.join().map_err(|_| error("a develop thread failed"))?).collect::<Result<Vec<_>>>()
            })?;
            for (i, rgb) in outputs.iter().enumerate() {
                self.engine.store_raw_tile(first + i, rgb)?;
            }
        }
        self.engine.finish_raw_develop()
    }
}

fn write_error(out: *mut OpBuffer, message: &str) {
    if !out.is_null() {
        // SAFETY: the caller passes a valid pointer to an OpBuffer it owns.
        unsafe { *out = OpBuffer::from_vec(message.as_bytes().to_vec()) };
    }
}

/// Creates an engine, or returns null and writes the reason to `error_out`.
///
/// # Safety
/// `error_out` must be null or point to an `OpBuffer`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn op_engine_create(error_out: *mut OpBuffer) -> *mut NativeEngine {
    match catch_unwind(NativeEngine::new) {
        Ok(Ok(engine)) => Box::into_raw(Box::new(engine)),
        Ok(Err(e)) => {
            write_error(error_out, &e.0);
            std::ptr::null_mut()
        }
        Err(_) => {
            write_error(error_out, "the engine failed to start");
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `engine` must come from [`op_engine_create`] and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn op_engine_destroy(engine: *mut NativeEngine) {
    if !engine.is_null() {
        // SAFETY: created by Box::into_raw in op_engine_create.
        drop(unsafe { Box::from_raw(engine) });
    }
}

/// Runs `method` with UTF-8 JSON `args` and optional bytes. Returns 0 and fills
/// `json_out` (UTF-8 JSON, possibly empty) and `data_out`, or returns 1 and puts the
/// error message in `json_out`.
///
/// # Safety
/// Pointers must be valid for their lengths (or null with length 0); `engine` must come
/// from [`op_engine_create`]; the out pointers must point to `OpBuffer`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn op_engine_call(
    engine: *mut NativeEngine,
    method: *const u8,
    method_len: usize,
    args: *const u8,
    args_len: usize,
    data: *const u8,
    data_len: usize,
    json_out: *mut OpBuffer,
    data_out: *mut OpBuffer,
) -> i32 {
    let slice = |ptr: *const u8, len: usize| -> &[u8] {
        if ptr.is_null() || len == 0 {
            &[]
        } else {
            // SAFETY: the caller guarantees the pointer is valid for `len` bytes.
            unsafe { std::slice::from_raw_parts(ptr, len) }
        }
    };
    if engine.is_null() || json_out.is_null() || data_out.is_null() {
        return 1;
    }
    // SAFETY: checked non-null; the caller owns these for the duration of the call.
    let (engine, json_out, data_out) = unsafe { (&mut *engine, &mut *json_out, &mut *data_out) };
    *json_out = OpBuffer::EMPTY;
    *data_out = OpBuffer::EMPTY;
    let method = String::from_utf8_lossy(slice(method, method_len)).into_owned();
    let args_bytes = slice(args, args_len);
    let data = slice(data, data_len);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let args: Value = if args_bytes.is_empty() { Value::Null } else { serde_json::from_slice(args_bytes).map_err(error)? };
        engine.call(&method, &args, data)
    }));
    match outcome {
        Ok(Ok((json, bytes))) => {
            if !json.is_null() {
                *json_out = OpBuffer::from_vec(json.to_string().into_bytes());
            }
            *data_out = OpBuffer::from_vec(bytes);
            0
        }
        Ok(Err(e)) => {
            *json_out = OpBuffer::from_vec(e.0.into_bytes());
            1
        }
        Err(_) => {
            *json_out = OpBuffer::from_vec(format!("{method} failed unexpectedly").into_bytes());
            1
        }
    }
}

/// Returns a buffer from [`op_engine_call`] or [`op_engine_create`].
///
/// # Safety
/// `buffer` must have come from this library and not been freed already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn op_buffer_free(buffer: OpBuffer) {
    if !buffer.ptr.is_null() {
        // SAFETY: built by OpBuffer::from_vec from a boxed slice of exactly this length.
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(buffer.ptr, buffer.len)) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use half::f16;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut rgb = Vec::new();
        for y in 0..height {
            for x in 0..width {
                rgb.extend([(x * 255 / width) as u8, (y * 255 / height) as u8, 90]);
            }
        }
        op_core::encode_png(&rgb, width, height, &op_core::srgb_icc_profile(), None).unwrap()
    }

    fn call(engine: &mut NativeEngine, method: &str, args: Value, data: &[u8]) -> (Value, Vec<u8>) {
        engine.call(method, &args, data).unwrap_or_else(|e| panic!("{method}: {e}"))
    }

    #[test]
    fn opens_edits_previews_and_exports_a_photo() {
        let _ = f16::ZERO;
        let mut engine = NativeEngine::new().expect("a GPU is required for this test");
        let (info, _) = call(&mut engine, "decode", Value::Null, &png(120, 80));
        assert_eq!(info["width"], 120);
        call(&mut engine, "buildProxy", json!({ "maxLongEdge": 64 }), &[]);
        let (_, plain) = call(&mut engine, "render", json!({ "width": 96, "height": 64 }), &[]);
        assert_eq!(plain.len(), 96 * 64 * 4);
        call(&mut engine, "setAdjustments", json!({ "exposure": 1.0 }), &[]);
        let (_, brighter) = call(&mut engine, "render", json!({ "width": 96, "height": 64 }), &[]);
        let centre = (32 * 96 + 48) * 4;
        assert!(brighter[centre + 1] > plain[centre + 1], "{} vs {}", brighter[centre + 1], plain[centre + 1]);
        let (_, jpeg) = call(&mut engine, "exportImage", json!({ "settings": { "format": "jpeg", "quality": 90, "longEdge": 0 } }), &[]);
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        let (tone, _) = call(&mut engine, "autoTone", Value::Null, &[]);
        assert!(tone["exposure"].is_number());
    }

    #[test]
    fn errors_come_back_as_messages() {
        let mut engine = NativeEngine::new().expect("a GPU is required for this test");
        assert!(engine.call("decode", &Value::Null, b"not an image").is_err());
        assert!(engine.call("noSuchMethod", &Value::Null, &[]).unwrap_err().0.contains("noSuchMethod"));
    }

    #[test]
    fn the_c_entry_points_round_trip() {
        let mut error = OpBuffer::EMPTY;
        // SAFETY: valid out pointer; the engine is destroyed below.
        let engine = unsafe { op_engine_create(&mut error) };
        assert!(!engine.is_null());
        let bytes = png(40, 30);
        let (mut json, mut data) = (OpBuffer::EMPTY, OpBuffer::EMPTY);
        let method = b"decode";
        // SAFETY: all pointers are valid for their lengths.
        let status = unsafe { op_engine_call(engine, method.as_ptr(), method.len(), std::ptr::null(), 0, bytes.as_ptr(), bytes.len(), &mut json, &mut data) };
        assert_eq!(status, 0);
        // SAFETY: json holds len UTF-8 bytes from the library.
        let text = unsafe { std::str::from_utf8(std::slice::from_raw_parts(json.ptr, json.len)).unwrap().to_string() };
        assert!(text.contains("\"width\":40"), "{text}");
        // SAFETY: both buffers came from the library and are freed once.
        unsafe {
            op_buffer_free(json);
            op_buffer_free(data);
            op_engine_destroy(engine);
        }
    }
}
