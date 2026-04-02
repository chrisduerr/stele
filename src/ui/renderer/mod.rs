//! Vulkan renderer.

use std::collections::{HashMap, LinkedList};
use std::hash::Hash;
use std::ops::Deref;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::Arc;
use std::{fs, process};

use image::{ColorType, ImageReader};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, WindowHandle,
};
use resvg::tiny_skia::Pixmap as SvgPixmap;
use resvg::usvg::{Options as SvgOptions, Transform as SvgTransform, Tree as SvgTree};
use smallvec::smallvec;
use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
use smithay_client_toolkit::reexports::client::{Connection, Proxy};
use stele_ipc::{Color, LayerFont};
use tracing::{error, info};
use vulkano::buffer::{Buffer, BufferContents, BufferCreateInfo, BufferUsage, Subbuffer};
use vulkano::command_buffer::allocator::StandardCommandBufferAllocator;
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, CommandBufferUsage, CopyBufferToImageInfo, PrimaryAutoCommandBuffer,
    PrimaryCommandBufferAbstract, RenderPassBeginInfo,
};
use vulkano::descriptor_set::allocator::StandardDescriptorSetAllocator;
use vulkano::descriptor_set::{DescriptorSet, WriteDescriptorSet};
use vulkano::device::physical::{PhysicalDevice, PhysicalDeviceType};
use vulkano::device::{
    Device, DeviceCreateInfo, DeviceExtensions, Queue, QueueCreateInfo, QueueFlags,
};
use vulkano::format::{ClearValue, Format};
use vulkano::image::sampler::{Filter, Sampler, SamplerCreateInfo};
use vulkano::image::view::ImageView;
use vulkano::image::{Image, ImageCreateInfo, ImageType, ImageUsage};
use vulkano::instance::{Instance, InstanceCreateInfo};
use vulkano::memory::allocator::{AllocationCreateInfo, MemoryTypeFilter, StandardMemoryAllocator};
use vulkano::pipeline::graphics::GraphicsPipelineCreateInfo;
use vulkano::pipeline::graphics::color_blend::{
    AttachmentBlend, BlendFactor, BlendOp, ColorBlendAttachmentState, ColorBlendState,
};
use vulkano::pipeline::graphics::input_assembly::InputAssemblyState;
use vulkano::pipeline::graphics::multisample::MultisampleState;
use vulkano::pipeline::graphics::rasterization::RasterizationState;
use vulkano::pipeline::graphics::vertex_input::{
    Vertex, VertexBufferDescription, VertexDefinition,
};
use vulkano::pipeline::graphics::viewport::{Viewport, ViewportState};
use vulkano::pipeline::layout::PipelineDescriptorSetLayoutCreateInfo;
use vulkano::pipeline::{
    DynamicState, GraphicsPipeline, Pipeline, PipelineBindPoint, PipelineLayout,
    PipelineShaderStageCreateInfo,
};
use vulkano::render_pass::{Framebuffer, FramebufferCreateInfo, RenderPass, Subpass};
use vulkano::shader::ShaderModule;
use vulkano::swapchain::{
    Surface, Swapchain, SwapchainAcquireFuture, SwapchainCreateInfo, SwapchainPresentInfo,
};
use vulkano::sync::{self, GpuFuture};
use vulkano::{Validated, VulkanError, VulkanLibrary, single_pass_renderpass, swapchain};

use crate::Error;
use crate::geometry::{Point, Size};
use crate::ui::renderer::text::Rasterizer;

/// Maximum number of cached image textures.
const MAX_IMAGE_TEXTURES: usize = 50;

/// Maximum number of cached text textures.
const MAX_TEXT_TEXTURES: usize = 50;

mod shaders;
mod text;

/// Vulkan renderer.
pub struct Renderer {
    sampler: Arc<Sampler>,
    surface: Arc<Surface>,
    device: Arc<Device>,
    queue: Arc<Queue>,

    descriptor_allocator: Arc<StandardDescriptorSetAllocator>,
    command_allocator: Arc<StandardCommandBufferAllocator>,
    memory_allocator: Arc<StandardMemoryAllocator>,

    last_frame_end: Option<Box<dyn GpuFuture>>,

    sized: Option<SizedRenderer>,

    rasterizer: Rasterizer,

    image_textures: LruMap<ImageTextureKey, Texture>,
    text_textures: LruMap<TextKey, Texture>,
}

impl Renderer {
    pub fn new(connection: Connection, surface: WlSurface) -> Result<Self, Error> {
        // Get display from the Wayland connection.
        let surface_handle = SurfaceHandle::new(connection, surface);

        // Initialize Vulkan library instance.
        let library = VulkanLibrary::new()?;
        let required_extensions = Surface::required_extensions(&surface_handle).unwrap();
        let instance = Instance::new(library, InstanceCreateInfo {
            enabled_extensions: required_extensions,
            ..Default::default()
        })?;

        // Determine ideal Vulkan device.
        let device_extensions = DeviceExtensions { khr_swapchain: true, ..Default::default() };
        let physical_devices = instance.enumerate_physical_devices()?;
        let (physical_device, queue_family_index) = physical_devices
            .filter(|device| device.supported_extensions().contains(&device_extensions))
            .filter_map(|device| graphics_queue_index(&surface_handle, device))
            .min_by_key(device_priority)
            .ok_or(Error::VulkanNoDevice)?;

        info!("Using Vulkan device {:?}", physical_device.properties().device_name);

        // Create the Vulkan device.
        let (device, mut queues) = Device::new(physical_device, DeviceCreateInfo {
            enabled_extensions: device_extensions,
            queue_create_infos: vec![QueueCreateInfo { queue_family_index, ..Default::default() }],
            ..Default::default()
        })?;
        let queue = queues.next().unwrap();

        // Create Vulkan allocators.
        let descriptor_allocator =
            Arc::new(StandardDescriptorSetAllocator::new(device.clone(), Default::default()));
        let command_allocator =
            Arc::new(StandardCommandBufferAllocator::new(device.clone(), Default::default()));
        let memory_allocator = Arc::new(StandardMemoryAllocator::new_default(device.clone()));

        // Create Vulkan image sampler.
        let sampler_info = SamplerCreateInfo {
            mag_filter: Filter::Linear,
            min_filter: Filter::Linear,
            ..Default::default()
        };
        let sampler = Sampler::new(device.clone(), sampler_info)?;

        // Create Vulkan surface from Wayland surface.
        let surface = Surface::from_window(instance.clone(), Arc::new(surface_handle)).unwrap();

        Ok(Self {
            descriptor_allocator,
            command_allocator,
            memory_allocator,
            sampler,
            surface,
            device,
            queue,
            image_textures: LruMap::new(MAX_IMAGE_TEXTURES),
            text_textures: LruMap::new(MAX_TEXT_TEXTURES),
            last_frame_end: Default::default(),
            rasterizer: Default::default(),
            sized: Default::default(),
        })
    }

    /// Perform drawing with this renderer mapped.
    pub fn draw<F>(&mut self, size: Size, clear_color: Color, fun: F)
    where
        F: FnOnce(&mut ActiveRenderPass<'_>),
    {
        let mut render_pass = match self.begin_render_pass(size, clear_color) {
            Ok(render_pass) => render_pass,
            Err(err) => {
                error!("Failed to start Vulkan render pass: {err}");
                return;
            },
        };

        fun(&mut render_pass);

        if let Err(err) = render_pass.end() {
            error!("Failed to finish Vulkan render pass: {err}");
        }
    }

    /// Start a new render pass.
    fn begin_render_pass<'a>(
        &'a mut self,
        size: Size,
        clear_color: Color,
    ) -> Result<ActiveRenderPass<'a>, Error> {
        let sized = self.sized(size);

        // Get the next framebuffer for rendering.
        let (framebuffer, present_info, framebuffer_future) = sized.next_image()?;
        let texture_pipeline = sized.texture_pipeline.clone();
        let color_pipeline = sized.color_pipeline.clone();
        let viewport = sized.viewport.clone();

        // Create a new Vulkan command buffer.
        let mut command_builder = AutoCommandBufferBuilder::primary(
            self.command_allocator.clone(),
            self.queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )?;

        // Start the render pass.
        let mut render_pass_info = RenderPassBeginInfo::framebuffer(framebuffer);
        render_pass_info.clear_values = vec![Some(ClearValue::Float(clear_color.into()))];
        command_builder.begin_render_pass(render_pass_info, Default::default())?;

        // Update the viewport.
        command_builder.set_viewport(0, smallvec![viewport])?;

        Ok(ActiveRenderPass {
            framebuffer_future,
            texture_pipeline,
            command_builder,
            color_pipeline,
            present_info,
            size,
            renderer: self,
        })
    }

    /// Get the size of a line of text.
    pub fn text_size(&mut self, font: &LayerFont, text: &str) -> Option<Size> {
        let layout = self.rasterizer.layout(font);
        layout.set_text(text);
        layout.set_width(-1);

        match layout.pixel_size() {
            (width @ 1.., height @ 1..) => Some(Size::new(width as u32, height as u32)),
            _ => None,
        }
    }

    /// Rasterize text and cache it as a Vulkan texture.
    pub fn load_text(
        &mut self,
        font: LayerFont,
        size: Size,
        text: Arc<String>,
    ) -> Result<Texture, Error> {
        // Try to load texture from cache.
        let key = TextKey::new(font.clone(), size, text);
        if let Some(texture) = self.text_textures.get(&key) {
            return Ok(texture.clone());
        }

        // Rasterize the text.
        let (data, size) = self.rasterizer.rasterize(&font, size.into(), &key.text);
        let image = self.create_texture(&data, size, Format::R8G8B8A8_UNORM)?;
        let texture = Texture { image, is_premultiplied: true };

        // Cache the rasterized texture.
        self.text_textures.insert(key, texture.clone());

        Ok(texture)
    }

    /// Load an image resource using its path.
    ///
    /// The `size` passed is only used for scalable resources. Images like PNGs
    /// are loaded at their original size and scaled on the GPU.
    pub fn load_resource(&mut self, size: Size, path: &Arc<PathBuf>) -> Result<Texture, Error> {
        if path.as_ref().extension().is_some_and(|ext| ext == "svg") {
            self.load_svg(size, path)
        } else {
            self.load_image(path)
        }
    }

    /// Load an SVG using its path.
    pub fn load_svg(&mut self, size: Size, path: &Arc<PathBuf>) -> Result<Texture, Error> {
        // Try to load texture from cache.
        let key = ImageTextureKey::Svg(size, path.clone());
        if let Some(texture) = self.image_textures.get(&key) {
            return Ok(texture.clone());
        }

        // Parse SVG data.
        let data = fs::read(&**path)?;
        let svg_tree = SvgTree::from_data(&data, &SvgOptions::default())?;

        // Calculate transforms to center SVG inside target buffer.
        let tree_size = svg_tree.size();
        let svg_width = tree_size.width();
        let svg_height = tree_size.height();
        let x_scale = size.width as f32 / svg_width;
        let y_scale = size.height as f32 / svg_height;
        let transform = SvgTransform::from_scale(x_scale, y_scale);

        // Render SVG into CPU buffer.
        let mut pixmap = SvgPixmap::new(size.width, size.height).unwrap();
        resvg::render(&svg_tree, transform, &mut pixmap.as_mut());

        // Upload SVG data to the GPU.
        let image = self.create_texture(pixmap.data(), size, Format::R8G8B8A8_UNORM)?;
        let texture = Texture { image, is_premultiplied: false };

        // Cache the rasterized texture.
        self.image_textures.insert(key, texture.clone());

        Ok(texture)
    }

    /// Load an image using its path.
    pub fn load_image(&mut self, path: &Arc<PathBuf>) -> Result<Texture, Error> {
        // Try to load texture from cache.
        let key = ImageTextureKey::Image(path.clone());
        if let Some(texture) = self.image_textures.get(&key) {
            return Ok(texture.clone());
        }

        // Load the image.
        let image = ImageReader::open(&**path)?.decode()?;

        // Convert image format to Vulkan image format.
        let format = match image.color() {
            ColorType::Rgb8 => Format::R8G8B8_UNORM,
            ColorType::Rgba8 => Format::R8G8B8A8_UNORM,
            ColorType::Rgb16 => Format::R16G16B16_UNORM,
            ColorType::Rgba16 => Format::R16G16B16A16_UNORM,
            ColorType::Rgb32F => Format::R32G32B32_SFLOAT,
            ColorType::Rgba32F => Format::R32G32B32A32_SFLOAT,
            format => return Err(Error::UnsupportedImageFormat(format)),
        };

        // Upload image to the GPU.
        let image_size = Size::new(image.width(), image.height());
        let image = self.create_texture(image.as_bytes(), image_size, format)?;
        let texture = Texture { image, is_premultiplied: false };

        // Cache the rasterized texture.
        self.image_textures.insert(key, texture.clone());

        Ok(texture)
    }

    /// Upload a new Vulkan texture.
    fn create_texture(
        &self,
        data: &[u8],
        size: Size,
        format: Format,
    ) -> Result<Arc<ImageView>, Error> {
        // Allocate Vulkan CPU buffer for the image.
        let buffer_info =
            BufferCreateInfo { usage: BufferUsage::TRANSFER_SRC, ..Default::default() };
        let allocation_info = AllocationCreateInfo {
            memory_type_filter: MemoryTypeFilter::PREFER_HOST
                | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
            ..Default::default()
        };
        let allocator = self.memory_allocator.clone();
        let upload_buffer =
            Buffer::new_slice(allocator.clone(), buffer_info, allocation_info, data.len() as u64)?;
        upload_buffer.write().unwrap().copy_from_slice(data);

        // Create Vulkan texture.
        let image_info = ImageCreateInfo {
            format,
            usage: ImageUsage::TRANSFER_DST | ImageUsage::SAMPLED,
            image_type: ImageType::Dim2d,
            extent: [size.width, size.height, 1],
            ..Default::default()
        };
        let image = Image::new(allocator, image_info, AllocationCreateInfo::default())?;

        // Upload image to the GPU.
        let mut command_builder = AutoCommandBufferBuilder::primary(
            self.command_allocator.clone(),
            self.queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )?;
        let copy_info = CopyBufferToImageInfo::buffer_image(upload_buffer, image.clone());
        command_builder.copy_buffer_to_image(copy_info)?;
        let _ = command_builder.build()?.execute(self.queue.clone())?;

        Ok(ImageView::new_default(image)?)
    }

    /// Crate a buffer for a fixed number of vertices.
    fn create_vertex_buffer<T, const N: usize>(
        &self,
        buffer: [T; N],
    ) -> Result<Subbuffer<[T]>, Error>
    where
        T: Default + Copy + BufferContents,
    {
        let allocator = self.memory_allocator.clone();

        let buffer_info =
            BufferCreateInfo { usage: BufferUsage::VERTEX_BUFFER, ..Default::default() };

        let allocation_info = AllocationCreateInfo {
            memory_type_filter: MemoryTypeFilter::PREFER_DEVICE
                | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
            ..Default::default()
        };

        Ok(Buffer::from_iter(allocator, buffer_info, allocation_info, buffer)?)
    }

    /// Get render state requiring a size.
    fn sized(&mut self, size: Size) -> &mut SizedRenderer {
        // Initialize or resize sized state.
        match &mut self.sized {
            // Resize renderer.
            Some(sized) => sized.resize(size),
            // Create sized state.
            None => match SizedRenderer::new(&self.device, &self.surface, size) {
                Ok(sized) => self.sized = Some(sized),
                Err(err) => {
                    error!(?err, "Failed to create Vulkan framebuffers");
                    process::exit(1);
                },
            },
        }

        self.sized.as_mut().unwrap()
    }
}

/// Render state requiring known size.
///
/// This state is initialized on-demand, to avoid Mesa's issue with resizing
/// before the first draw.
#[derive(Debug)]
pub struct SizedRenderer {
    texture_pipeline: Arc<GraphicsPipeline>,
    color_pipeline: Arc<GraphicsPipeline>,
    framebuffers: Vec<Arc<Framebuffer>>,
    render_pass: Arc<RenderPass>,
    swapchain: Arc<Swapchain>,
    viewport: Viewport,

    size: Size,
}

impl SizedRenderer {
    /// Create sized renderer state.
    fn new(device: &Arc<Device>, surface: &Arc<Surface>, size: Size) -> Result<Self, Error> {
        // Get supported image format & capabilities for the device.
        let phys_device = device.physical_device();
        let surface_capabilities = phys_device.surface_capabilities(surface, Default::default())?;
        let (image_format, _) = phys_device.surface_formats(surface, Default::default())?[0];

        // Create swapchain with its images.
        let surface = surface.clone();
        let composite_alpha =
            surface_capabilities.supported_composite_alpha.into_iter().next().unwrap();
        let (swapchain, images) = Swapchain::new(device.clone(), surface, SwapchainCreateInfo {
            composite_alpha,
            image_format,
            min_image_count: surface_capabilities.min_image_count.max(2),
            image_usage: ImageUsage::COLOR_ATTACHMENT,
            image_extent: size.into(),
            ..Default::default()
        })?;

        // Create render pass.
        let render_pass = single_pass_renderpass!(
            device.clone(),
            attachments: {
                color: {
                    format: swapchain.image_format(),
                    samples: 1,
                    load_op: Clear,
                    store_op: Store,
                },
            },
            pass: {
                color: [color],
                depth_stencil: {},
            },
        )?;

        // Create framebuffers.
        let framebuffers = Self::create_framebuffers(&render_pass, images)?;

        // Create Vulkan viewport.
        let viewport = Viewport { extent: size.into(), offset: [0., 0.], depth_range: 0.0..=1. };

        // Create pipeline for texture rendering.
        let texture_pipeline = Self::create_pipeline(
            device.clone(),
            render_pass.clone(),
            shaders::texture::vertex::load,
            shaders::texture::fragment::load,
            ImageVertex::per_vertex(),
        )?;

        // Create pipeline for single-color rectangles.
        let color_pipeline = Self::create_pipeline(
            device.clone(),
            render_pass.clone(),
            shaders::color::vertex::load,
            shaders::color::fragment::load,
            ColorVertex::per_vertex(),
        )?;

        Ok(Self {
            texture_pipeline,
            color_pipeline,
            framebuffers,
            render_pass,
            swapchain,
            viewport,
            size,
        })
    }

    /// Create a new graphics pipeline.
    fn create_pipeline<F, V>(
        device: Arc<Device>,
        render_pass: Arc<RenderPass>,
        vertex_shader_loader: V,
        fragment_shader_loader: F,
        vertex_description: VertexBufferDescription,
    ) -> Result<Arc<GraphicsPipeline>, Error>
    where
        F: FnOnce(Arc<Device>) -> Result<Arc<ShaderModule>, Validated<VulkanError>>,
        V: FnOnce(Arc<Device>) -> Result<Arc<ShaderModule>, Validated<VulkanError>>,
    {
        // Initialize shaders.
        let vertex_shader = vertex_shader_loader(device.clone())?.entry_point("main").unwrap();
        let fragment_shader = fragment_shader_loader(device.clone())?.entry_point("main").unwrap();
        let vertex_input_state = vertex_description.definition(&vertex_shader).unwrap();
        let stages = smallvec![
            PipelineShaderStageCreateInfo::new(vertex_shader),
            PipelineShaderStageCreateInfo::new(fragment_shader),
        ];

        // Create pipeline layout.
        let layout_params = PipelineDescriptorSetLayoutCreateInfo::from_stages(&stages)
            .into_pipeline_layout_create_info(device.clone())?;
        let pipeline_layout = PipelineLayout::new(device.clone(), layout_params)?;

        // Create blend config for premultiplied alpha.
        let blend = Some(AttachmentBlend {
            src_color_blend_factor: BlendFactor::One,
            dst_color_blend_factor: BlendFactor::OneMinusSrcAlpha,
            color_blend_op: BlendOp::Add,
            src_alpha_blend_factor: BlendFactor::One,
            dst_alpha_blend_factor: BlendFactor::Zero,
            alpha_blend_op: BlendOp::Add,
        });

        // Create the graphics pipeline for texture rendering.
        let subpass = Subpass::from(render_pass, 0).unwrap();
        let pipeline_info = GraphicsPipelineCreateInfo {
            stages,
            vertex_input_state: Some(vertex_input_state),
            input_assembly_state: Some(InputAssemblyState::default()),
            viewport_state: Some(ViewportState::default()),
            rasterization_state: Some(RasterizationState::default()),
            multisample_state: Some(MultisampleState::default()),
            color_blend_state: Some(ColorBlendState::with_attachment_states(
                subpass.num_color_attachments(),
                ColorBlendAttachmentState { blend, ..Default::default() },
            )),
            dynamic_state: [DynamicState::Viewport].into_iter().collect(),
            subpass: Some(subpass.into()),
            ..GraphicsPipelineCreateInfo::layout(pipeline_layout.clone())
        };
        let pipeline = GraphicsPipeline::new(device, None, pipeline_info)?;

        Ok(pipeline)
    }

    /// Resize the renderer.
    fn resize(&mut self, size: Size) {
        if self.size == size {
            return;
        }
        self.size = size;

        // Recreate the Vulkan swapchain.
        let swapchain = self.swapchain.recreate(SwapchainCreateInfo {
            image_extent: size.into(),
            ..self.swapchain.create_info()
        });
        let swapchain_framebuffers = swapchain.and_then(|(swapchain, images)| {
            let framebuffers = Self::create_framebuffers(&self.render_pass, images)?;
            Ok((swapchain, framebuffers))
        });
        match swapchain_framebuffers {
            Ok((swapchain, framebuffers)) => {
                self.viewport.extent = size.into();
                self.framebuffers = framebuffers;
                self.swapchain = swapchain;
            },
            Err(err) => error!("Failed to recreate Vulkan swapchain: {err}"),
        }
    }

    /// Get the next swapchain image.
    fn next_image(
        &mut self,
    ) -> Result<(Arc<Framebuffer>, SwapchainPresentInfo, SwapchainAcquireFuture), Error> {
        let next_image = swapchain::acquire_next_image(self.swapchain.clone(), None);
        let (image_index, suboptimal, framebuffer_future) = next_image
            // Recreate swapchain without drawing if it is out of date.
            .inspect_err(|err| if matches!(err, Validated::Error(VulkanError::OutOfDate)) {
                self.size = Size::default();
            })?;

        // Recreate swapchain before next render if image is suboptimal.
        if suboptimal {
            self.size = Size::default();
        }

        let framebuffer = self.framebuffers[image_index as usize].clone();

        let present_info =
            SwapchainPresentInfo::swapchain_image_index(self.swapchain.clone(), image_index);

        Ok((framebuffer, present_info, framebuffer_future))
    }

    /// Create Vulkan framebuffers.
    fn create_framebuffers(
        render_pass: &Arc<RenderPass>,
        images: Vec<Arc<Image>>,
    ) -> Result<Vec<Arc<Framebuffer>>, Validated<VulkanError>> {
        images
            .into_iter()
            .map(|image| {
                let view = ImageView::new_default(image)?;

                Framebuffer::new(render_pass.clone(), FramebufferCreateInfo {
                    attachments: vec![view],
                    ..Default::default()
                })
            })
            .collect()
    }
}

/// Data for an in-progress render pass.
pub struct ActiveRenderPass<'a> {
    command_builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer>,
    framebuffer_future: SwapchainAcquireFuture,
    texture_pipeline: Arc<GraphicsPipeline>,
    color_pipeline: Arc<GraphicsPipeline>,
    present_info: SwapchainPresentInfo,

    renderer: &'a mut Renderer,

    size: Size,
}

impl<'a> ActiveRenderPass<'a> {
    /// Render an image.
    pub fn draw_texture(
        &mut self,
        texture: Texture,
        point: Point<f32>,
        size: Size<f32>,
    ) -> Result<(), Error> {
        let extent = texture.image().extent();

        // Switch to texture rendering shaders.
        let pipeline_layout = self.texture_pipeline.layout().clone();
        self.command_builder.bind_pipeline_graphics(self.texture_pipeline.clone())?;

        // Create descriptor set for the texture.
        let descriptor_allocator = self.renderer.descriptor_allocator.clone();
        let sampler = self.renderer.sampler.clone();
        let layout = pipeline_layout.set_layouts()[0].clone();
        let descriptors = [WriteDescriptorSet::image_view_sampler(0, texture.image, sampler)];
        let descriptor_set = DescriptorSet::new(descriptor_allocator, layout, descriptors, [])?;

        // Bind descriptor set to the active pipeline.
        self.command_builder.bind_descriptor_sets(
            PipelineBindPoint::Graphics,
            pipeline_layout,
            0,
            descriptor_set,
        )?;

        // Calculate size transform.
        let x_scale = size.width / extent[0] as f32;
        let y_scale = size.height / extent[1] as f32;

        // Determine whether shaders need to premultiply the alapha channel.
        let is_premultiplied = if texture.is_premultiplied { 1. } else { 0. };

        // Calculate image vertices.
        let x = -1. + 2. * point.x / self.size.width as f32;
        let y = -1. + 2. * point.y / self.size.height as f32;
        let width = 2. * extent[0] as f32 * x_scale / self.size.width as f32;
        let height = 2. * extent[1] as f32 * y_scale / self.size.height as f32;
        let vertex_buffer = self.renderer.create_vertex_buffer([
            // Top Left -> Bottom Left -> Top Right
            ImageVertex { position: [x, y], uv: [0., 0.], is_premultiplied },
            ImageVertex { position: [x, y + height], uv: [0., 1.], is_premultiplied },
            ImageVertex { position: [x + width, y], uv: [1., 0.], is_premultiplied },
            // Top Right -> Bottom Left -> Bottom Right
            ImageVertex { position: [x + width, y], uv: [1., 0.], is_premultiplied },
            ImageVertex { position: [x, y + height], uv: [0., 1.], is_premultiplied },
            ImageVertex { position: [x + width, y + height], uv: [1., 1.], is_premultiplied },
        ])?;

        // Bind vertex buffer to the active pipeline.
        let vertex_count = vertex_buffer.len() as u32;
        self.command_builder.bind_vertex_buffers(0, vertex_buffer)?;

        // Render image vertices.
        unsafe { self.command_builder.draw(vertex_count, 1, 0, 0) }?;

        Ok(())
    }

    /// Render a single-color rectangle.
    pub fn draw_color(
        &mut self,
        color: Color,
        point: Point<f32>,
        size: Size<f32>,
    ) -> Result<(), Error> {
        // Switch to color rendering shaders.
        self.command_builder.bind_pipeline_graphics(self.color_pipeline.clone())?;

        // Convert color to premultiplied alpha.
        let mut color: [f32; 4] = color.into();
        color[0] *= color[3];
        color[1] *= color[3];
        color[2] *= color[3];

        // Calculate vertices.
        let x = -1. + 2. * point.x / self.size.width as f32;
        let y = -1. + 2. * point.y / self.size.height as f32;
        let width = 2. * size.width / self.size.width as f32;
        let height = 2. * size.height / self.size.height as f32;
        let vertex_buffer = self.renderer.create_vertex_buffer([
            // Top Left -> Bottom Left -> Top Right
            ColorVertex { position: [x, y], color },
            ColorVertex { position: [x, y + height], color },
            ColorVertex { position: [x + width, y], color },
            // Top Right -> Bottom Left -> Bottom Right
            ColorVertex { position: [x + width, y], color },
            ColorVertex { position: [x, y + height], color },
            ColorVertex { position: [x + width, y + height], color },
        ])?;

        // Bind vertex buffer to the active pipeline.
        let vertex_count = vertex_buffer.len() as u32;
        self.command_builder.bind_vertex_buffers(0, vertex_buffer)?;

        // Render image vertices.
        unsafe { self.command_builder.draw(vertex_count, 1, 0, 0) }?;

        Ok(())
    }

    /// Finalize a render pass.
    fn end(mut self) -> Result<(), Error> {
        // Complete render pass and collect all its Vulkan commands.
        self.command_builder.end_render_pass(Default::default())?;
        let command_buffer = self.command_builder.build()?;

        // Get time when last frame is done and new framebuffer is ready.
        let last_frame = self.renderer.last_frame_end.take();
        let frame_ready = last_frame
            .unwrap_or_else(|| sync::now(self.renderer.device.clone()).boxed())
            .join(self.framebuffer_future);

        // Flush commands and swap buffers once the frame buffer is ready.
        let frame_done = frame_ready
            .then_execute(self.renderer.queue.clone(), command_buffer)?
            .then_swapchain_present(self.renderer.queue.clone(), self.present_info)
            .then_signal_fence_and_flush()
            .inspect_err(|err| {
                if matches!(err, Validated::Error(VulkanError::OutOfDate))
                    && let Some(sized) = &mut self.renderer.sized
                {
                    sized.size = Size::default();
                }
            })?;
        self.renderer.last_frame_end = Some(frame_done.boxed());

        Ok(())
    }
}

/// Texture vertex layout.
#[derive(BufferContents, Vertex, Copy, Clone, Default, Debug)]
#[repr(C)]
struct ImageVertex {
    #[format(R32G32_SFLOAT)]
    position: [f32; 2],
    #[format(R32G32_SFLOAT)]
    uv: [f32; 2],
    #[format(R32_SFLOAT)]
    is_premultiplied: f32,
}

/// Color vertex layout.
#[derive(BufferContents, Vertex, Copy, Clone, Default, Debug)]
#[repr(C)]
struct ColorVertex {
    #[format(R32G32_SFLOAT)]
    position: [f32; 2],
    #[format(R32G32B32A32_SFLOAT)]
    color: [f32; 4],
}

/// Capacity-constrained LRU cache map.
pub struct LruMap<K, V> {
    map: HashMap<K, V>,
    lru: LinkedList<K>,
    capacity: usize,
}

impl<K, V> LruMap<K, V>
where
    K: Hash + PartialEq + Eq + Clone,
{
    pub fn new(capacity: usize) -> Self {
        Self { capacity, map: Default::default(), lru: Default::default() }
    }

    /// Add a new entry to the cache.
    pub fn insert(&mut self, key: K, value: V) {
        if self.map.contains_key(&key) {
            // Remove old LRU entry if the value already exists.
            self.lru.extract_if(|cached| *cached == key).take(1).for_each(drop);
        } else {
            // Remove oldest entry if cache is full.
            while self.map.len() >= self.capacity {
                let key = self.lru.pop_back().unwrap();
                self.map.remove(&key);
            }

            // Add tile to the cache.
            self.map.insert(key.clone(), value);
        }

        // Mark item as the least-recently used.
        self.lru.push_front(key);
    }

    /// Check if a key exists.
    pub fn contains_key(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    /// Get a mutable reference to the value using its key.
    pub fn get(&mut self, key: &K) -> Option<&mut V> {
        // Mark entry as most recently used.
        let lru_key = self.lru.extract_if(|cached| cached == key).next()?;
        self.lru.push_front(lru_key);

        self.map.get_mut(key)
    }
}

/// LRU cache key for image textures.
#[derive(Hash, PartialEq, Eq, Clone)]
enum ImageTextureKey {
    Svg(Size, Arc<PathBuf>),
    Image(Arc<PathBuf>),
}

/// Cache key for text images.
#[derive(Hash, PartialEq, Eq, Clone)]
struct TextKey {
    family: Option<Arc<String>>,
    font_size: Option<u32>,
    color: Option<Color>,

    text: Arc<String>,
    size: Size,
}

impl TextKey {
    fn new(font: LayerFont, size: Size, text: Arc<String>) -> Self {
        let font_size = font.size.map(|size| (size * 1_000.).round() as u32);

        Self { font_size, size, text, family: font.family, color: font.color }
    }
}

/// Vulkan image.
#[derive(Clone)]
pub struct Texture {
    image: Arc<ImageView>,
    is_premultiplied: bool,
}

impl Deref for Texture {
    type Target = ImageView;

    fn deref(&self) -> &Self::Target {
        &self.image
    }
}

/// Wayland interface pointers.
struct SurfaceHandle {
    connection: Connection,
    surface: WlSurface,
}

impl SurfaceHandle {
    fn new(connection: Connection, surface: WlSurface) -> Self {
        Self { connection, surface }
    }
}

impl HasDisplayHandle for SurfaceHandle {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let display_ptr = NonNull::new(self.connection.backend().display_ptr().cast()).unwrap();
        let wayland_display = WaylandDisplayHandle::new(display_ptr);
        let display = RawDisplayHandle::Wayland(wayland_display);
        Ok(unsafe { DisplayHandle::borrow_raw(display) })
    }
}

impl HasWindowHandle for SurfaceHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let surface_ptr = NonNull::new(self.surface.id().as_ptr().cast()).unwrap();
        let wayland_window = WaylandWindowHandle::new(surface_ptr);
        let window = RawWindowHandle::Wayland(wayland_window);
        Ok(unsafe { WindowHandle::borrow_raw(window) })
    }
}

/// Extract the graphics queue's index from a physical Vulkan device.
fn graphics_queue_index(
    display: &impl HasDisplayHandle,
    device: Arc<PhysicalDevice>,
) -> Option<(Arc<PhysicalDevice>, u32)> {
    let mut queue_properties = device.queue_family_properties().iter().enumerate();
    let queue_index = queue_properties.position(|(i, queue)| {
        queue.queue_flags.intersects(QueueFlags::GRAPHICS)
            && device.presentation_support(i as u32, display).unwrap_or(false)
    })?;

    Some((device, queue_index as u32))
}

/// Get the device priority based on device type.
///
/// A lower priority indicates the device is more optimal.
fn device_priority((device, _queue_index): &(Arc<PhysicalDevice>, u32)) -> usize {
    match device.properties().device_type {
        PhysicalDeviceType::DiscreteGpu => 0,
        PhysicalDeviceType::IntegratedGpu => 1,
        PhysicalDeviceType::VirtualGpu => 2,
        PhysicalDeviceType::Cpu => 3,
        PhysicalDeviceType::Other => 4,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_map_insert() {
        let mut cache = LruMap::<i8, i8>::new(10);

        for i in 0..15 {
            cache.insert(i, -i);
        }

        assert_eq!(cache.get(&0), None);
        assert_eq!(cache.get(&4), None);
        assert_eq!(cache.get(&5), Some(&mut -5));
        assert_eq!(cache.get(&14), Some(&mut -14));
    }

    #[test]
    fn lru_map_access() {
        let mut cache = LruMap::<i8, i8>::new(10);

        for i in 0..10 {
            cache.insert(i, -i);
        }

        // Access keys to update the last usage.
        assert_eq!(cache.get(&0), Some(&mut 0));
        assert_eq!(cache.get(&2), Some(&mut -2));

        cache.insert(10, -10);
        cache.insert(11, -11);
        cache.insert(12, -12);

        assert_eq!(cache.get(&0), Some(&mut 0));
        assert_eq!(cache.get(&1), None);
        assert_eq!(cache.get(&2), Some(&mut -2));
        assert_eq!(cache.get(&3), None);
        assert_eq!(cache.get(&4), None);
        assert_eq!(cache.get(&5), Some(&mut -5));
        assert_eq!(cache.get(&10), Some(&mut -10));
        assert_eq!(cache.get(&11), Some(&mut -11));
        assert_eq!(cache.get(&12), Some(&mut -12));
    }
}
