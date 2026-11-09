use bytemuck::{Pod, Zeroable, bytes_of, cast_slice};
use futures_channel::oneshot;
use op_core::{Adjustments, LinearImage};
use wgpu::util::align_to;

const SOURCE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const SOURCE_BYTES_PER_PIXEL: u32 = 8;
const OUTPUT_BYTES_PER_PIXEL: u32 = 4;
const WORKGROUP_SIZE: u32 = 16;
/// Source pixels each export tile can see beyond its own area, for sharpening.
const TILE_MARGIN: u32 = 2;
const BLURRED_LUMA_LONG_EDGE: u32 = 128;

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
}

struct Preview {
    width: u32,
    height: u32,
    source_view: wgpu::TextureView,
    output_view: wgpu::TextureView,
    develop: wgpu::BindGroup,
    blit: wgpu::BindGroup,
}

/// Develops images on the GPU: an interactive path over a downscaled proxy and a
/// tiled path for full-resolution export. Both run the same shader.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    develop_pipeline: wgpu::ComputePipeline,
    develop_layout: wgpu::BindGroupLayout,
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    adjust_buffer: wgpu::Buffer,
    region_buffer: wgpu::Buffer,
    blit_buffer: wgpu::Buffer,
    mask_view: wgpu::TextureView,
    blurred_luma_view: wgpu::TextureView,
    base_exposure: Option<f32>,
    preview: Option<Preview>,
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, target: wgpu::TextureFormat) -> Self {
        let develop_module =
            device.create_shader_module(wgpu::include_wgsl!("develop.wgsl"));
        let blit_module = device.create_shader_module(wgpu::include_wgsl!("blit.wgsl"));

        let develop_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("develop"),
            entries: &[
                texture_entry(0, wgpu::ShaderStages::COMPUTE, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: OUTPUT_FORMAT,
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
            ],
        });
        let develop_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("develop"),
                bind_group_layouts: &[Some(&develop_layout)],
                immediate_size: 0,
            });
        let develop_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("develop"),
            layout: Some(&develop_pipeline_layout),
            module: &develop_module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

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
                uniform_entry(2, wgpu::ShaderStages::VERTEX),
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
        let blit_buffer = uniform_buffer(&device, "blit", size_of::<[f32; 2]>());
        let mask_view = create_mask_view(&device, &queue, 1, 1, &[255]);
        let blurred_luma_view = create_luma_view(&device, &queue, 1, 1, &[0]);

        Self {
            device,
            queue,
            develop_pipeline,
            develop_layout,
            blit_pipeline,
            blit_layout,
            sampler,
            adjust_buffer,
            region_buffer,
            blit_buffer,
            mask_view,
            blurred_luma_view,
            base_exposure: None,
            preview: None,
        }
    }

    /// Enables the default RAW look with an exposure offset in EV, or disables it for
    /// images that already have a tone curve.
    pub fn set_base_exposure(&mut self, base_exposure: Option<f32>) {
        self.base_exposure = base_exposure;
    }

    /// Sets the subject mask for the current image. `mask` holds `width * height`
    /// bytes covering the whole image, at any resolution.
    pub fn set_mask(&mut self, width: u32, height: u32, mask: &[u8]) {
        self.mask_view = create_mask_view(&self.device, &self.queue, width, height, mask);
        if let Some(preview) = &self.preview {
            let develop = self.develop_bind_group(&preview.source_view, &preview.output_view);
            self.preview.as_mut().expect("checked above").develop = develop;
        }
    }

    /// Uploads `proxy` as the image shown by [`Self::draw_preview`] and clears any
    /// mask belonging to the previous image.
    pub fn set_preview(&mut self, proxy: &LinearImage) {
        self.mask_view = create_mask_view(&self.device, &self.queue, 1, 1, &[255]);
        let (luma_width, luma_height, luma) = proxy.blurred_luma(BLURRED_LUMA_LONG_EDGE);
        self.blurred_luma_view = create_luma_view(&self.device, &self.queue, luma_width, luma_height, &luma);
        let (width, height) = (proxy.width, proxy.height);
        let source = self.create_source_texture(width, height);
        self.upload_source(&source, &proxy.rgba_region(0, 0, width, height), width, height);
        let output = self.create_output_texture(width, height);

        let source_view = source.create_view(&Default::default());
        let output_view = output.create_view(&Default::default());
        let develop = self.develop_bind_group(&source_view, &output_view);
        let blit = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&output_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.blit_buffer.as_entire_binding(),
                },
            ],
        });
        self.preview = Some(Preview { width, height, source_view, output_view, develop, blit });
    }

    /// Develops the proxy with `adjustments` and draws it, letterboxed, into `target`.
    pub fn draw_preview(
        &self,
        adjustments: &Adjustments,
        target: &wgpu::TextureView,
        target_size: (u32, u32),
    ) -> Result<(), GpuError> {
        let preview = self.preview.as_ref().ok_or(GpuError::NoImage)?;
        let (width, height) = (preview.width as f32, preview.height as f32);
        self.write_params(adjustments, [width, height], [0.0; 2], [width, height], [0.0; 2], [width, height]);
        let scale = letterbox((preview.width, preview.height), target_size);
        self.queue.write_buffer(&self.blit_buffer, 0, bytes_of(&scale));

        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.dispatch(&mut encoder, &preview.develop, preview.width, preview.height);
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
        // The tint is an on-screen editing aid and must never reach a delivered file.
        let adjustments = &Adjustments { mask_overlay: 0.0, ..*adjustments };
        // Small images, like thumbnails, do not need full-size tile textures.
        let tile_size = tile_size
            .min(image.width.max(image.height))
            .min(self.device.limits().max_texture_dimension_2d - 2 * TILE_MARGIN);
        let source_size = tile_size + 2 * TILE_MARGIN;
        let source = self.create_source_texture(source_size, source_size);
        let output = self.create_output_texture(tile_size, tile_size);
        let source_view = source.create_view(&Default::default());
        let output_view = output.create_view(&Default::default());
        let bind_group = self.develop_bind_group(&source_view, &output_view);

        let row_pitch = align_to(tile_size * OUTPUT_BYTES_PER_PIXEL, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("export readback"),
            size: u64::from(row_pitch) * u64::from(tile_size),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let (full_w, full_h) = (image.width, image.height);
        let out_stride = full_w as usize * 3;
        let mut out = vec![0u8; out_stride * full_h as usize];

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
                self.dispatch(&mut encoder, &bind_group, w, h);
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
                        let src = &mapped[row * row_pitch as usize..][..w as usize * 4];
                        let dst_start = (tile_y as usize + row) * out_stride + tile_x as usize * 3;
                        let dst = &mut out[dst_start..dst_start + w as usize * 3];
                        for (rgba, rgb) in src.chunks_exact(4).zip(dst.chunks_exact_mut(3)) {
                            rgb.copy_from_slice(&rgba[..3]);
                        }
                    }
                }
                readback.unmap();
            }
        }
        Ok(out)
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
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("output"),
            size: extent(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
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
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("develop"),
            layout: &self.develop_layout,
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
        let region = Region {
            full_size,
            offset,
            extent,
            base_exposure: self.base_exposure.unwrap_or(0.0),
            has_base_look: f32::from(u8::from(self.base_exposure.is_some())),
            source_offset,
            source_size,
        };
        self.queue.write_buffer(&self.region_buffer, 0, bytes_of(&region));
    }

    fn dispatch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        bind_group: &wgpu::BindGroup,
        width: u32,
        height: u32,
    ) {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&self.develop_pipeline);
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

/// Encodes tightly packed RGB8 as a baseline JPEG. `quality` is 1..=100.
pub fn encode_jpeg(rgb: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, GpuError> {
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, quality).encode(
        rgb,
        width as u16,
        height as u16,
        jpeg_encoder::ColorType::Rgb,
    )?;
    Ok(out)
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

    #[test]
    fn base_look_maps_sensor_range_to_display_range() {
        let mut renderer = renderer();
        renderer.set_base_exposure(Some(1.2));
        assert_eq!(export_gray(&renderer, 0.0), 0);
        assert_eq!(export_gray(&renderer, 1.0), 255);
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
    fn letterbox_fits_inside_the_target() {
        assert_eq!(letterbox((100, 100), (200, 100)), [0.5, 1.0]);
        assert_eq!(letterbox((100, 100), (100, 200)), [1.0, 0.5]);
    }

    #[test]
    fn jpeg_output_has_a_valid_header() {
        let jpeg = encode_jpeg(&[128; 3 * 16 * 16], 16, 16, 90).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }
}
