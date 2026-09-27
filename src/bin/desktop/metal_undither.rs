//! grimoire's undither filter on the Metal presenter's frames.
//!
//! The filter itself, and why it differs from grimoire where it does, is
//! described in `metal_undither.metal`. This is the plumbing: four passes at
//! the guest's resolution between the frame the presenter was given and the
//! pass that scales it into the drawable. A native guest frame goes through
//! the filter and is drawn as it comes out. A raster presented at a larger
//! scale -- the outline text surface -- is read back down to one colour per
//! guest pixel first, and put back together afterwards, so text drawn at the
//! drawable's scale is shown exactly as it was drawn.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{ns_string, NSString};
use objc2_metal::{
    MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLDevice, MTLLibrary,
    MTLLoadAction, MTLPixelFormat, MTLPrimitiveType, MTLRenderCommandEncoder,
    MTLRenderPassDescriptor, MTLRenderPipelineDescriptor, MTLRenderPipelineState,
    MTLResourceOptions, MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor,
    MTLTextureUsage, MTLViewport,
};
use objc2_quartz_core::CAMetalDrawable;

use super::{
    aspect_fit_viewport, finish_presentation, non_null_bytes, GuestCursorData, GuestFrameMetadata,
    GuestFrameUniforms, FRAME_RESOURCE_COUNT,
};

/// The browser player's settings, which are grimoire's defaults:
/// sensitivity, strength, detail recovery and the upscale's edge threshold.
const SENSITIVITY: f32 = 0.67;
const STRENGTH: f32 = 0.67;
const DETAIL: f32 = 0.5;
const EDGE: f32 = 64.0;
/// Blend in light rather than in stored values (see the shader's header).
pub(super) const LIGHT: bool = true;

/// Whether the filter is wanted: `SYSTEMLESS_UNDITHER=1`. It is off by
/// default, as the browser player's Undither button is.
pub(super) fn enabled_by_environment() -> bool {
    std::env::var("SYSTEMLESS_UNDITHER").is_ok_and(|value| value == "1")
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct UnditherParams {
    sens: f32,
    strength: f32,
    detail: f32,
    edge: f32,
    light: u32,
    scale: u32,
}

impl UnditherParams {
    pub(super) fn new(light: bool, scale: u32) -> Self {
        Self {
            sens: SENSITIVITY,
            strength: STRENGTH,
            detail: DETAIL,
            edge: EDGE,
            light: u32::from(light),
            scale,
        }
    }
}

/// What a raster frame carries for the filter: the guest's palette indices
/// under it, packed `width / scale` to a row, and the palette they index.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct RasterUndither {
    pub(super) indices: Vec<u8>,
    pub(super) palette: [u32; 256],
    pub(super) scale: u32,
}

/// The filter's render pipelines, compiled once and shared by the presenter
/// and its worker thread.
#[derive(Clone)]
pub(super) struct UnditherPipelines {
    source_guest: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    source_presented: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    mean: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    detect: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    detail: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    upscale: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    compose: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
}

// Where each pass writes. The source keeps its class in alpha and needs only
// eight bits; the residuals are small numbers that eight bits would quantise
// onto the faint end of the detector's ramp, so the passes between are float.
const SOURCE_FORMAT: MTLPixelFormat = MTLPixelFormat::RGBA8Unorm;
const WORKING_FORMAT: MTLPixelFormat = MTLPixelFormat::RGBA32Float;
const OUTPUT_FORMAT: MTLPixelFormat = MTLPixelFormat::RGBA8Unorm;

impl UnditherPipelines {
    pub(super) fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<Self, String> {
        let source = NSString::from_str(concat!(
            include_str!("metal_present.metal"),
            include_str!("metal_undither.metal")
        ));
        let library = device
            .newLibraryWithSource_options_error(&source, None)
            .map_err(|error| format!("undither shader compilation failed: {error}"))?;
        let vertex = library
            .newFunctionWithName(ns_string!("raster_vertex"))
            .ok_or_else(|| "undither vertex function was not found".to_string())?;
        let pipeline = |name: &str, format: MTLPixelFormat| {
            let fragment = library
                .newFunctionWithName(&NSString::from_str(name))
                .ok_or_else(|| format!("undither function {name} was not found"))?;
            let descriptor = MTLRenderPipelineDescriptor::new();
            descriptor.setVertexFunction(Some(&vertex));
            descriptor.setFragmentFunction(Some(&fragment));
            unsafe {
                descriptor
                    .colorAttachments()
                    .objectAtIndexedSubscript(0)
                    .setPixelFormat(format);
            }
            device
                .newRenderPipelineStateWithDescriptor_error(&descriptor)
                .map_err(|error| format!("undither pipeline {name} failed: {error}"))
        };
        Ok(Self {
            source_guest: pipeline("undither_source_guest", SOURCE_FORMAT)?,
            source_presented: pipeline("undither_source_presented", SOURCE_FORMAT)?,
            mean: pipeline("undither_mean", WORKING_FORMAT)?,
            detect: pipeline("undither_detect", WORKING_FORMAT)?,
            detail: pipeline("undither_detail", WORKING_FORMAT)?,
            upscale: pipeline("undither_super", OUTPUT_FORMAT)?,
            compose: pipeline("undither_compose", OUTPUT_FORMAT)?,
        })
    }
}

type Texture = Retained<ProtocolObject<dyn MTLTexture>>;

struct GuestTargets {
    size: (u32, u32),
    source: Texture,
    mean: Texture,
    detected: Texture,
    detailed: Texture,
    filtered: Texture,
}

/// The textures one presenter draws the filter through, and the ring of
/// buffers that carry a raster frame's indices to the GPU. Everything but the
/// index buffers is written and read by the GPU alone, so one set serves
/// frames in flight one after another; the CPU writes the index buffers, so
/// there are as many as there are frames in flight.
#[derive(Default)]
pub(super) struct UnditherTargets {
    guest: Option<GuestTargets>,
    composed: Option<((u32, u32), Texture)>,
    index_buffers: Vec<Retained<ProtocolObject<dyn MTLBuffer>>>,
    index_buffer_size: usize,
    next_index_buffer: usize,
}

fn target_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    format: MTLPixelFormat,
    (width, height): (u32, u32),
) -> Result<Texture, String> {
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format,
            width as usize,
            height as usize,
            false,
        )
    };
    descriptor.setStorageMode(MTLStorageMode::Private);
    descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
    device
        .newTextureWithDescriptor(&descriptor)
        .ok_or_else(|| "Metal failed to allocate an undither texture".to_string())
}

impl UnditherTargets {
    fn guest(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        size: (u32, u32),
    ) -> Result<&GuestTargets, String> {
        if self.guest.as_ref().map(|targets| targets.size) != Some(size) {
            self.guest = Some(GuestTargets {
                size,
                source: target_texture(device, SOURCE_FORMAT, size)?,
                mean: target_texture(device, WORKING_FORMAT, size)?,
                detected: target_texture(device, WORKING_FORMAT, size)?,
                detailed: target_texture(device, WORKING_FORMAT, size)?,
                filtered: target_texture(device, OUTPUT_FORMAT, size)?,
            });
        }
        Ok(self.guest.as_ref().unwrap())
    }

    fn composed(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        size: (u32, u32),
    ) -> Result<Texture, String> {
        if self.composed.as_ref().map(|(held, _)| *held) != Some(size) {
            self.composed = Some((size, target_texture(device, OUTPUT_FORMAT, size)?));
        }
        Ok(self.composed.as_ref().unwrap().1.clone())
    }

    fn index_buffer(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        indices: &[u8],
    ) -> Result<Retained<ProtocolObject<dyn MTLBuffer>>, String> {
        if self.index_buffer_size != indices.len() {
            let mut buffers = Vec::with_capacity(FRAME_RESOURCE_COUNT);
            for _ in 0..FRAME_RESOURCE_COUNT {
                buffers.push(
                    device
                        .newBufferWithLength_options(
                            indices.len(),
                            MTLResourceOptions::MTLResourceStorageModeShared,
                        )
                        .ok_or_else(|| "Metal failed to allocate an index buffer".to_string())?,
                );
            }
            self.index_buffers = buffers;
            self.index_buffer_size = indices.len();
            self.next_index_buffer = 0;
        }
        let buffer = self.index_buffers[self.next_index_buffer].clone();
        self.next_index_buffer = (self.next_index_buffer + 1) % self.index_buffers.len();
        unsafe {
            std::ptr::copy_nonoverlapping(
                indices.as_ptr(),
                buffer.contents().as_ptr().cast::<u8>(),
                indices.len(),
            );
        }
        Ok(buffer)
    }
}

/// Draw one full-target quad with `pipeline` into `target`.
fn encode_pass(
    command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
    pipeline: &ProtocolObject<dyn MTLRenderPipelineState>,
    target: &ProtocolObject<dyn MTLTexture>,
    bind: impl FnOnce(&ProtocolObject<dyn MTLRenderCommandEncoder>),
) -> Result<(), String> {
    let pass = unsafe { MTLRenderPassDescriptor::new() };
    let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
    color.setTexture(Some(target));
    color.setLoadAction(MTLLoadAction::DontCare);
    color.setStoreAction(MTLStoreAction::Store);
    let encoder = command_buffer
        .renderCommandEncoderWithDescriptor(&pass)
        .ok_or_else(|| "Metal failed to create an undither encoder".to_string())?;
    encoder.setRenderPipelineState(pipeline);
    encoder.setViewport(MTLViewport {
        originX: 0.0,
        originY: 0.0,
        width: target.width() as f64,
        height: target.height() as f64,
        znear: 0.0,
        zfar: 1.0,
    });
    bind(&encoder);
    unsafe {
        encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::TriangleStrip, 0, 4)
    };
    encoder.endEncoding();
    Ok(())
}

/// The four filter passes over a source already drawn into `targets.source`.
fn encode_filter(
    command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
    pipelines: &UnditherPipelines,
    targets: &GuestTargets,
    params: &UnditherParams,
) -> Result<(), String> {
    let bind_params = |encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>| unsafe {
        encoder.setFragmentBytes_length_atIndex(
            non_null_bytes(params),
            size_of::<UnditherParams>(),
            0,
        );
    };
    encode_pass(
        command_buffer,
        &pipelines.mean,
        &targets.mean,
        |encoder| unsafe {
            encoder.setFragmentTexture_atIndex(Some(&targets.source), 0);
        },
    )?;
    encode_pass(
        command_buffer,
        &pipelines.detect,
        &targets.detected,
        |encoder| {
            unsafe {
                encoder.setFragmentTexture_atIndex(Some(&targets.source), 0);
                encoder.setFragmentTexture_atIndex(Some(&targets.mean), 1);
            }
            bind_params(encoder);
        },
    )?;
    encode_pass(
        command_buffer,
        &pipelines.detail,
        &targets.detailed,
        |encoder| {
            unsafe {
                encoder.setFragmentTexture_atIndex(Some(&targets.source), 0);
                encoder.setFragmentTexture_atIndex(Some(&targets.detected), 1);
            }
            bind_params(encoder);
        },
    )?;
    encode_pass(
        command_buffer,
        &pipelines.upscale,
        &targets.filtered,
        |encoder| {
            unsafe {
                encoder.setFragmentTexture_atIndex(Some(&targets.source), 0);
                encoder.setFragmentTexture_atIndex(Some(&targets.detailed), 1);
            }
            bind_params(encoder);
        },
    )
}

/// Filter a native guest frame whose visible bytes are in `guest_buffer`,
/// leaving the result in the returned texture at the content's size.
pub(super) fn encode_guest(
    command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
    device: &ProtocolObject<dyn MTLDevice>,
    pipelines: &UnditherPipelines,
    targets: &mut UnditherTargets,
    guest_buffer: &ProtocolObject<dyn MTLBuffer>,
    metadata: &GuestFrameMetadata,
    light: bool,
) -> Result<Texture, String> {
    let (_, _, width, height) = metadata.content_rect;
    let targets = targets.guest(device, (width, height))?;
    encode_pass(
        command_buffer,
        &pipelines.source_guest,
        &targets.source,
        |encoder| unsafe {
            encoder.setFragmentBuffer_offset_atIndex(Some(guest_buffer), 0, 0);
            encoder.setFragmentBytes_length_atIndex(
                non_null_bytes(&metadata.palette),
                size_of::<[u32; 256]>(),
                1,
            );
            encoder.setFragmentBytes_length_atIndex(
                non_null_bytes(&metadata.uniforms),
                size_of::<GuestFrameUniforms>(),
                2,
            );
            encoder.setFragmentBytes_length_atIndex(
                non_null_bytes(&metadata.cursor),
                size_of::<GuestCursorData>(),
                3,
            );
        },
    )?;
    encode_filter(
        command_buffer,
        pipelines,
        targets,
        &UnditherParams::new(light, 1),
    )?;
    Ok(targets.filtered.clone())
}

/// Filter a raster presented at `undither.scale` raster pixels per guest
/// pixel, already uploaded into `presented`, and put it back together at the
/// raster's size in the returned texture.
pub(super) fn encode_presented(
    command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
    device: &ProtocolObject<dyn MTLDevice>,
    pipelines: &UnditherPipelines,
    targets: &mut UnditherTargets,
    presented: &ProtocolObject<dyn MTLTexture>,
    undither: &RasterUndither,
    light: bool,
) -> Result<Texture, String> {
    let raster_size = (presented.width() as u32, presented.height() as u32);
    let scale = undither.scale;
    let guest_size = (raster_size.0 / scale, raster_size.1 / scale);
    if scale == 0
        || guest_size.0 * scale != raster_size.0
        || guest_size.1 * scale != raster_size.1
        || undither.indices.len() != guest_size.0 as usize * guest_size.1 as usize
    {
        return Err("undither indices do not match the presented raster".to_string());
    }
    let params = UnditherParams::new(light, scale);
    let index_buffer = targets.index_buffer(device, &undither.indices)?;
    let composed = targets.composed(device, raster_size)?;
    let targets = targets.guest(device, guest_size)?;
    encode_pass(
        command_buffer,
        &pipelines.source_presented,
        &targets.source,
        |encoder| unsafe {
            encoder.setFragmentTexture_atIndex(Some(presented), 0);
            encoder.setFragmentBuffer_offset_atIndex(Some(&index_buffer), 0, 0);
            encoder.setFragmentBytes_length_atIndex(
                non_null_bytes(&undither.palette),
                size_of::<[u32; 256]>(),
                1,
            );
            encoder.setFragmentBytes_length_atIndex(
                non_null_bytes(&params),
                size_of::<UnditherParams>(),
                2,
            );
        },
    )?;
    encode_filter(command_buffer, pipelines, targets, &params)?;
    encode_pass(
        command_buffer,
        &pipelines.compose,
        &composed,
        |encoder| unsafe {
            encoder.setFragmentTexture_atIndex(Some(presented), 0);
            encoder.setFragmentTexture_atIndex(Some(&targets.source), 1);
            encoder.setFragmentTexture_atIndex(Some(&targets.filtered), 2);
            encoder.setFragmentBytes_length_atIndex(
                non_null_bytes(&params),
                size_of::<UnditherParams>(),
                0,
            );
        },
    )?;
    Ok(composed)
}

/// Scale `texture` into the drawable with the presenter's raster pass and
/// present it, as `encode_raster_frame` does for an uploaded frame.
pub(super) fn present_texture(
    command_queue: &ProtocolObject<dyn MTLCommandQueue>,
    raster_pipeline: &ProtocolObject<dyn MTLRenderPipelineState>,
    encode: impl FnOnce(&ProtocolObject<dyn MTLCommandBuffer>) -> Result<Texture, String>,
    drawable: &ProtocolObject<dyn CAMetalDrawable>,
    drawable_size: (u32, u32),
    transactional: bool,
) -> Result<(), String> {
    let command_buffer = command_queue
        .commandBuffer()
        .ok_or_else(|| "Metal failed to create a command buffer".to_string())?;
    let texture = encode(&command_buffer)?;
    let drawable_texture = unsafe { drawable.texture() };
    let pass = unsafe { MTLRenderPassDescriptor::new() };
    let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
    color.setTexture(Some(&drawable_texture));
    color.setLoadAction(MTLLoadAction::Clear);
    color.setStoreAction(MTLStoreAction::Store);
    color.setClearColor(objc2_metal::MTLClearColor {
        red: 0.0,
        green: 0.0,
        blue: 0.0,
        alpha: 1.0,
    });
    let encoder = command_buffer
        .renderCommandEncoderWithDescriptor(&pass)
        .ok_or_else(|| "Metal failed to create a render encoder".to_string())?;
    encoder.setRenderPipelineState(raster_pipeline);
    unsafe { encoder.setFragmentTexture_atIndex(Some(&texture), 0) };
    let viewport = aspect_fit_viewport(
        texture.width() as u32,
        texture.height() as u32,
        drawable_size.0,
        drawable_size.1,
    );
    encoder.setViewport(MTLViewport {
        originX: viewport.0,
        originY: viewport.1,
        width: viewport.2,
        height: viewport.3,
        znear: 0.0,
        zfar: 1.0,
    });
    unsafe {
        encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::TriangleStrip, 0, 4)
    };
    encoder.endEncoding();
    finish_presentation(&command_buffer, drawable, transactional);
    Ok(())
}

/// Pack the guest's indices under a raster presented at `scale`, or `None`
/// when the guest's screen is not eight bits deep or the raster is not a whole
/// number of guest pixels.
pub(super) fn raster_undither(
    guest: &super::GuestIndices<'_>,
    raster_size: (u32, u32),
    mut indices: Vec<u8>,
) -> Option<RasterUndither> {
    let scale = guest.scale;
    if guest.pixel_size != 8
        || scale == 0
        || !raster_size.0.is_multiple_of(scale)
        || !raster_size.1.is_multiple_of(scale)
    {
        return None;
    }
    let layout = super::guest_visible_byte_layout(
        guest.row_bytes,
        8,
        guest.left,
        guest.top,
        raster_size.0 / scale,
        raster_size.1 / scale,
    )?;
    let last_row_end = layout
        .first_row_offset
        .checked_add((layout.row_count - 1).checked_mul(layout.row_stride)?)?
        .checked_add(layout.visible_row_bytes)?;
    if last_row_end > guest.framebuffer.len() {
        return None;
    }
    super::copy_guest_visible_pixels(&mut indices, guest.framebuffer, layout);
    Some(RasterUndither {
        indices,
        palette: *guest.palette,
        scale,
    })
}

#[cfg(test)]
mod tests {
    use std::ptr::NonNull;

    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::ProtocolObject;
    use objc2_foundation::{ns_string, NSString};
    use objc2_metal::{
        MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice,
        MTLDevice, MTLLibrary, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType, MTLRegion,
        MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
        MTLResourceOptions, MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor,
        MTLTextureUsage, MTLViewport,
    };
    use systemless::display::CursorImage;

    use super::super::{guest_cursor_data, GuestFrameMetadata};
    use super::{
        encode_guest, encode_presented, RasterUndither, UnditherPipelines, UnditherTargets,
    };

    type Device = Retained<ProtocolObject<dyn MTLDevice>>;
    type Texture = Retained<ProtocolObject<dyn MTLTexture>>;

    /// The frame and grimoire's output that `generate.mjs` wrote.
    struct Reference {
        width: u32,
        height: u32,
        palette: [u32; 256],
        indices: Vec<u8>,
        expected: Vec<u8>,
    }

    fn reference() -> Reference {
        let bytes = include_bytes!("../../../tests/undither/grimoire-reference.bin");
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let (width, height) = (word(0), word(4));
        let mut palette = [0u32; 256];
        for (i, entry) in palette.iter_mut().enumerate() {
            *entry = word(8 + i * 4);
        }
        let pixels = (width * height) as usize;
        let indices_at = 8 + 256 * 4;
        let expected_at = indices_at + pixels;
        assert_eq!(bytes.len(), expected_at + pixels * 4, "fixture length");
        Reference {
            width,
            height,
            palette,
            indices: bytes[indices_at..expected_at].to_vec(),
            expected: bytes[expected_at..].to_vec(),
        }
    }

    fn device() -> Device {
        unsafe { Retained::retain(MTLCreateSystemDefaultDevice()) }.expect("Metal device required")
    }

    fn region(width: u32, height: u32) -> MTLRegion {
        MTLRegion {
            origin: objc2_metal::MTLOrigin { x: 0, y: 0, z: 0 },
            size: objc2_metal::MTLSize {
                width: width as usize,
                height: height as usize,
                depth: 1,
            },
        }
    }

    fn shared_texture(
        device: &Device,
        format: MTLPixelFormat,
        (width, height): (u32, u32),
        usage: MTLTextureUsage,
    ) -> Texture {
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                format,
                width as usize,
                height as usize,
                false,
            )
        };
        descriptor.setStorageMode(MTLStorageMode::Shared);
        descriptor.setUsage(usage);
        device.newTextureWithDescriptor(&descriptor).unwrap()
    }

    /// Encode with `encode`, then copy the texture it returns out one texel
    /// to a pixel through the presenter's own raster pass, and read it back.
    fn run(
        device: &Device,
        encode: impl FnOnce(&ProtocolObject<dyn MTLCommandBuffer>) -> Result<Texture, String>,
    ) -> image::RgbaImage {
        autoreleasepool(|_| {
            let library = device
                .newLibraryWithSource_options_error(
                    &NSString::from_str(concat!(
                        include_str!("metal_present.metal"),
                        include_str!("metal_undither.metal")
                    )),
                    None,
                )
                .unwrap();
            let descriptor = MTLRenderPipelineDescriptor::new();
            descriptor.setVertexFunction(
                library
                    .newFunctionWithName(ns_string!("raster_vertex"))
                    .as_deref(),
            );
            descriptor.setFragmentFunction(
                library
                    .newFunctionWithName(ns_string!("raster_fragment"))
                    .as_deref(),
            );
            unsafe {
                descriptor
                    .colorAttachments()
                    .objectAtIndexedSubscript(0)
                    .setPixelFormat(MTLPixelFormat::RGBA8Unorm);
            }
            let copy = device
                .newRenderPipelineStateWithDescriptor_error(&descriptor)
                .unwrap();
            let queue = device.newCommandQueue().unwrap();
            let command = queue.commandBuffer().unwrap();
            let texture = encode(&command).unwrap();
            let size = (texture.width() as u32, texture.height() as u32);
            let output = shared_texture(
                device,
                MTLPixelFormat::RGBA8Unorm,
                size,
                MTLTextureUsage::RenderTarget,
            );
            let pass = unsafe { MTLRenderPassDescriptor::new() };
            let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
            color.setTexture(Some(&output));
            color.setLoadAction(MTLLoadAction::Clear);
            color.setStoreAction(MTLStoreAction::Store);
            let encoder = command.renderCommandEncoderWithDescriptor(&pass).unwrap();
            encoder.setRenderPipelineState(&copy);
            encoder.setViewport(MTLViewport {
                originX: 0.0,
                originY: 0.0,
                width: size.0 as f64,
                height: size.1 as f64,
                znear: 0.0,
                zfar: 1.0,
            });
            unsafe {
                encoder.setFragmentTexture_atIndex(Some(&texture), 0);
                encoder.drawPrimitives_vertexStart_vertexCount(
                    MTLPrimitiveType::TriangleStrip,
                    0,
                    4,
                );
            }
            encoder.endEncoding();
            command.commit();
            unsafe { command.waitUntilCompleted() };
            let mut result = image::RgbaImage::new(size.0, size.1);
            unsafe {
                output.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                    NonNull::new(result.as_mut_ptr().cast()).unwrap(),
                    size.0 as usize * 4,
                    region(size.0, size.1),
                    0,
                );
            }
            result
        })
    }

    /// Put an eight-bit frame through the filter as a native guest frame.
    fn undither_guest(
        width: u32,
        height: u32,
        indices: &[u8],
        palette: &[u32; 256],
        cursor: Option<(&CursorImage, (i16, i16))>,
        light: bool,
    ) -> image::RgbaImage {
        let device = device();
        let pipelines = UnditherPipelines::new(&device).unwrap();
        let mut targets = UnditherTargets::default();
        let buffer = unsafe {
            device.newBufferWithBytes_length_options(
                NonNull::new(indices.as_ptr().cast_mut().cast()).unwrap(),
                indices.len(),
                MTLResourceOptions::MTLResourceStorageModeShared,
            )
        }
        .unwrap();
        let (mut uniforms, cursor) = match cursor {
            Some((image, position)) => guest_cursor_data(Some(image), position),
            None => guest_cursor_data(None, (0, 0)),
        };
        uniforms.row_bytes = width;
        uniforms.width = width;
        uniforms.height = height;
        uniforms.pixel_size = 8;
        let metadata = GuestFrameMetadata {
            screen_layout: (width, width as u16, height as u16, 8),
            content_rect: (0, 0, width, height),
            palette: *palette,
            uniforms,
            cursor,
            drawable_size: (width, height),
            undither: true,
        };
        run(&device, |command| {
            encode_guest(
                command,
                &device,
                &pipelines,
                &mut targets,
                &buffer,
                &metadata,
                light,
            )
        })
    }

    /// Put a raster presented at `scale` through the filter, with the guest's
    /// indices under it.
    fn undither_presented(
        raster: &[u32],
        size: (u32, u32),
        indices: &[u8],
        palette: &[u32; 256],
        scale: u32,
        light: bool,
    ) -> image::RgbaImage {
        let device = device();
        let pipelines = UnditherPipelines::new(&device).unwrap();
        let mut targets = UnditherTargets::default();
        let upload = shared_texture(
            &device,
            MTLPixelFormat::BGRA8Unorm,
            size,
            MTLTextureUsage::ShaderRead,
        );
        unsafe {
            upload.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                region(size.0, size.1),
                0,
                NonNull::new(raster.as_ptr().cast_mut().cast()).unwrap(),
                size.0 as usize * 4,
            );
        }
        let undither = RasterUndither {
            indices: indices.to_vec(),
            palette: *palette,
            scale,
        };
        run(&device, |command| {
            encode_presented(
                command,
                &device,
                &pipelines,
                &mut targets,
                &upload,
                &undither,
                light,
            )
        })
    }

    fn rgb(argb: u32) -> [u8; 3] {
        [(argb >> 16) as u8, (argb >> 8) as u8, argb as u8]
    }

    fn pixel_rgb(image: &image::RgbaImage, x: u32, y: u32) -> [u8; 3] {
        let p = image.get_pixel(x, y);
        [p[0], p[1], p[2]]
    }

    fn animated(index: u8) -> bool {
        (0xE0..0xFC).contains(&index)
    }

    /// Light, from an sRGB-encoded pixel: decoded, with Rec. 709 weights.
    fn light(rgb: [u8; 3]) -> f64 {
        let lin = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
    }

    #[test]
    fn undither_matches_grimoire_on_the_reference_frame() {
        let reference = reference();
        let (width, height) = (reference.width, reference.height);
        let output = undither_guest(
            width,
            height,
            &reference.indices,
            &reference.palette,
            None,
            false,
        );
        let (mut apart, mut changed, mut expected_changed, mut worst) = (0, 0, 0, 0);
        for y in 0..height {
            for x in 0..width {
                let i = (y * width + x) as usize;
                let before = rgb(reference.palette[reference.indices[i] as usize]);
                let expected = &reference.expected[i * 4..i * 4 + 3];
                let got = pixel_rgb(&output, x, y);
                let most = (0..3).map(|c| got[c].abs_diff(expected[c])).max().unwrap();
                worst = worst.max(most);
                apart += usize::from(most > 2);
                changed += usize::from(got != before);
                expected_changed += usize::from(expected != before);
            }
        }
        // The fixture's generator reports that grimoire without its
        // supersample stage is over a thousand pixels more than 2 away, so a
        // filter missing a stage cannot pass this; nor can one that does
        // nothing, since grimoire changes thousands of pixels.
        let limit = (width * height) as usize / 2000;
        assert!(
            apart <= limit,
            "{apart} pixels more than 2 from grimoire (limit {limit}), worst {worst}"
        );
        assert!(
            changed * 10 >= expected_changed * 9,
            "the filter changed {changed} pixels where grimoire changes {expected_changed}"
        );
    }

    #[test]
    fn undither_leaves_animated_colours_alone() {
        let reference = reference();
        let (width, height) = (reference.width, reference.height);
        let locked = undither_guest(
            width,
            height,
            &reference.indices,
            &reference.palette,
            None,
            true,
        );
        // The same colours under ordinary indices, so nothing is locked.
        let mut palette = reference.palette;
        let unlocked_indices = reference
            .indices
            .iter()
            .map(|&index| {
                if animated(index) {
                    palette[usize::from(index - 0x40)] = reference.palette[usize::from(index)];
                    index - 0x40
                } else {
                    index
                }
            })
            .collect::<Vec<_>>();
        let unlocked = undither_guest(width, height, &unlocked_indices, &palette, None, true);
        let (mut touched, mut touched_unlocked, mut total) = (0, 0, 0);
        for y in 0..height {
            for x in 0..width {
                let index = reference.indices[(y * width + x) as usize];
                if !animated(index) {
                    continue;
                }
                let before = rgb(reference.palette[usize::from(index)]);
                total += 1;
                touched += usize::from(pixel_rgb(&locked, x, y) != before);
                touched_unlocked += usize::from(pixel_rgb(&unlocked, x, y) != before);
            }
        }
        assert!(total > 0);
        assert_eq!(touched, 0, "{touched} of {total} animated pixels changed");
        assert!(
            touched_unlocked > 0,
            "unlocked, the filter changes none of the animated pixels either"
        );
    }

    #[test]
    fn undither_in_light_keeps_a_dithered_area_as_bright() {
        let reference = reference();
        let (width, height) = (reference.width, reference.height);
        let lit = undither_guest(
            width,
            height,
            &reference.indices,
            &reference.palette,
            None,
            true,
        );
        let stored = undither_guest(
            width,
            height,
            &reference.indices,
            &reference.palette,
            None,
            false,
        );
        // The bottom-right square is left out: its ordinary pixels blend with
        // animated ones that are locked and do not blend back, so light there
        // is not conserved in any arithmetic -- in light it rises by about a
        // sixth -- and that is grimoire's lock, not the blending.
        let (mut raw_light, mut lit_light, mut stored_light) = (0.0, 0.0, 0.0);
        for y in 0..height {
            for x in 0..width {
                if x >= 64 && y >= 32 {
                    continue;
                }
                let before =
                    rgb(reference.palette[reference.indices[(y * width + x) as usize] as usize]);
                if pixel_rgb(&stored, x, y) == before {
                    continue;
                }
                raw_light += light(before);
                lit_light += light(pixel_rgb(&lit, x, y));
                stored_light += light(pixel_rgb(&stored, x, y));
            }
        }
        let lit_loss = lit_light / raw_light - 1.0;
        let stored_loss = stored_light / raw_light - 1.0;
        // grimoire's arithmetic darkens what it blends, which is what makes
        // this a test: without that loss there would be nothing to keep.
        assert!(
            stored_loss < -0.01,
            "blending stored values loses only {:.2}% here",
            stored_loss * 100.0
        );
        assert!(
            lit_loss.abs() < 0.005,
            "blending in light changes the light by {:.2}% (stored values: {:.2}%)",
            lit_loss * 100.0,
            stored_loss * 100.0
        );
    }

    #[test]
    fn undither_shows_the_cursor_as_drawn() {
        let reference = reference();
        let (width, height) = (reference.width, reference.height);
        // A filled 16x16 square cursor, black with a white border, over the
        // checkerboard in the top-left corner.
        let mut data = [0u8; 32];
        let mask = [0xFFu8; 32];
        for row in 1..15 {
            data[row * 2..row * 2 + 2].copy_from_slice(&0x7FFEu16.to_be_bytes());
        }
        let cursor = CursorImage::mono(data, mask, 0, 0);
        let output = undither_guest(
            width,
            height,
            &reference.indices,
            &reference.palette,
            Some((&cursor, (4, 4))),
            true,
        );
        for y in 0..16 {
            for x in 0..16 {
                let border = x == 0 || y == 0 || x == 15 || y == 15;
                let expected = if border { [255; 3] } else { [0; 3] };
                assert_eq!(
                    pixel_rgb(&output, 4 + x, 4 + y),
                    expected,
                    "cursor pixel {x},{y}"
                );
            }
        }
    }

    #[test]
    fn presented_raster_keeps_text_as_drawn_and_filters_the_rest_as_the_guest_frame() {
        let reference = reference();
        let (width, height) = (reference.width, reference.height);
        let scale = 2;
        let size = (width * scale, height * scale);
        let mut raster = vec![0u32; (size.0 * size.1) as usize];
        for y in 0..size.1 {
            for x in 0..size.0 {
                let index = reference.indices[((y / scale) * width + x / scale) as usize];
                raster[(y * size.0 + x) as usize] = reference.palette[usize::from(index)];
            }
        }
        // Text drawn at the raster's scale: a diagonal stroke through the
        // blocks of guest row 40, columns 40 to 47, and a block that is one
        // colour but not its pixel's colour.
        let is_text =
            |gx: u32, gy: u32| (gy == 40 && (40..48).contains(&gx)) || (gx, gy) == (70, 50);
        for gx in 40..48 {
            let (x, y) = (gx * scale, 40 * scale);
            raster[(y * size.0 + x) as usize] = 0xFF000000;
            raster[((y + 1) * size.0 + x + 1) as usize] = 0xFFFFFFFF;
        }
        for y in 100..102 {
            for x in 140..142 {
                raster[(y * size.0 + x) as usize] = 0xFF123456;
            }
        }
        let presented = undither_presented(
            &raster,
            size,
            &reference.indices,
            &reference.palette,
            scale,
            true,
        );
        let guest = undither_guest(
            width,
            height,
            &reference.indices,
            &reference.palette,
            None,
            true,
        );
        // Text is shown exactly as drawn. A guest pixel far enough from any
        // text for no pass to reach it -- the detector looks three pixels
        // along a line, and four more passes reach one each -- is a block of
        // what the guest frame's filter made of it.
        let near_text = |gx: u32, gy: u32| {
            (gx.saturating_sub(8)..=gx + 8)
                .any(|tx| (gy.saturating_sub(8)..=gy + 8).any(|ty| is_text(tx, ty)))
        };
        let mut compared = 0;
        for y in 0..size.1 {
            for x in 0..size.0 {
                let (gx, gy) = (x / scale, y / scale);
                let got = pixel_rgb(&presented, x, y);
                if is_text(gx, gy) {
                    assert_eq!(
                        got,
                        rgb(raster[(y * size.0 + x) as usize]),
                        "text at {x},{y}"
                    );
                } else if !near_text(gx, gy) {
                    assert_eq!(got, pixel_rgb(&guest, gx, gy), "picture at {x},{y}");
                    compared += 1;
                }
            }
        }
        assert!(compared > (size.0 * size.1) as usize / 2);
    }
}
