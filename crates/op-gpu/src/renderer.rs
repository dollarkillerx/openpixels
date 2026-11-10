use std::sync::OnceLock;

use bytemuck::{Pod, Zeroable, bytes_of, cast_slice};
use futures_channel::oneshot;
use op_core::{Adjustments, CURVE_LUT_SIZE, Geometry, LensMap, LensProfile, LinearImage, SkinRetouch, ToneCurve, WarpField};
use wgpu::util::align_to;

const SOURCE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const SOURCE_BYTES_PER_PIXEL: u32 = 8;
const OUTPUT_BYTES_PER_PIXEL: u32 = 4;
/// Floats, because 16-bit unsigned normalised textures are not part of core WebGPU.
const DEEP_OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
const DEEP_OUTPUT_BYTES_PER_PIXEL: u32 = 16;
const DEVELOP_SHADER: &str = include_str!("develop.wgsl");
const WORKGROUP_SIZE: u32 = 16;
/// Source pixels each export tile can see beyond its own area, for sharpening and denoising.
const TILE_MARGIN: u32 = 5;
const BLURRED_LUMA_LONG_EDGE: u32 = 128;
const DETAIL_LUMA_LONG_EDGE: u32 = 512;
/// The detail map is at least this much smaller than the image, or it would hold no less detail.
const DETAIL_LUMA_REDUCTION: u32 = 4;
const HAZE_LONG_EDGE: u32 = 256;

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no image is loaded")]
    NoImage,
    #[error("GPU buffer mapping failed: {0}")]
    Map(#[from] wgpu::BufferAsyncError),
    #[error("GPU buffer range is unavailable: {0}")]
    MapRange(#[from] wgpu::MapRangeError),
    #[error("GPU work was cancelled")]
    Cancelled,
    #[error("JPEG encoding failed: {0}")]
    Jpeg(#[from] jpeg_encoder::EncodingError),
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Region {
    full_size: [f32; 2],
    offset: [f32; 2],
    extent: [f32; 2],
    base_exposure: f32,
    has_base_look: f32,
    source_offset: [f32; 2],
    source_size: [f32; 2],
    lens_vignette: [f32; 4],
    lens_aspect: [f32; 2],
    _padding: [f32; 2],
    camera: [[f32; 4]; 3],
    airlight: [f32; 4],
    sky_tint: [f32; 4],
    output: [[f32; 4]; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlitParams {
    scale: [f32; 2],
    uv_origin: [f32; 2],
    uv_axis_s: [f32; 2],
    uv_axis_t: [f32; 2],
    lens_aspect: [f32; 2],
    lens_scale: f32,
    compare: f32,
    distortion: [f32; 4],
    chroma: [f32; 4],
    perspective: [f32; 4],
    view: [f32; 4],
    detail_rect: [f32; 4],
    flags: [f32; 4],
}

/// How the framed image is shown: `zoom` times its fitted size, around `center` (0..=1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewTransform {
    pub zoom: f32,
    pub center: [f32; 2],
}

impl Default for ViewTransform {
    fn default() -> Self {
        Self { zoom: 1.0, center: [0.5; 2] }
    }
}

/// The part of the framed image on the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisibleArea {
    /// Half size on the canvas in clip space: 1 fills that axis.
    pub quad: [f32; 2],
    /// First visible s and t.
    pub start: [f32; 2],
    /// Visible span of s and t.
    pub span: [f32; 2],
}

impl VisibleArea {
    /// What a fitted image of clip-space half size `fit` shows at `view`. An axis that
    /// does not fill the canvas stays centred; one that overflows is panned.
    pub fn new(fit: [f32; 2], view: ViewTransform) -> Self {
        let axis = |index: usize| {
            let zoomed = fit[index] * view.zoom;
            if zoomed <= 1.0 {
                return (zoomed, 0.0, 1.0);
            }
            let span = 1.0 / zoomed;
            (1.0, (view.center[index] - span / 2.0).clamp(0.0, 1.0 - span), span)
        };
        let (x, y) = (axis(0), axis(1));
        Self { quad: [x.0, y.0], start: [x.1, y.1], span: [x.2, y.2] }
    }
}

/// A region of the full-resolution image, developed alongside the proxy.
struct Detail {
    /// x, y, width and height in pixels of the image.
    region: [u32; 4],
    full_size: [u32; 2],
    source_view: wgpu::TextureView,
    source_offset: [u32; 2],
    source_size: [u32; 2],
    output_view: wgpu::TextureView,
    develop: wgpu::BindGroup,
}

struct Preview {
    width: u32,
    height: u32,
    source: wgpu::Texture,
    source_view: wgpu::TextureView,
    output_view: wgpu::TextureView,
    before_view: wgpu::TextureView,
    before_ready: bool,
    warp: wgpu::Texture,
    warp_view: wgpu::TextureView,
    develop: wgpu::BindGroup,
    blit: wgpu::BindGroup,
}

/// Develops images on the GPU: an interactive path over a downscaled proxy and a
/// tiled path for full-resolution export. Both run the same shader.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    develop: DevelopPass,
    /// The same pass writing floats, built when a 16-bit file is first exported.
    deep_develop: OnceLock<DevelopPass>,
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    adjust_buffer: wgpu::Buffer,
    region_buffer: wgpu::Buffer,
    blit_buffer: wgpu::Buffer,
    mask_view: wgpu::TextureView,
    blurred_luma_view: wgpu::TextureView,
    detail_luma_view: wgpu::TextureView,
    haze_view: wgpu::TextureView,
    airlight: [f32; 3],
    skin_smooth_view: wgpu::TextureView,
    skin_blemish_view: wgpu::TextureView,
    local_mask_view: wgpu::TextureView,
    curve_texture: wgpu::Texture,
    curve_view: wgpu::TextureView,
    look_view: wgpu::TextureView,
    background_image_view: wgpu::TextureView,
    sky_image_view: wgpu::TextureView,
    sky_mask_view: wgpu::TextureView,
    sky_tint: [f32; 3],
    view: ViewTransform,
    clip_warnings: [bool; 2],
    detail: Option<Detail>,
    no_detail_view: wgpu::TextureView,
    /// The soft proof table, and 0 for off, 1 to show it, 2 to also warn about gamut.
    proof_view: wgpu::TextureView,
    proof_mode: f32,
    base_exposure: Option<f32>,
    lens_profile: Option<LensProfile>,
    camera_matrix: Option<[[f32; 3]; 3]>,
    preview: Option<Preview>,
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, target: wgpu::TextureFormat) -> Self {
        let develop = develop_pass(&device, OUTPUT_FORMAT, DEVELOP_SHADER);
        let blit_module = device.create_shader_module(wgpu::include_wgsl!("blit.wgsl"));

        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[
                texture_entry(0, wgpu::ShaderStages::FRAGMENT, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform_entry(2, wgpu::ShaderStages::VERTEX_FRAGMENT),
                texture_entry(3, wgpu::ShaderStages::FRAGMENT, true),
                texture_entry(4, wgpu::ShaderStages::FRAGMENT, true),
                texture_entry(5, wgpu::ShaderStages::FRAGMENT, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let blit_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("blit"),
                bind_group_layouts: &[Some(&blit_layout)],
                immediate_size: 0,
            });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blit_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blit_module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(target.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let adjust_buffer = uniform_buffer(&device, "adjustments", size_of::<Adjustments>());
        let region_buffer = uniform_buffer(&device, "region", size_of::<Region>());
        let blit_buffer = uniform_buffer(&device, "blit", size_of::<BlitParams>());
        let mask_view = create_mask_view(&device, &queue, 1, 1, &[255]);
        let blurred_luma_view = create_luma_view(&device, &queue, 1, 1, &[0]);
        let detail_luma_view = create_luma_view(&device, &queue, 1, 1, &[0]);
        let haze_view = create_luma_view(&device, &queue, 1, 1, &[0]);
        let skin_smooth_view = create_correction_view(&device, &queue, 1, 1, &[0; 4]);
        let skin_blemish_view = create_correction_view(&device, &queue, 1, 1, &[0; 4]);
        let local_mask_view = create_local_mask_view(&device, &queue, 1, 1, &[0; 4]);
        let curve_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tone curve"),
            size: extent(CURVE_LUT_SIZE as u32, 1),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SOURCE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let curve_view = curve_texture.create_view(&Default::default());
        let look_view = create_look_view(&device, &queue, 2, &IDENTITY_LOOK);
        let background_image_view = create_replace_view(&device, &queue, 1, 1, &[0, 0, 0, 255]);
        let sky_image_view = create_replace_view(&device, &queue, 1, 1, &[0, 0, 0, 255]);
        let sky_mask_view = create_mask_view(&device, &queue, 1, 1, &[0]);
        let no_detail_view = create_replace_view(&device, &queue, 1, 1, &[0, 0, 0, 255]);
        let proof_view = create_proof_view(&device, &queue, 2, &IDENTITY_PROOF);

        Self {
            device,
            queue,
            develop,
            deep_develop: OnceLock::new(),
            blit_pipeline,
            blit_layout,
            sampler,
            adjust_buffer,
            region_buffer,
            blit_buffer,
            mask_view,
            blurred_luma_view,
            detail_luma_view,
            haze_view,
            airlight: [1.0; 3],
            skin_smooth_view,
            skin_blemish_view,
            local_mask_view,
            curve_texture,
            curve_view,
            look_view,
            background_image_view,
            sky_image_view,
            sky_mask_view,
            sky_tint: [1.0; 3],
            view: ViewTransform::default(),
            clip_warnings: [false; 2],
            detail: None,
            no_detail_view,
            proof_view,
            proof_mode: 0.0,
            base_exposure: None,
            lens_profile: None,
            camera_matrix: None,
            preview: None,
        }
    }

    /// Enables the default RAW look with an exposure offset in EV, or disables it for
    /// images that already have a tone curve.
    /// Sets what the open file says about its lens and its camera's colour.
    /// Replaces the lens corrections, for lenses the file itself says nothing about.
    pub fn set_lens_profile(&mut self, lens: LensProfile) {
        self.lens_profile = Some(lens);
    }

    pub fn set_camera_data(&mut self, lens: Option<LensProfile>, matrix: Option<[[f32; 3]; 3]>) {
        self.lens_profile = lens;
        self.camera_matrix = matrix;
    }

    /// The lens corrections in effect for an edit of an image this size.
    pub fn lens_map(&self, adjustments: &Adjustments, width: u32, height: u32) -> LensMap {
        LensMap::new(self.lens_profile.as_ref(), adjustments, width, height)
    }

    pub fn set_base_exposure(&mut self, base_exposure: Option<f32>) {
        self.base_exposure = base_exposure;
    }

    /// Sets the subject mask for the current image. `mask` holds `width * height`
    /// bytes covering the whole image, at any resolution.
    /// Sets the image that replaces the background, straight sRGB RGBA8.
    pub fn set_background_image(&mut self, width: u32, height: u32, rgba: &[u8]) {
        self.background_image_view = create_replace_view(&self.device, &self.queue, width, height, rgba);
        self.rebuild_preview_bindings();
    }

    /// Sets the image that replaces the sky, straight sRGB RGBA8.
    pub fn set_sky_image(&mut self, width: u32, height: u32, rgba: &[u8]) {
        self.sky_image_view = create_replace_view(&self.device, &self.queue, width, height, rgba);
        self.sky_tint = neutral_tint(rgba);
        self.rebuild_preview_bindings();
    }

    /// Sets where the sky is, `width * height` bytes covering the whole image.
    pub fn set_sky_mask(&mut self, width: u32, height: u32, mask: &[u8]) {
        self.sky_mask_view = create_mask_view(&self.device, &self.queue, width, height, mask);
        self.rebuild_preview_bindings();
    }

    pub fn set_mask(&mut self, width: u32, height: u32, mask: &[u8]) {
        self.mask_view = create_mask_view(&self.device, &self.queue, width, height, mask);
        self.rebuild_preview_bindings();
    }

    /// Updates the liquify field shown by the preview.
    pub fn update_warp(&self, field: &WarpField) {
        if let Some(preview) = &self.preview {
            self.write_warp(&preview.warp, field);
        }
    }

    fn write_warp(&self, texture: &wgpu::Texture, field: &WarpField) {
        self.queue.write_texture(
            texture.as_image_copy(),
            cast_slice(&field.to_half_bits()),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(field.width * 4),
                rows_per_image: Some(field.height),
            },
            extent(field.width, field.height),
        );
    }

    /// Replaces the proxy's pixels, keeping its mask and corrections. `proxy` must have
    /// the same size as the one given to [`Self::set_preview`]. The surround maps are
    /// remade too, so clarity and texture do not bring back what a heal removed.
    pub fn update_preview_pixels(&mut self, proxy: &LinearImage) {
        let Some(preview) = &self.preview else { return };
        let (width, height) = (proxy.width, proxy.height);
        self.upload_source(&preview.source, &proxy.rgba_region(0, 0, width, height), width, height);
        self.build_surround_maps(proxy);
        self.rebuild_preview_bindings();
    }

    fn build_surround_maps(&mut self, proxy: &LinearImage) {
        let (luma_width, luma_height, luma) = proxy.blurred_luma(BLURRED_LUMA_LONG_EDGE);
        self.blurred_luma_view = create_luma_view(&self.device, &self.queue, luma_width, luma_height, &luma);
        let detail_long_edge = (proxy.width.max(proxy.height) / DETAIL_LUMA_REDUCTION).clamp(1, DETAIL_LUMA_LONG_EDGE);
        let (detail_width, detail_height, detail) = proxy.blurred_luma(detail_long_edge);
        self.detail_luma_view = create_luma_view(&self.device, &self.queue, detail_width, detail_height, &detail);
        let (haze_width, haze_height, haze, airlight) = proxy.haze_map(HAZE_LONG_EDGE);
        self.haze_view = create_luma_view(&self.device, &self.queue, haze_width, haze_height, &haze);
        self.airlight = airlight;
    }

    /// Converts a position on the canvas, in 0..=1, to the position within the image
    /// drawn with `geometry`. `None` outside the image.
    pub fn canvas_to_image(
        &self,
        geometry: &Geometry,
        target_size: (u32, u32),
        canvas_x: f32,
        canvas_y: f32,
    ) -> Option<(f32, f32)> {
        let preview = self.preview.as_ref()?;
        let size = (preview.width, preview.height);
        let area = VisibleArea::new(letterbox(geometry.output_size(size.0, size.1), target_size), self.view);
        let on_quad = |position: f32, axis: usize| (position - 0.5) / area.quad[axis] + 0.5;
        let (across, down) = (on_quad(canvas_x, 0), on_quad(canvas_y, 1));
        if !(0.0..=1.0).contains(&across) || !(0.0..=1.0).contains(&down) {
            return None;
        }
        let (s, t) = (area.start[0] + across * area.span[0], area.start[1] + down * area.span[1]);
        let map = geometry.uv_map(size.0, size.1);
        let u = map.origin[0] + s * map.axis_s[0] + t * map.axis_t[0];
        let v = map.origin[1] + s * map.axis_s[1] + t * map.axis_t[1];
        ((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)).then_some((u, v))
    }

    /// Sets the creative 3D lookup table: `size`^3 RGB triples with red varying fastest.
    /// `None` restores the identity.
    pub fn set_look(&mut self, look: Option<(u32, &[f32])>) {
        self.look_view = match look {
            Some((size, rgb)) => create_look_view(&self.device, &self.queue, size, rgb),
            None => create_look_view(&self.device, &self.queue, 2, &IDENTITY_LOOK),
        };
        self.rebuild_preview_bindings();
    }

    /// Updates the tone curve lookup table.
    pub fn set_curve(&self, curve: &ToneCurve) {
        self.queue.write_texture(
            self.curve_texture.as_image_copy(),
            cast_slice(&curve.lut()),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(CURVE_LUT_SIZE as u32 * SOURCE_BYTES_PER_PIXEL),
                rows_per_image: Some(1),
            },
            extent(CURVE_LUT_SIZE as u32, 1),
        );
    }

    /// Sets the local adjustment masks, one layer per RGBA channel, covering the image.
    pub fn set_local_mask(&mut self, width: u32, height: u32, rgba: &[u8]) {
        self.local_mask_view = create_local_mask_view(&self.device, &self.queue, width, height, rgba);
        self.rebuild_preview_bindings();
    }

    /// Sets the skin corrections for the current image.
    pub fn set_skin_retouch(&mut self, retouch: &SkinRetouch) {
        let (width, height) = (retouch.width, retouch.height);
        self.skin_smooth_view = create_correction_view(&self.device, &self.queue, width, height, &retouch.smooth);
        self.skin_blemish_view = create_correction_view(&self.device, &self.queue, width, height, &retouch.blemish);
        self.rebuild_preview_bindings();
    }

    fn rebuild_preview_bindings(&mut self) {
        if let Some(preview) = &self.preview {
            let develop = self.develop_bind_group(&preview.source_view, &preview.output_view);
            self.preview.as_mut().expect("checked above").develop = develop;
        }
        if let Some(detail) = &self.detail {
            let develop = self.develop_bind_group(&detail.source_view, &detail.output_view);
            self.detail.as_mut().expect("checked above").develop = develop;
        }
    }

    fn blit_bind_group(
        &self,
        output: &wgpu::TextureView,
        warp: &wgpu::TextureView,
        before: &wgpu::TextureView,
        detail: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let texture = |binding, view| wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) };
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &self.blit_layout,
            entries: &[
                texture(0, output),
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: self.blit_buffer.as_entire_binding() },
                texture(3, warp),
                texture(4, before),
                texture(5, detail),
                texture(6, &self.proof_view),
            ],
        })
    }

    fn rebuild_blit(&mut self) {
        let Some(preview) = &self.preview else { return };
        let detail = self.detail.as_ref().map_or(&self.no_detail_view, |detail| &detail.output_view);
        let blit = self.blit_bind_group(&preview.output_view, &preview.warp_view, &preview.before_view, detail);
        self.preview.as_mut().expect("checked above").blit = blit;
    }

    pub fn set_view(&mut self, view: ViewTransform) {
        self.view = view;
    }

    pub fn view(&self) -> ViewTransform {
        self.view
    }

    /// Shows the preview as it would look through a soft proof: a `size`^3 RGBA table,
    /// red fastest, alpha 1 outside the proof's gamut. `None` turns the proof off.
    pub fn set_proof(&mut self, proof: Option<(u32, &[f32])>, gamut_warning: bool) {
        match proof {
            Some((size, rgba)) => {
                self.proof_view = create_proof_view(&self.device, &self.queue, size, rgba);
                self.proof_mode = if gamut_warning { 2.0 } else { 1.0 };
            }
            None => {
                self.proof_view = create_proof_view(&self.device, &self.queue, 2, &IDENTITY_PROOF);
                self.proof_mode = 0.0;
            }
        }
        self.rebuild_blit();
    }

    /// Marks clipped highlights and shadows on the canvas.
    pub fn set_clip_warnings(&mut self, highlights: bool, shadows: bool) {
        self.clip_warnings = [highlights, shadows];
    }

    /// The part of the framed image on a canvas of the given size.
    pub fn visible_area(&self, geometry: &Geometry, target_size: (u32, u32)) -> Option<VisibleArea> {
        let preview = self.preview.as_ref()?;
        Some(VisibleArea::new(letterbox(geometry.output_size(preview.width, preview.height), target_size), self.view))
    }

    pub fn preview_size(&self) -> Option<(u32, u32)> {
        self.preview.as_ref().map(|preview| (preview.width, preview.height))
    }

    /// Whether the full-resolution layer covers a rectangle of source UV.
    pub fn detail_covers(&self, min: [f32; 2], max: [f32; 2]) -> bool {
        self.detail.as_ref().is_some_and(|detail| {
            let [x, y, w, h] = detail.region.map(|value| value as f32);
            let (full_w, full_h) = (detail.full_size[0] as f32, detail.full_size[1] as f32);
            min[0] >= x / full_w && min[1] >= y / full_h && max[0] <= (x + w) / full_w && max[1] <= (y + h) / full_h
        })
    }

    pub fn has_detail(&self) -> bool {
        self.detail.is_some()
    }

    pub fn clear_detail(&mut self) {
        if self.detail.take().is_some() {
            self.rebuild_blit();
        }
    }

    /// Prepares a region of the full-resolution image to be developed with each frame.
    pub fn set_detail(&mut self, image: &LinearImage, region: [u32; 4]) {
        let [x, y, w, h] = region;
        let (source_x, source_y) = (x.saturating_sub(TILE_MARGIN), y.saturating_sub(TILE_MARGIN));
        let source_w = (x + w + TILE_MARGIN).min(image.width) - source_x;
        let source_h = (y + h + TILE_MARGIN).min(image.height) - source_y;
        let source = self.create_source_texture(source_w, source_h);
        self.upload_source(&source, &image.rgba_region(source_x, source_y, source_w, source_h), source_w, source_h);
        let output = self.create_output_texture(w, h);
        let source_view = source.create_view(&Default::default());
        let output_view = output.create_view(&Default::default());
        let develop = self.develop_bind_group(&source_view, &output_view);
        self.detail = Some(Detail {
            region,
            full_size: [image.width, image.height],
            source_view,
            source_offset: [x - source_x, y - source_y],
            source_size: [source_w, source_h],
            output_view,
            develop,
        });
        self.rebuild_blit();
    }

    /// Uploads `proxy` as the image shown by [`Self::draw_preview`] and clears any
    /// mask and skin corrections belonging to the previous image.
    pub fn set_preview(&mut self, proxy: &LinearImage) {
        self.mask_view = create_mask_view(&self.device, &self.queue, 1, 1, &[255]);
        self.skin_smooth_view = create_correction_view(&self.device, &self.queue, 1, 1, &[0; 4]);
        self.skin_blemish_view = create_correction_view(&self.device, &self.queue, 1, 1, &[0; 4]);
        self.local_mask_view = create_local_mask_view(&self.device, &self.queue, 1, 1, &[0; 4]);
        self.build_surround_maps(proxy);
        let (width, height) = (proxy.width, proxy.height);
        let source = self.create_source_texture(width, height);
        self.upload_source(&source, &proxy.rgba_region(0, 0, width, height), width, height);
        let output = self.create_output_texture(width, height);
        let before = self.create_output_texture(width, height);
        let before_view = before.create_view(&Default::default());

        let source_view = source.create_view(&Default::default());
        let output_view = output.create_view(&Default::default());
        let develop = self.develop_bind_group(&source_view, &output_view);
        let empty_field = WarpField::new(width, height);
        let warp = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("warp"),
            size: extent(empty_field.width, empty_field.height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.write_warp(&warp, &empty_field);
        let warp_view = warp.create_view(&Default::default());
        let blit = self.blit_bind_group(&output_view, &warp_view, &before_view, &self.no_detail_view);
        self.detail = None;
        self.preview =
            Some(Preview { width, height, source, source_view, output_view, before_view, before_ready: false, warp, warp_view, develop, blit });
    }

    /// Whether the comparison image has been developed for the current proxy.
    pub fn comparison_ready(&self) -> bool {
        self.preview.as_ref().is_some_and(|preview| preview.before_ready)
    }

    /// Develops the comparison image from the untouched proxy. `current` is what the
    /// source texture should go back to holding, when that is not the same image.
    pub fn prepare_comparison(&mut self, original: &LinearImage, current: Option<&LinearImage>) {
        let Some(preview) = &self.preview else { return };
        let (width, height) = (preview.width, preview.height);
        let (w, h) = (width as f32, height as f32);
        if current.is_some() {
            self.upload_source(&preview.source, &original.rgba_region(0, 0, width, height), width, height);
        }
        self.write_params(&Adjustments::default(), [w, h], [0.0; 2], [w, h], [0.0; 2], [w, h]);
        let bind_group = self.develop_bind_group(&preview.source_view, &preview.before_view);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.dispatch(&mut encoder, &bind_group, width, height);
        self.queue.submit([encoder.finish()]);
        if let Some(image) = current {
            self.upload_source(&preview.source, &image.rgba_region(0, 0, width, height), width, height);
        }
        self.preview.as_mut().expect("checked above").before_ready = true;
    }

    /// Develops the proxy with `adjustments` and draws it, letterboxed, into `target`.
    pub fn draw_preview(
        &self,
        adjustments: &Adjustments,
        geometry: &Geometry,
        target: &wgpu::TextureView,
        target_size: (u32, u32),
        compare: f32,
    ) -> Result<(), GpuError> {
        let preview = self.preview.as_ref().ok_or(GpuError::NoImage)?;
        let (width, height) = (preview.width as f32, preview.height as f32);
        self.write_params(adjustments, [width, height], [0.0; 2], [width, height], [0.0; 2], [width, height]);
        let area = VisibleArea::new(letterbox(geometry.output_size(preview.width, preview.height), target_size), self.view);
        let map = geometry.uv_map(preview.width, preview.height);
        let lens = self.lens_map(adjustments, preview.width, preview.height);
        let detail_rect = self.detail.as_ref().map_or([0.0, 0.0, 1.0, 1.0], |detail| {
            let [x, y, w, h] = detail.region.map(|value| value as f32);
            let (full_w, full_h) = (detail.full_size[0] as f32, detail.full_size[1] as f32);
            [x / full_w, y / full_h, (x + w) / full_w, (y + h) / full_h]
        });
        let blit = BlitParams {
            scale: area.quad,
            uv_origin: map.origin,
            uv_axis_s: map.axis_s,
            uv_axis_t: map.axis_t,
            lens_aspect: lens.aspect,
            lens_scale: lens.scale,
            compare,
            distortion: lens.distortion,
            chroma: lens.chroma,
            perspective: [lens.perspective[0], lens.perspective[1], f32::from(lens.keep_whole), adjustments.post_crop_vignette],
            view: [area.start[0], area.start[1], area.span[0], area.span[1]],
            detail_rect,
            flags: [
                f32::from(self.detail.is_some()),
                f32::from(self.clip_warnings[0]),
                f32::from(self.clip_warnings[1]),
                self.proof_mode,
            ],
        };
        self.queue.write_buffer(&self.blit_buffer, 0, bytes_of(&blit));

        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.dispatch(&mut encoder, &preview.develop, preview.width, preview.height);
        if let Some(detail) = &self.detail {
            // The two passes need different parameters, so they cannot share a submission.
            self.queue.submit([encoder.finish()]);
            let [x, y, w, h] = detail.region;
            self.write_params(
                adjustments,
                detail.full_size.map(|value| value as f32),
                [x as f32, y as f32],
                [w as f32, h as f32],
                detail.source_offset.map(|value| value as f32),
                detail.source_size.map(|value| value as f32),
            );
            encoder = self.device.create_command_encoder(&Default::default());
            self.dispatch(&mut encoder, &detail.develop, w, h);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.06, g: 0.06, b: 0.07, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &preview.blit, &[]);
            pass.draw(0..4, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    pub fn present(&self, frame: wgpu::SurfaceTexture) {
        self.queue.present(frame);
    }

    /// Resolves once all work submitted so far has finished on the GPU.
    pub async fn wait_idle(&self) -> Result<(), GpuError> {
        let (sender, receiver) = oneshot::channel();
        self.queue.on_submitted_work_done(move || {
            let _ = sender.send(());
        });
        self.poll();
        receiver.await.map_err(|_| GpuError::Cancelled)
    }

    /// Develops `image` at full resolution in tiles of at most `tile_size` pixels.
    /// Only one tile is on the GPU at a time, so memory use does not grow with the
    /// image. Returns tightly packed RGB8.
    pub async fn export_rgb8(
        &self,
        image: &LinearImage,
        adjustments: &Adjustments,
        tile_size: u32,
    ) -> Result<Vec<u8>, GpuError> {
        let mut out = vec![0u8; image.width as usize * image.height as usize * 3];
        let stride = image.width as usize * 3;
        self.export_tiles(image, adjustments, tile_size, &self.develop, OUTPUT_FORMAT, OUTPUT_BYTES_PER_PIXEL, |row, x, y| {
            let dst = &mut out[y * stride + x * 3..][..row.len() / 4 * 3];
            for (rgba, rgb) in row.chunks_exact(4).zip(dst.chunks_exact_mut(3)) {
                rgb.copy_from_slice(&rgba[..3]);
            }
        })
        .await?;
        Ok(out)
    }

    /// Like [`Self::export_rgb8`], but with 16 bits per channel, encoded in the colour
    /// space `adjustments.output_space` names. Colours outside sRGB reach a wider space.
    pub async fn export_rgb16(
        &self,
        image: &LinearImage,
        adjustments: &Adjustments,
        tile_size: u32,
    ) -> Result<Vec<u16>, GpuError> {
        let pass = self.deep_develop.get_or_init(|| {
            develop_pass(&self.device, DEEP_OUTPUT_FORMAT, &DEVELOP_SHADER.replace("rgba8unorm", "rgba32float"))
        });
        let mut out = vec![0u16; image.width as usize * image.height as usize * 3];
        let stride = image.width as usize * 3;
        self.export_tiles(image, adjustments, tile_size, pass, DEEP_OUTPUT_FORMAT, DEEP_OUTPUT_BYTES_PER_PIXEL, |row, x, y| {
            let dst = &mut out[y * stride + x * 3..][..row.len() / 16 * 3];
            for (rgba, rgb) in row.chunks_exact(16).zip(dst.chunks_exact_mut(3)) {
                for (bytes, value) in rgba.chunks_exact(4).zip(rgb.iter_mut()) {
                    let sample = f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                    // Unlike the 8-bit target, a float target does not clamp by itself.
                    *value = (sample.clamp(0.0, 1.0) * 65535.0).round() as u16;
                }
            }
        })
        .await?;
        Ok(out)
    }

    /// Runs `pass` over the image tile by tile, calling `store` for each row of output
    /// with its bytes and its position in the image.
    #[allow(clippy::too_many_arguments)]
    async fn export_tiles(
        &self,
        image: &LinearImage,
        adjustments: &Adjustments,
        tile_size: u32,
        pass: &DevelopPass,
        format: wgpu::TextureFormat,
        bytes_per_pixel: u32,
        mut store: impl FnMut(&[u8], usize, usize),
    ) -> Result<(), GpuError> {
        // The tint is an on-screen editing aid and must never reach a delivered file.
        let adjustments = &Adjustments { mask_overlay: 0.0, local_overlay: 0.0, ..*adjustments };
        // Small images, like thumbnails, do not need full-size tile textures.
        let tile_size = tile_size
            .min(image.width.max(image.height))
            .min(self.device.limits().max_texture_dimension_2d - 2 * TILE_MARGIN);
        let source_size = tile_size + 2 * TILE_MARGIN;
        let source = self.create_source_texture(source_size, source_size);
        let output = self.create_output_texture_as(format, tile_size, tile_size);
        let source_view = source.create_view(&Default::default());
        let output_view = output.create_view(&Default::default());
        let bind_group = self.bind_develop(&pass.layout, &source_view, &output_view);

        let row_pitch = align_to(tile_size * bytes_per_pixel, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("export readback"),
            size: u64::from(row_pitch) * u64::from(tile_size),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let (full_w, full_h) = (image.width, image.height);

        for tile_y in (0..full_h).step_by(tile_size as usize) {
            for tile_x in (0..full_w).step_by(tile_size as usize) {
                let w = tile_size.min(full_w - tile_x);
                let h = tile_size.min(full_h - tile_y);

                let (source_x, source_y) = (tile_x.saturating_sub(TILE_MARGIN), tile_y.saturating_sub(TILE_MARGIN));
                let source_w = (tile_x + w + TILE_MARGIN).min(full_w) - source_x;
                let source_h = (tile_y + h + TILE_MARGIN).min(full_h) - source_y;
                let pixels = image.rgba_region(source_x, source_y, source_w, source_h);
                self.upload_source(&source, &pixels, source_w, source_h);
                self.write_params(
                    adjustments,
                    [full_w as f32, full_h as f32],
                    [tile_x as f32, tile_y as f32],
                    [w as f32, h as f32],
                    [(tile_x - source_x) as f32, (tile_y - source_y) as f32],
                    [source_w as f32, source_h as f32],
                );

                let mut encoder = self.device.create_command_encoder(&Default::default());
                self.dispatch_pass(&pass.pipeline, &mut encoder, &bind_group, w, h);
                encoder.copy_texture_to_buffer(
                    output.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &readback,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(row_pitch),
                            rows_per_image: Some(tile_size),
                        },
                    },
                    extent(w, h),
                );
                self.queue.submit([encoder.finish()]);

                self.map_for_reading(&readback).await?;
                {
                    let mapped = readback.slice(..).get_mapped_range()?;
                    for row in 0..h as usize {
                        let src = &mapped[row * row_pitch as usize..][..(w * bytes_per_pixel) as usize];
                        store(src, tile_x as usize, tile_y as usize + row);
                    }
                }
                readback.unmap();
            }
        }
        Ok(())
    }

    fn create_source_texture(&self, width: u32, height: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("source"),
            size: extent(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SOURCE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn create_output_texture(&self, width: u32, height: u32) -> wgpu::Texture {
        self.create_output_texture_as(OUTPUT_FORMAT, width, height)
    }

    fn create_output_texture_as(&self, format: wgpu::TextureFormat, width: u32, height: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("output"),
            size: extent(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    fn upload_source(&self, texture: &wgpu::Texture, rgba: &[u16], width: u32, height: u32) {
        self.queue.write_texture(
            texture.as_image_copy(),
            cast_slice(rgba),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * SOURCE_BYTES_PER_PIXEL),
                rows_per_image: Some(height),
            },
            extent(width, height),
        );
    }

    fn develop_bind_group(
        &self,
        source: &wgpu::TextureView,
        output: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        self.bind_develop(&self.develop.layout, source, output)
    }

    fn bind_develop(
        &self,
        layout: &wgpu::BindGroupLayout,
        source: &wgpu::TextureView,
        output: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("develop"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(output),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.adjust_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.region_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.mask_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&self.blurred_luma_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&self.skin_smooth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(&self.skin_blemish_view),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(&self.local_mask_view),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureView(&self.curve_view),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::TextureView(&self.look_view),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: wgpu::BindingResource::TextureView(&self.detail_luma_view),
                },
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: wgpu::BindingResource::TextureView(&self.haze_view),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: wgpu::BindingResource::TextureView(&self.background_image_view),
                },
                wgpu::BindGroupEntry {
                    binding: 16,
                    resource: wgpu::BindingResource::TextureView(&self.sky_image_view),
                },
                wgpu::BindGroupEntry {
                    binding: 15,
                    resource: wgpu::BindingResource::TextureView(&self.sky_mask_view),
                },
            ],
        })
    }

    fn write_params(
        &self,
        adjustments: &Adjustments,
        full_size: [f32; 2],
        offset: [f32; 2],
        extent: [f32; 2],
        source_offset: [f32; 2],
        source_size: [f32; 2],
    ) {
        self.queue.write_buffer(&self.adjust_buffer, 0, bytes_of(adjustments));
        let lens = self.lens_map(adjustments, full_size[0] as u32, full_size[1] as u32);
        let region = Region {
            full_size,
            offset,
            extent,
            base_exposure: self.base_exposure.unwrap_or(0.0),
            has_base_look: f32::from(u8::from(self.base_exposure.is_some())),
            source_offset,
            source_size,
            lens_vignette: lens.vignette,
            lens_aspect: lens.aspect,
            _padding: [0.0; 2],
            camera: self.camera_rows(adjustments),
            airlight: [self.airlight[0], self.airlight[1], self.airlight[2], 0.0],
            sky_tint: [self.sky_tint[0], self.sky_tint[1], self.sky_tint[2], 0.0],
            output: op_core::OutputSpace::from_shader_code(adjustments.output_space).shader_rows(),
        };
        self.queue.write_buffer(&self.region_buffer, 0, bytes_of(&region));
    }

    /// The camera matrix blended from the identity by `strength`.
    /// The camera match followed by the calibration sliders, as rows of the colour matrix.
    pub fn camera_rows(&self, adjustments: &Adjustments) -> [[f32; 4]; 3] {
        let camera: [[f32; 3]; 3] = std::array::from_fn(|row| {
            std::array::from_fn(|col| {
                let identity = f32::from(u8::from(row == col));
                self.camera_matrix.map_or(identity, |matrix| identity + (matrix[row][col] - identity) * adjustments.camera_profile)
            })
        });
        let calibration = adjustments.calibration_matrix();
        std::array::from_fn(|row| {
            let mut out = [0.0; 4];
            for (col, value) in out.iter_mut().take(3).enumerate() {
                *value = (0..3).map(|middle| calibration[row][middle] * camera[middle][col]).sum();
            }
            out
        })
    }

    fn dispatch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        bind_group: &wgpu::BindGroup,
        width: u32,
        height: u32,
    ) {
        self.dispatch_pass(&self.develop.pipeline, encoder, bind_group, width, height);
    }

    fn dispatch_pass(
        &self,
        pipeline: &wgpu::ComputePipeline,
        encoder: &mut wgpu::CommandEncoder,
        bind_group: &wgpu::BindGroup,
        width: u32,
        height: u32,
    ) {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.dispatch_workgroups(width.div_ceil(WORKGROUP_SIZE), height.div_ceil(WORKGROUP_SIZE), 1);
    }

    async fn map_for_reading(&self, buffer: &wgpu::Buffer) -> Result<(), GpuError> {
        let (sender, receiver) = oneshot::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.poll();
        receiver.await.map_err(|_| GpuError::Cancelled)??;
        Ok(())
    }

    /// Native backends only make progress when polled; in the browser the event
    /// loop drives callbacks and this has nothing to do.
    fn poll(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }
}

/// The develop shader compiled for one output format.
struct DevelopPass {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

fn develop_pass(device: &wgpu::Device, format: wgpu::TextureFormat, shader: &str) -> DevelopPass {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("develop"),
        source: wgpu::ShaderSource::Wgsl(shader.into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("develop"),
        entries: &[
            texture_entry(0, wgpu::ShaderStages::COMPUTE, false),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            },
            uniform_entry(2, wgpu::ShaderStages::COMPUTE),
            uniform_entry(3, wgpu::ShaderStages::COMPUTE),
            texture_entry(4, wgpu::ShaderStages::COMPUTE, true),
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            texture_entry(6, wgpu::ShaderStages::COMPUTE, true),
            texture_entry(7, wgpu::ShaderStages::COMPUTE, true),
            texture_entry(8, wgpu::ShaderStages::COMPUTE, true),
            texture_entry(9, wgpu::ShaderStages::COMPUTE, true),
            texture_entry(10, wgpu::ShaderStages::COMPUTE, true),
            wgpu::BindGroupLayoutEntry {
                binding: 11,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
                count: None,
            },
            texture_entry(12, wgpu::ShaderStages::COMPUTE, true),
            texture_entry(13, wgpu::ShaderStages::COMPUTE, true),
            texture_entry(14, wgpu::ShaderStages::COMPUTE, true),
            texture_entry(15, wgpu::ShaderStages::COMPUTE, true),
            texture_entry(16, wgpu::ShaderStages::COMPUTE, true),
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("develop"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("develop"),
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    DevelopPass { layout, pipeline }
}

/// Encodes tightly packed RGB8 as a baseline JPEG. `quality` is 1..=100; `icc` is the
/// colour profile the samples are in.
pub fn encode_jpeg(rgb: &[u8], width: u32, height: u32, quality: u8, icc: &[u8], exif: Option<Vec<u8>>) -> Result<Vec<u8>, GpuError> {
    let mut out = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut out, quality);
    if let Some(exif) = exif {
        encoder.add_app_segment(1, exif)?;
    }
    encoder.add_icc_profile(icc)?;
    encoder.encode(
        rgb,
        width as u16,
        height as u16,
        jpeg_encoder::ColorType::Rgb,
    )?;
    Ok(out)
}

/// An image's mean colour in linear light, with its luminance divided out and its strength
/// limited, so relighting moves the colour of a photo but never its exposure.
fn neutral_tint(rgba: &[u8]) -> [f32; 3] {
    const LIMITS: (f32, f32) = (0.6, 1.5);
    let linear = |byte: u8| {
        let c = f32::from(byte) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let mut total = [0.0f32; 3];
    for pixel in rgba.chunks_exact(4) {
        for channel in 0..3 {
            total[channel] += linear(pixel[channel]);
        }
    }
    let luma = 0.2126 * total[0] + 0.7152 * total[1] + 0.0722 * total[2];
    if luma <= 0.0 {
        return [1.0; 3];
    }
    total.map(|value| (value / luma).clamp(LIMITS.0, LIMITS.1))
}

fn create_replace_view(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32, rgba: &[u8]) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("replacement"),
        size: extent(width, height),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        extent(width, height),
    );
    texture.create_view(&Default::default())
}

fn create_mask_view(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    mask: &[u8],
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("subject mask"),
        size: extent(width, height),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        mask,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width),
            rows_per_image: Some(height),
        },
        extent(width, height),
    );
    texture.create_view(&Default::default())
}

fn create_luma_view(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    luma: &[u16],
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("blurred luma"),
        size: extent(width, height),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        cast_slice(luma),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 2),
            rows_per_image: Some(height),
        },
        extent(width, height),
    );
    texture.create_view(&Default::default())
}

const IDENTITY_LOOK: [f32; 24] = [
    0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0,
];

const IDENTITY_PROOF: [f32; 32] = [
    0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0,
];

fn create_proof_view(device: &wgpu::Device, queue: &wgpu::Queue, size: u32, rgba: &[f32]) -> wgpu::TextureView {
    let dimensions = wgpu::Extent3d { width: size, height: size, depth_or_array_layers: size };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("proof"),
        size: dimensions,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: SOURCE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bits: Vec<u16> = rgba.iter().map(|&v| half::f16::from_f32(v).to_bits()).collect();
    queue.write_texture(
        texture.as_image_copy(),
        cast_slice(&bits),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * SOURCE_BYTES_PER_PIXEL), rows_per_image: Some(size) },
        dimensions,
    );
    texture.create_view(&Default::default())
}

fn create_look_view(device: &wgpu::Device, queue: &wgpu::Queue, size: u32, rgb: &[f32]) -> wgpu::TextureView {
    let dimensions = wgpu::Extent3d { width: size, height: size, depth_or_array_layers: size };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("look"),
        size: dimensions,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: SOURCE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let one = half::f16::from_f32(1.0).to_bits();
    let rgba: Vec<u16> = rgb
        .chunks_exact(3)
        .flat_map(|c| [half::f16::from_f32(c[0]).to_bits(), half::f16::from_f32(c[1]).to_bits(), half::f16::from_f32(c[2]).to_bits(), one])
        .collect();
    queue.write_texture(
        texture.as_image_copy(),
        cast_slice(&rgba),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * SOURCE_BYTES_PER_PIXEL),
            rows_per_image: Some(size),
        },
        dimensions,
    );
    texture.create_view(&Default::default())
}

fn create_local_mask_view(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("local mask"),
        size: extent(width, height),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        extent(width, height),
    );
    texture.create_view(&Default::default())
}

fn create_correction_view(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    rgba: &[u16],
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("skin correction"),
        size: extent(width, height),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SOURCE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        cast_slice(rgba),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * SOURCE_BYTES_PER_PIXEL),
            rows_per_image: Some(height),
        },
        extent(width, height),
    );
    texture.create_view(&Default::default())
}

fn letterbox(image: (u32, u32), target: (u32, u32)) -> [f32; 2] {
    let image_aspect = image.0 as f32 / image.1.max(1) as f32;
    let target_aspect = target.0 as f32 / target.1.max(1) as f32;
    if target_aspect > image_aspect {
        [image_aspect / target_aspect, 1.0]
    } else {
        [1.0, target_aspect / image_aspect]
    }
}

fn extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d { width, height, depth_or_array_layers: 1 }
}

fn uniform_buffer(device: &wgpu::Device, label: &str, size: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn texture_entry(binding: u32, visibility: wgpu::ShaderStages, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use half::f16;

    fn renderer() -> Renderer {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&Default::default())
                .await
                .expect("a GPU adapter is required for these tests");
            let (device, queue) = adapter
                .request_device(&Default::default())
                .await
                .expect("device");
            Renderer::new(device, queue, wgpu::TextureFormat::Rgba8Unorm)
        })
    }

    /// A horizontal and vertical ramp, so any tile misplacement changes the output.
    fn ramp(width: u32, height: u32) -> LinearImage {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 3);
        for y in 0..height {
            for x in 0..width {
                pixels.push(f16::from_f32(x as f32 / width as f32).to_bits());
                pixels.push(f16::from_f32(y as f32 / height as f32).to_bits());
                pixels.push(f16::from_f32(0.25).to_bits());
            }
        }
        LinearImage::new(width, height, pixels)
    }

    fn export(renderer: &Renderer, image: &LinearImage, adj: &Adjustments, tile: u32) -> Vec<u8> {
        pollster::block_on(renderer.export_rgb8(image, adj, tile)).unwrap()
    }

    #[test]
    fn tiled_export_matches_untiled_export_exactly() {
        let renderer = renderer();
        let image = ramp(300, 170);
        let adj = Adjustments { exposure: 0.4, contrast: 0.3, vignette: -0.6, ..Default::default() };
        assert_eq!(export(&renderer, &image, &adj, 64), export(&renderer, &image, &adj, 512));
    }

    #[test]
    fn neutral_settings_are_the_srgb_encoding_of_the_source() {
        let renderer = renderer();
        let bits = f16::from_f32(0.18).to_bits();
        let image = LinearImage::new(4, 4, vec![bits; 48]);
        let out = export(&renderer, &image, &Adjustments::default(), 64);
        // sRGB(0.18) = 117.9
        for value in out {
            assert!((value as i32 - 118).abs() <= 1, "got {value}");
        }
    }

    #[test]
    fn one_stop_of_exposure_doubles_linear_light() {
        let renderer = renderer();
        let bits = f16::from_f32(0.18).to_bits();
        let image = LinearImage::new(4, 4, vec![bits; 48]);
        let adj = Adjustments { exposure: 1.0, ..Default::default() };
        let out = export(&renderer, &image, &adj, 64);
        // sRGB(0.36) = 160.6
        assert!((out[0] as i32 - 161).abs() <= 1, "got {}", out[0]);
    }

    #[test]
    fn positive_saturation_widens_channel_spread() {
        let renderer = renderer();
        let (r, g, b) = (f16::from_f32(0.4).to_bits(), f16::from_f32(0.2).to_bits(), f16::from_f32(0.1).to_bits());
        let image = LinearImage::new(1, 1, vec![r, g, b]);
        let base = export(&renderer, &image, &Adjustments::default(), 64);
        let adj = Adjustments { saturation: 0.5, ..Default::default() };
        let vivid = export(&renderer, &image, &adj, 64);
        assert!(vivid[0] as i32 - vivid[2] as i32 > base[0] as i32 - base[2] as i32);
    }

    fn flat_gray(width: u32, height: u32) -> LinearImage {
        let bits = f16::from_f32(0.18).to_bits();
        LinearImage::new(width, height, vec![bits; width as usize * height as usize * 3])
    }

    #[test]
    fn background_exposure_only_changes_the_background() {
        let mut renderer = renderer();
        renderer.set_mask(2, 1, &[255, 0]);
        let adj = Adjustments { background_exposure: 1.0, ..Default::default() };
        let out = export(&renderer, &flat_gray(2, 1), &adj, 64);
        // Left is subject, sRGB(0.18) = 117.9. Right is background, sRGB(0.36) = 160.6.
        assert!((out[0] as i32 - 118).abs() <= 1, "subject got {}", out[0]);
        assert!((out[3] as i32 - 161).abs() <= 1, "background got {}", out[3]);
    }

    #[test]
    fn without_a_mask_everything_is_subject() {
        let renderer = renderer();
        let adj = Adjustments { background_exposure: 1.0, ..Default::default() };
        let out = export(&renderer, &flat_gray(4, 4), &adj, 64);
        assert!((out[0] as i32 - 118).abs() <= 1, "got {}", out[0]);
    }

    #[test]
    fn tiled_export_matches_untiled_export_with_a_mask() {
        let mut renderer = renderer();
        let mask: Vec<u8> = (0..64u32 * 64).map(|i| (i % 251) as u8).collect();
        renderer.set_mask(64, 64, &mask);
        let image = ramp(300, 170);
        let adj = Adjustments { background_exposure: -1.2, ..Default::default() };
        assert_eq!(export(&renderer, &image, &adj, 64), export(&renderer, &image, &adj, 512));
    }

    #[test]
    fn the_mask_overlay_never_reaches_an_export() {
        let mut renderer = renderer();
        renderer.set_mask(1, 1, &[0]);
        let image = flat_gray(4, 4);
        let plain = export(&renderer, &image, &Adjustments::default(), 64);
        let tinted = Adjustments { mask_overlay: 1.0, ..Default::default() };
        assert_eq!(plain, export(&renderer, &image, &tinted, 64));
    }

    /// A shallow ramp, which 8 bits can only show as a few visible steps.
    fn shallow_ramp() -> LinearImage {
        let (width, height) = (1024u32, 8u32);
        let mut pixels = Vec::new();
        for _ in 0..height {
            for x in 0..width {
                pixels.extend([f16::from_f32(0.18 + 0.02 * x as f32 / width as f32).to_bits(); 3]);
            }
        }
        LinearImage::new(width, height, pixels)
    }

    #[test]
    fn replacing_the_background_keeps_the_subject_and_fills_the_rest() {
        let mut renderer = renderer();
        renderer.set_mask(2, 1, &[255, 0]);
        renderer.set_background_image(1, 1, &[188, 188, 188, 255]);
        let image = flat_gray(64, 16);
        let adj = Adjustments { replace_background: 1.0, ..Default::default() };
        let out = export(&renderer, &image, &adj, 64);
        let at = |x: usize| out[(8 * 64 + x) * 3];
        assert_eq!(at(60), 188, "background");
        assert!(at(2).abs_diff(export(&renderer, &image, &Adjustments::default(), 64)[2 * 3]) <= 1, "subject");
    }

    #[test]
    fn replacing_the_sky_follows_its_own_mask_and_keeps_tiles_identical() {
        let mut renderer = renderer();
        renderer.set_sky_mask(1, 2, &[255, 0]);
        renderer.set_sky_image(2, 2, &[230, 40, 40, 255, 40, 40, 230, 255, 230, 40, 40, 255, 40, 40, 230, 255]);
        let image = flat_gray(64, 64);
        let adj = Adjustments { replace_sky: 1.0, ..Default::default() };
        let out = export(&renderer, &image, &adj, 512);
        let plain = export(&renderer, &image, &Adjustments::default(), 512);
        assert!(out[(2 * 64 + 4) * 3].abs_diff(230) <= 8, "sky takes the replacement");
        assert_eq!(out[(61 * 64 + 4) * 3], plain[(61 * 64 + 4) * 3], "the ground is untouched");
        assert_eq!(out, export(&renderer, &image, &adj, 16));
    }

    #[test]
    fn the_sky_and_background_can_be_replaced_together() {
        let mut renderer = renderer();
        renderer.set_mask(2, 1, &[255, 0]);
        renderer.set_sky_mask(1, 2, &[255, 0]);
        renderer.set_background_image(1, 1, &[60, 60, 60, 255]);
        renderer.set_sky_image(1, 1, &[220, 220, 220, 255]);
        let image = flat_gray(64, 64);
        let adj = Adjustments { replace_background: 1.0, replace_sky: 1.0, ..Default::default() };
        let out = export(&renderer, &image, &adj, 64);
        let plain = export(&renderer, &image, &Adjustments::default(), 64);
        assert!(out[(4 * 64 + 60) * 3].abs_diff(220) <= 3, "background side of the sky");
        assert!(out[(60 * 64 + 60) * 3].abs_diff(60) <= 3, "background below the sky");
        assert_eq!(out[(60 * 64 + 2) * 3], plain[(60 * 64 + 2) * 3], "the subject");
    }

    #[test]
    fn relighting_shifts_the_ground_towards_the_sky_without_changing_its_brightness() {
        let mut renderer = renderer();
        renderer.set_sky_mask(1, 2, &[255, 0]);
        renderer.set_sky_image(1, 1, &[255, 150, 90, 255]);
        let image = flat_gray(32, 32);
        let adj = Adjustments { replace_sky: 1.0, replace_sky_relight: 1.0, ..Default::default() };
        let out = export(&renderer, &image, &adj, 64);
        let plain = export(&renderer, &image, &Adjustments::default(), 64);
        let at = (30 * 32 + 8) * 3;
        assert!(out[at] > plain[at] && out[at + 2] < plain[at + 2], "warmer: {:?} against {:?}", &out[at..at + 3], &plain[at..at + 3]);
    }

    #[test]
    fn a_replacement_fills_the_frame_without_stretching() {
        let mut renderer = renderer();
        renderer.set_mask(1, 1, &[0]);
        // A wide image, of which a square frame only shows the middle half.
        let mut rgba = Vec::new();
        for x in 0..4 {
            rgba.extend(if x == 1 || x == 2 { [200, 200, 200, 255] } else { [20, 20, 20, 255] });
        }
        renderer.set_background_image(4, 1, &rgba);
        let image = flat_gray(32, 32);
        let adj = Adjustments { replace_background: 1.0, ..Default::default() };
        let out = export(&renderer, &image, &adj, 64);
        assert!(out[(16 * 32 + 2) * 3] > 150 && out[(16 * 32 + 29) * 3] > 150, "only the middle shows");
    }

    #[test]
    fn a_picked_colour_shifts_and_other_colours_do_not() {
        let renderer = renderer();
        // Left half red-orange, right half blue.
        let mut pixels = Vec::new();
        for _ in 0..16 {
            for x in 0..32 {
                let colour = if x < 16 { [0.6f32, 0.15, 0.05] } else { [0.05f32, 0.1, 0.6] };
                pixels.extend(colour.map(|v| f16::from_f32(v).to_bits()));
            }
        }
        let image = LinearImage::new(32, 16, pixels);
        let adj = Adjustments { point_hue: 15.0, point_width: 30.0, point_sat_shift: -1.0, ..Default::default() };
        let out = export(&renderer, &image, &adj, 64);
        let plain = export(&renderer, &image, &Adjustments::default(), 64);
        let at = |buffer: &[u8], x: usize| [buffer[(8 * 32 + x) * 3], buffer[(8 * 32 + x) * 3 + 1], buffer[(8 * 32 + x) * 3 + 2]];
        let [r, g, b] = at(&out, 4);
        assert!(r.abs_diff(g) <= 2 && g.abs_diff(b) <= 2, "the picked colour lost its saturation: {r} {g} {b}");
        assert_eq!(at(&out, 28), at(&plain, 28), "blue is not touched");
    }

    #[test]
    fn a_larger_sharpening_radius_reaches_further_from_an_edge() {
        let renderer = renderer();
        let bits = |v: f32| f16::from_f32(v).to_bits();
        let mut pixels = Vec::new();
        for _ in 0..16 {
            for x in 0..32 {
                pixels.extend([bits(if x < 16 { 0.1 } else { 0.5 }); 3]);
            }
        }
        let image = LinearImage::new(32, 16, pixels);
        let near = export(&renderer, &image, &Adjustments { sharpen: 1.0, ..Default::default() }, 64);
        let far = export(&renderer, &image, &Adjustments { sharpen: 1.0, sharpen_radius: 3.0, ..Default::default() }, 64);
        let row = |buffer: &[u8], x: usize| buffer[(8 * 32 + x) * 3];
        // Two pixels from the edge: outside a radius of 1 and inside a radius of 3.
        assert_eq!(row(&near, 18), export(&renderer, &image, &Adjustments::default(), 64)[(8 * 32 + 18) * 3]);
        assert!(row(&far, 18) > row(&near, 18), "{} against {}", row(&far, 18), row(&near, 18));
    }

    #[test]
    fn masking_leaves_gentle_texture_unsharpened() {
        let renderer = renderer();
        let bits = |v: f32| f16::from_f32(v).to_bits();
        let mut pixels = Vec::new();
        for _ in 0..16 {
            for x in 0..32 {
                pixels.extend([bits(if x % 2 == 0 { 0.4 } else { 0.402 }); 3]);
            }
        }
        let image = LinearImage::new(32, 16, pixels);
        let plain = export(&renderer, &image, &Adjustments::default(), 64);
        let masked = export(&renderer, &image, &Adjustments { sharpen: 1.0, sharpen_masking: 1.0, ..Default::default() }, 64);
        assert!(plain.iter().zip(&masked).all(|(a, b)| a.abs_diff(*b) <= 1));
    }

    #[test]
    fn a_sixteen_bit_export_matches_eight_bit_with_finer_steps() {
        let renderer = renderer();
        let image = shallow_ramp();
        let adj = Adjustments { exposure: 0.2, contrast: 0.2, ..Default::default() };
        let shallow = pollster::block_on(renderer.export_rgb8(&image, &adj, 512)).unwrap();
        let deep = pollster::block_on(renderer.export_rgb16(&image, &adj, 512)).unwrap();
        assert_eq!(deep.len(), shallow.len());
        for (index, (&d, &s)) in deep.iter().zip(&shallow).enumerate() {
            assert!((f32::from(d) / 257.0 - f32::from(s)).abs() <= 0.51, "sample {index}: {d} against {s}");
        }
        let levels = |row: Vec<u32>| row.windows(2).filter(|pair| pair[0] != pair[1]).count() + 1;
        let shallow_levels = levels(shallow[..1024 * 3].iter().step_by(3).map(|&v| u32::from(v)).collect());
        let deep_levels = levels(deep[..1024 * 3].iter().step_by(3).map(|&v| u32::from(v)).collect());
        assert!(deep_levels > shallow_levels * 5, "{deep_levels} levels against {shallow_levels}");
    }

    #[test]
    fn sixteen_bit_tiles_are_identical_to_one_pass() {
        let renderer = renderer();
        let image = ramp(300, 170);
        let adj = Adjustments { exposure: 0.4, contrast: 0.3, vignette: -0.6, ..Default::default() };
        let tiled = pollster::block_on(renderer.export_rgb16(&image, &adj, 64)).unwrap();
        let whole = pollster::block_on(renderer.export_rgb16(&image, &adj, 512)).unwrap();
        assert_eq!(tiled, whole);
    }

    fn export_colour16(renderer: &Renderer, rgb: [f32; 3], adj: &Adjustments) -> [u16; 3] {
        let image = LinearImage::new(1, 1, rgb.map(|v| f16::from_f32(v).to_bits()).to_vec());
        let out = pollster::block_on(renderer.export_rgb16(&image, adj, 64)).unwrap();
        [out[0], out[1], out[2]]
    }

    #[test]
    fn colours_outside_srgb_reach_a_wide_export() {
        let renderer = renderer();
        // Display P3's red, in linear sRGB.
        let p3_red = [1.2249, -0.0421, -0.0196];
        let p3 = Adjustments { output_space: op_core::OutputSpace::P3.shader_code(), ..Default::default() };
        let [r, g, b] = export_colour16(&renderer, p3_red, &p3);
        assert!(r > 65000 && g < 600 && b < 600, "P3 red in a P3 export: {r} {g} {b}");
        // In sRGB it can only be clipped.
        let [r, g, b] = export_colour16(&renderer, p3_red, &Adjustments::default());
        assert_eq!((r, g, b), (65535, 0, 0));
        // Edits on 0..1, such as a look or curve, keep it outside sRGB.
        let edited = Adjustments { contrast: 0.2, saturation: -0.1, ..p3 };
        let [r, g, _] = export_colour16(&renderer, p3_red, &edited);
        let srgb_red_in_p3 = export_colour16(&renderer, [1.0, 0.0, 0.0], &edited);
        assert!(r > srgb_red_in_p3[0] && g < srgb_red_in_p3[1], "{r} {g} vs {srgb_red_in_p3:?}");
    }

    #[test]
    fn noise_below_black_stays_black() {
        let renderer = renderer();
        let lifted = Adjustments { blacks: 0.6, ..Default::default() };
        let noise = export_colour16(&renderer, [-0.01, -0.02, -0.01], &lifted);
        let black = export_colour16(&renderer, [0.0, 0.0, 0.0], &lifted);
        for c in 0..3 {
            assert!(noise[c].abs_diff(black[c]) < 700, "{noise:?} vs {black:?}");
        }
    }

    fn export_gray(renderer: &Renderer, value: f32) -> u8 {
        let bits = f16::from_f32(value).to_bits();
        let image = LinearImage::new(1, 1, vec![bits; 3]);
        export(renderer, &image, &Adjustments::default(), 64)[0]
    }

    fn checker(width: u32, height: u32) -> LinearImage {
        let (dark, bright) = (f16::from_f32(0.1).to_bits(), f16::from_f32(0.4).to_bits());
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 3);
        for y in 0..height {
            for x in 0..width {
                let value = if (x + y) % 2 == 0 { dark } else { bright };
                pixels.extend([value, value, value]);
            }
        }
        LinearImage::new(width, height, pixels)
    }

    #[test]
    fn tiles_stay_identical_with_spatial_adjustments() {
        let mut renderer = renderer();
        let image = checker(300, 170);
        renderer.set_preview(&image);
        let adj = Adjustments { sharpen: 0.6, clarity: 0.5, ..Default::default() };
        assert_eq!(export(&renderer, &image, &adj, 64), export(&renderer, &image, &adj, 512));
    }

    #[test]
    fn sharpen_raises_edge_contrast() {
        let renderer = renderer();
        let image = checker(32, 32);
        let flat = export(&renderer, &image, &Adjustments::default(), 64);
        let adj = Adjustments { sharpen: 0.5, ..Default::default() };
        let sharp = export(&renderer, &image, &adj, 64);
        let spread = |out: &[u8]| i32::from(out[3 * 16 * 32 + 3 * 17]) - i32::from(out[3 * 16 * 32 + 3 * 16]);
        assert!(spread(&sharp).abs() > spread(&flat).abs());
    }

    #[test]
    fn hsl_only_changes_the_targeted_band() {
        let renderer = renderer();
        let pixel = |r: f32, g: f32, b: f32| {
            LinearImage::new(1, 1, vec![f16::from_f32(r).to_bits(), f16::from_f32(g).to_bits(), f16::from_f32(b).to_bits()])
        };
        // Band 0 is red; desaturate it fully.
        let adj = Adjustments { hsl_saturation: [[-1.0, 0.0, 0.0, 0.0], [0.0; 4]], ..Default::default() };
        let red = export(&renderer, &pixel(0.6, 0.05, 0.05), &adj, 64);
        assert!((i32::from(red[0]) - i32::from(red[1])).abs() <= 2, "red should turn grey: {red:?}");
        let blue_plain = export(&renderer, &pixel(0.05, 0.05, 0.6), &Adjustments::default(), 64);
        let blue = export(&renderer, &pixel(0.05, 0.05, 0.6), &adj, 64);
        assert_eq!(blue_plain, blue, "blue is outside the red band");
    }

    fn one_stop_correction(width: u32, height: u32) -> SkinRetouch {
        let up = f16::from_f32(1.0).to_bits();
        let count = width as usize * height as usize;
        SkinRetouch {
            width,
            height,
            smooth: [up, up, up, 0].repeat(count),
            blemish: vec![0; count * 4],
        }
    }

    #[test]
    fn skin_corrections_scale_with_their_slider() {
        let mut renderer = renderer();
        let image = flat_gray(4, 4);
        renderer.set_skin_retouch(&one_stop_correction(4, 4));
        let off = export(&renderer, &image, &Adjustments::default(), 64);
        let full = export(&renderer, &image, &Adjustments { skin_smooth: 1.0, ..Default::default() }, 64);
        // sRGB(0.18) = 117.9, sRGB(0.36) = 160.6.
        assert!((i32::from(off[0]) - 118).abs() <= 1, "got {}", off[0]);
        assert!((i32::from(full[0]) - 161).abs() <= 1, "got {}", full[0]);
    }

    #[test]
    fn a_new_image_clears_skin_corrections() {
        let mut renderer = renderer();
        let image = flat_gray(4, 4);
        renderer.set_skin_retouch(&one_stop_correction(4, 4));
        renderer.set_preview(&image);
        let adj = Adjustments { skin_smooth: 1.0, ..Default::default() };
        let out = export(&renderer, &image, &adj, 64);
        assert!((i32::from(out[0]) - 118).abs() <= 1, "got {}", out[0]);
    }

    #[test]
    fn a_local_layer_only_affects_its_masked_area() {
        let mut renderer = renderer();
        // Left pixel is in layer 0, right pixel is not.
        renderer.set_local_mask(2, 1, &[255, 0, 0, 0, 0, 0, 0, 0]);
        let mut adj = Adjustments::default();
        adj.local[0][0] = 1.0;
        let out = export(&renderer, &flat_gray(2, 1), &adj, 64);
        assert!((i32::from(out[0]) - 161).abs() <= 1, "masked got {}", out[0]);
        assert!((i32::from(out[3]) - 118).abs() <= 1, "unmasked got {}", out[3]);
    }

    #[test]
    fn local_layers_use_separate_channels() {
        let mut renderer = renderer();
        renderer.set_local_mask(1, 1, &[0, 255, 0, 0]);
        let mut adj = Adjustments::default();
        adj.local[0][0] = 2.0;
        adj.local[1][0] = 1.0;
        let out = export(&renderer, &flat_gray(1, 1), &adj, 64);
        assert!((i32::from(out[0]) - 161).abs() <= 1, "got {}", out[0]);
    }

    #[test]
    fn a_new_image_clears_local_masks() {
        let mut renderer = renderer();
        renderer.set_local_mask(1, 1, &[255, 0, 0, 0]);
        let image = flat_gray(4, 4);
        renderer.set_preview(&image);
        let mut adj = Adjustments::default();
        adj.local[0][0] = 1.0;
        let out = export(&renderer, &image, &adj, 64);
        assert!((i32::from(out[0]) - 118).abs() <= 1, "got {}", out[0]);
    }

    #[test]
    fn base_look_maps_sensor_range_to_display_range() {
        let mut renderer = renderer();
        renderer.set_base_exposure(Some(1.2));
        assert_eq!(export_gray(&renderer, 0.0), 0);
        assert_eq!(export_gray(&renderer, 1.0), 255);
    }

    #[test]
    fn blacks_lift_pure_black_and_stay_continuous_with_the_next_level() {
        let renderer = renderer();
        let bits = |v: f32| f16::from_f32(v).to_bits();
        let mut pixels = Vec::new();
        for _ in 0..8 {
            for x in 0..16 {
                pixels.extend([bits(if x < 8 { 0.0 } else { 0.0003 }); 3]);
            }
        }
        let image = LinearImage::new(16, 8, pixels);
        let out = export(&renderer, &image, &Adjustments { blacks: 1.0, ..Default::default() }, 64);
        let (black, next) = (out[(4 * 16 + 2) * 3], out[(4 * 16 + 12) * 3]);
        assert!(black > 10, "pure black is lifted: {black}");
        // Before, pure black stayed at 0 while this level rose to about 31.
        assert!(next >= black && next - black <= 6, "and lands just below the level above it: {black} vs {next}");
    }

    #[test]
    fn clarity_does_not_darken_highlights_brighter_than_white() {
        let renderer = renderer();
        let bits = |v: f32| f16::from_f32(v).to_bits();
        let mut pixels = Vec::new();
        for _ in 0..32 {
            for x in 0..32 {
                pixels.extend([bits(if (12..20).contains(&x) { 1.0 } else { 0.1 }); 3]);
            }
        }
        let image = LinearImage::new(32, 32, pixels);
        let mut renderer = renderer;
        renderer.set_preview(&image);
        let adj = Adjustments { exposure: 1.0, clarity: 1.0, ..Default::default() };
        let out = export(&renderer, &image, &adj, 64);
        assert_eq!(out[(16 * 32 + 16) * 3], 255, "a highlight pushed past white stays white");
    }

    #[test]
    fn whites_lift_bright_tones_most_and_keep_their_order() {
        const VALUES: [f32; 6] = [0.02, 0.2, 0.4, 0.6, 0.75, 0.85];
        const COLUMN: u32 = 8;
        let renderer = renderer();
        let columns = VALUES.len() as u32 * COLUMN;
        let mut pixels = Vec::new();
        for _ in 0..COLUMN {
            for x in 0..columns {
                pixels.extend([f16::from_f32(VALUES[(x / COLUMN) as usize]).to_bits(); 3]);
            }
        }
        let image = LinearImage::new(columns, COLUMN, pixels);
        let levels = |whites: f32| -> Vec<i32> {
            let out = export(&renderer, &image, &Adjustments { whites, ..Default::default() }, 64);
            (0..VALUES.len()).map(|column| i32::from(out[(4 * columns as usize + column * COLUMN as usize + 4) * 3])).collect()
        };
        let plain = levels(0.0);
        let lifted = levels(1.0);
        let lowered = levels(-1.0);
        let gain: Vec<i32> = lifted.iter().zip(&plain).map(|(a, b)| a - b).collect();
        assert_eq!(gain[0], 0, "deep shadows are left alone: {gain:?}");
        assert!(gain[3] > gain[2] && gain[2] >= gain[1], "the effect grows towards white: {gain:?}");
        assert!(gain[3] >= 8, "a full move is clearly visible: {gain:?}");
        for curve in [&lifted, &lowered] {
            // Only tones already pushed to white may land on the same level.
            assert!(curve.windows(2).all(|pair| pair[0] < pair[1] || pair[1] == 255), "tones stay in order: {curve:?}");
        }
    }

    #[test]
    fn base_look_is_monotonic_and_brightens_midtones() {
        let mut renderer = renderer();
        let plain = export_gray(&renderer, 0.09);
        renderer.set_base_exposure(Some(1.2));
        let values: Vec<u8> = [0.01, 0.05, 0.09, 0.2, 0.5, 0.9].map(|v| export_gray(&renderer, v)).to_vec();
        assert!(values.windows(2).all(|pair| pair[0] < pair[1]), "{values:?}");
        assert!(values[2] > plain, "{} should be brighter than {plain}", values[2]);
    }

    #[test]
    fn base_look_keeps_tiles_consistent() {
        let mut renderer = renderer();
        renderer.set_base_exposure(Some(0.8));
        let image = ramp(300, 170);
        let adj = Adjustments { exposure: 0.2, ..Default::default() };
        assert_eq!(export(&renderer, &image, &adj, 64), export(&renderer, &image, &adj, 512));
    }

    #[test]
    fn the_camera_matrix_scales_with_its_strength() {
        let mut renderer = renderer();
        let swap = [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
        renderer.set_camera_data(None, Some(swap));
        let image = LinearImage::new(1, 1, [0.5f32, 0.2, 0.05].map(|v| f16::from_f32(v).to_bits()).to_vec());
        let plain = export(&renderer, &image, &Adjustments::default(), 64);
        let full = export(&renderer, &image, &Adjustments { camera_profile: 1.0, ..Default::default() }, 64);
        assert_eq!((plain[0], plain[2]), (full[2], full[0]), "red and blue swap");
        let half = export(&renderer, &image, &Adjustments { camera_profile: 0.5, ..Default::default() }, 64);
        assert!(half[0] < plain[0] && half[0] > full[0], "{half:?}");
    }

    #[test]
    fn lens_vignetting_brightens_the_corners_only() {
        let renderer = renderer();
        let adj = Adjustments { lens_manual: [0.0, 1.0, 0.0, 0.0], ..Default::default() };
        let plain = export(&renderer, &flat_gray(101, 101), &Adjustments::default(), 64);
        let lifted = export(&renderer, &flat_gray(101, 101), &adj, 64);
        let centre = (50 * 101 + 50) * 3;
        assert!(i32::from(lifted[centre]) - i32::from(plain[centre]) <= 1, "centre moved");
        assert!(lifted[0] > plain[0] + 25, "corner {} -> {}", plain[0], lifted[0]);
    }

    #[test]
    fn lens_vignetting_is_seamless_across_tiles() {
        let renderer = renderer();
        let adj = Adjustments { lens_manual: [0.0, 0.8, 0.0, 0.0], ..Default::default() };
        assert_eq!(export(&renderer, &ramp(300, 170), &adj, 64), export(&renderer, &ramp(300, 170), &adj, 512));
    }

    #[test]
    fn canvas_positions_map_through_the_crop() {
        let mut renderer = renderer();
        renderer.set_preview(&flat_gray(100, 100));
        let geometry = Geometry { crop_x: 0.5, crop_y: 0.5, crop_w: 0.5, crop_h: 0.5, ..Default::default() };
        let centre = renderer.canvas_to_image(&geometry, (200, 200), 0.5, 0.5).unwrap();
        assert!((centre.0 - 0.75).abs() < 1e-4 && (centre.1 - 0.75).abs() < 1e-4, "{centre:?}");
    }

    #[test]
    fn positions_in_the_letterbox_bars_are_outside_the_image() {
        let mut renderer = renderer();
        renderer.set_preview(&flat_gray(100, 100));
        assert_eq!(renderer.canvas_to_image(&Geometry::default(), (200, 100), 0.05, 0.5), None);
    }

    fn noisy(width: u32, height: u32) -> LinearImage {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let wave = ((x * 7 + y * 13) % 11) as f32 / 11.0 - 0.5;
                pixels.extend([0.2 + wave * 0.05, 0.2 - wave * 0.05, 0.2 + wave * 0.03].map(|v| f16::from_f32(v).to_bits()));
            }
        }
        LinearImage::new(width, height, pixels)
    }

    fn spread(rgb: &[u8]) -> f32 {
        let mean = rgb.iter().map(|&v| f32::from(v)).sum::<f32>() / rgb.len() as f32;
        (rgb.iter().map(|&v| (f32::from(v) - mean).powi(2)).sum::<f32>() / rgb.len() as f32).sqrt()
    }

    #[test]
    fn denoising_reduces_variation() {
        let renderer = renderer();
        let image = noisy(64, 64);
        let plain = export(&renderer, &image, &Adjustments::default(), 128);
        let adj = Adjustments { denoise_luma: 1.0, denoise_chroma: 1.0, ..Default::default() };
        let clean = export(&renderer, &image, &adj, 128);
        assert!(spread(&clean) < spread(&plain) * 0.7, "{} vs {}", spread(&clean), spread(&plain));
    }

    #[test]
    fn denoising_keeps_strong_edges() {
        let renderer = renderer();
        let (dark, bright) = (f16::from_f32(0.02).to_bits(), f16::from_f32(0.8).to_bits());
        let mut pixels = Vec::new();
        for _ in 0..32 {
            for x in 0..64 {
                pixels.extend([if x < 32 { dark } else { bright }; 3]);
            }
        }
        let image = LinearImage::new(64, 32, pixels);
        let adj = Adjustments { denoise_luma: 0.5, ..Default::default() };
        let out = export(&renderer, &image, &adj, 128);
        let at = |x: usize| i32::from(out[(16 * 64 + x) * 3]);
        assert!(at(34) - at(29) > 150, "edge blurred: {} to {}", at(29), at(34));
    }

    #[test]
    fn tiles_stay_identical_when_denoising() {
        let mut renderer = renderer();
        let image = noisy(300, 170);
        renderer.set_preview(&image);
        let adj = Adjustments { denoise_luma: 0.7, denoise_chroma: 0.7, sharpen: 0.3, ..Default::default() };
        assert_eq!(export(&renderer, &image, &adj, 64), export(&renderer, &image, &adj, 512));
    }

    #[test]
    fn a_curve_changes_the_output() {
        let renderer = renderer();
        let image = flat_gray(4, 4);
        let mut curve = ToneCurve::default();
        curve.curve[0] = vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
        renderer.set_curve(&curve);
        let adj = Adjustments { curve_active: 1.0, ..Default::default() };
        let out = export(&renderer, &image, &adj, 64);
        assert!(i32::from(out[0]) > 135, "got {}", out[0]);
    }

    #[test]
    fn grading_tints_shadows_without_touching_highlights() {
        let renderer = renderer();
        let mut adj = Adjustments::default();
        adj.grade[0] = [240.0, 1.0, 0.0, 0.0];
        let shadow = export(&renderer, &LinearImage::new(1, 1, vec![f16::from_f32(0.02).to_bits(); 3]), &adj, 64);
        assert!(shadow[2] > shadow[0], "shadow should turn blue: {shadow:?}");
        let bright = LinearImage::new(1, 1, vec![f16::from_f32(0.9).to_bits(); 3]);
        assert_eq!(export(&renderer, &bright, &adj, 64), export(&renderer, &bright, &Adjustments::default(), 64));
    }

    #[test]
    fn the_identity_look_changes_nothing() {
        let renderer = renderer();
        let image = ramp(64, 32);
        let adj = Adjustments { lut_amount: 1.0, ..Default::default() };
        let plain = export(&renderer, &image, &Adjustments::default(), 64);
        let looked = export(&renderer, &image, &adj, 64);
        for (a, b) in plain.iter().zip(&looked) {
            assert!((i32::from(*a) - i32::from(*b)).abs() <= 1, "{a} vs {b}");
        }
    }

    #[test]
    fn a_look_recolours_in_proportion_to_its_amount() {
        let mut renderer = renderer();
        // Every input maps to pure red.
        let red: Vec<f32> = [1.0, 0.0, 0.0].repeat(8);
        renderer.set_look(Some((2, &red)));
        let image = flat_gray(2, 2);
        let full = export(&renderer, &image, &Adjustments { lut_amount: 1.0, ..Default::default() }, 64);
        assert_eq!((full[0], full[1]), (255, 0));
        let half = export(&renderer, &image, &Adjustments { lut_amount: 0.5, ..Default::default() }, 64);
        assert!(half[0] > 118 && half[0] < 255 && half[1] < 118, "{half:?}");
    }

    /// Bright blocks on a darker field, with fine stripes on top.
    fn detailed(width: u32, height: u32) -> LinearImage {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let block = if (x / 40 + y / 40) % 2 == 0 { 0.4 } else { 0.25 };
                let stripe = if (x / 3) % 2 == 0 { 0.03 } else { -0.03 };
                pixels.extend([f16::from_f32(block + stripe).to_bits(); 3]);
            }
        }
        LinearImage::new(width, height, pixels)
    }

    #[test]
    fn texture_raises_fine_detail_and_negative_texture_lowers_it() {
        let mut renderer = renderer();
        let image = detailed(256, 256);
        renderer.set_preview(&image);
        let plain = export(&renderer, &image, &Adjustments::default(), 512);
        let up = export(&renderer, &image, &Adjustments { texture: 1.0, ..Default::default() }, 512);
        let down = export(&renderer, &image, &Adjustments { texture: -1.0, ..Default::default() }, 512);
        assert!(spread(&up) > spread(&plain), "{} vs {}", spread(&up), spread(&plain));
        assert!(spread(&down) < spread(&plain), "{} vs {}", spread(&down), spread(&plain));
    }

    #[test]
    fn a_hazy_image_gains_contrast_from_dehazing() {
        let mut renderer = renderer();
        // A scene seen through bright haze: everything is lifted towards 0.6.
        let mut pixels = Vec::new();
        for y in 0..128u32 {
            for x in 0..128u32 {
                let scene = if (x / 16 + y / 16) % 2 == 0 { 0.05 } else { 0.4 };
                pixels.extend([f16::from_f32(scene * 0.4 + 0.6 * 0.6).to_bits(); 3]);
            }
        }
        let image = LinearImage::new(128, 128, pixels);
        renderer.set_preview(&image);
        let plain = export(&renderer, &image, &Adjustments::default(), 256);
        let clear = export(&renderer, &image, &Adjustments { dehaze: 0.8, ..Default::default() }, 256);
        let hazier = export(&renderer, &image, &Adjustments { dehaze: -0.8, ..Default::default() }, 256);
        assert!(spread(&clear) > spread(&plain) * 1.3, "{} vs {}", spread(&clear), spread(&plain));
        assert!(spread(&hazier) < spread(&plain), "{} vs {}", spread(&hazier), spread(&plain));
    }

    #[test]
    fn grain_adds_variation_to_a_flat_image() {
        let renderer = renderer();
        let image = flat_gray(2048, 64);
        let plain = export(&renderer, &image, &Adjustments::default(), 2048);
        let grainy = export(&renderer, &image, &Adjustments { grain: 1.0, ..Default::default() }, 2048);
        assert!(plain.iter().all(|&value| value == plain[0]));
        // A part of it: `spread` sums in single precision, which drifts over a long image.
        assert!(spread(&grainy[..3 * 256 * 64]) > 1.5, "{}", spread(&grainy[..3 * 256 * 64]));
    }

    #[test]
    fn tiles_stay_identical_with_texture_dehaze_and_grain() {
        let mut renderer = renderer();
        let image = detailed(300, 170);
        renderer.set_preview(&image);
        let adj = Adjustments { texture: 0.7, dehaze: 0.5, grain: 0.6, grain_size: 0.5, ..Default::default() };
        assert_eq!(export(&renderer, &image, &adj, 64), export(&renderer, &image, &adj, 512));
    }

    #[test]
    fn a_luminance_range_limits_a_layer_to_the_bright_pixels() {
        let mut renderer = renderer();
        renderer.set_local_mask(1, 1, &[255, 0, 0, 0]);
        let levels = [0.02f32, 0.02, 0.02, 0.7, 0.7, 0.7];
        let image = LinearImage::new(2, 1, levels.map(|v| f16::from_f32(v).to_bits()).to_vec());
        let mut adj = Adjustments::default();
        adj.local[0][0] = -1.0;
        adj.local_range[0] = [0.5, 1.0, 0.0, 0.0];
        let plain = export(&renderer, &image, &Adjustments::default(), 64);
        let out = export(&renderer, &image, &adj, 64);
        assert_eq!(out[0], plain[0], "dark pixel is outside the range");
        assert!(out[3] < plain[3] - 20, "bright pixel {} -> {}", plain[3], out[3]);
    }

    #[test]
    fn a_colour_range_limits_a_layer_to_its_hue() {
        let mut renderer = renderer();
        renderer.set_local_mask(1, 1, &[255, 0, 0, 0]);
        let values = [0.5f32, 0.05, 0.05, 0.05, 0.05, 0.5];
        let image = LinearImage::new(2, 1, values.map(|v| f16::from_f32(v).to_bits()).to_vec());
        let mut adj = Adjustments::default();
        adj.local[0][0] = 1.0;
        adj.local_range[0] = [0.0, 1.0, 0.0, 30.0];
        let plain = export(&renderer, &image, &Adjustments::default(), 64);
        let out = export(&renderer, &image, &adj, 64);
        assert!(out[0] > plain[0] + 20, "red {} -> {}", plain[0], out[0]);
        assert_eq!(&out[3..6], &plain[3..6], "blue is outside the hue range");
    }

    #[test]
    fn a_small_zoom_stays_centred_and_a_large_one_pans() {
        let fit = VisibleArea::new([0.5, 1.0], ViewTransform::default());
        assert_eq!(fit, VisibleArea { quad: [0.5, 1.0], start: [0.0, 0.0], span: [1.0, 1.0] });

        // The width just fills the canvas; the height overflows it and shows half.
        let zoomed = VisibleArea::new([0.5, 1.0], ViewTransform { zoom: 2.0, center: [0.5, 0.25] });
        assert_eq!(zoomed, VisibleArea { quad: [1.0, 1.0], start: [0.0, 0.0], span: [1.0, 0.5] });

        // Panning stops at the edge of the image.
        let edge = VisibleArea::new([1.0, 1.0], ViewTransform { zoom: 4.0, center: [1.0, 0.0] });
        assert_eq!(edge.start, [0.75, 0.0]);
        assert_eq!(edge.span, [0.25, 0.25]);
    }

    #[test]
    fn clicks_map_through_the_zoomed_view() {
        let mut renderer = renderer();
        renderer.set_preview(&flat_gray(200, 200));
        renderer.set_view(ViewTransform { zoom: 4.0, center: [0.75, 0.25] });
        let (u, v) = renderer.canvas_to_image(&Geometry::default(), (200, 200), 0.5, 0.5).unwrap();
        assert!((u - 0.75).abs() < 1e-4 && (v - 0.25).abs() < 1e-4, "{u}, {v}");
        let (u, v) = renderer.canvas_to_image(&Geometry::default(), (200, 200), 0.0, 1.0).unwrap();
        assert!((u - 0.625).abs() < 1e-4 && (v - 0.375).abs() < 1e-4, "{u}, {v}");
    }

    #[test]
    fn letterbox_fits_inside_the_target() {
        assert_eq!(letterbox((100, 100), (200, 100)), [0.5, 1.0]);
        assert_eq!(letterbox((100, 100), (100, 200)), [1.0, 0.5]);
    }

    #[test]
    fn jpeg_output_has_a_valid_header() {
        let jpeg = encode_jpeg(&[128; 3 * 16 * 16], 16, 16, 90, &op_core::srgb_icc_profile(), None).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }
}
