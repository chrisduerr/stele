//! Vulkan renderer.

use std::process;
use std::ptr::NonNull;
use std::sync::Arc;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, WindowHandle,
};
use smallvec::smallvec;
use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
use smithay_client_toolkit::reexports::client::{Connection, Proxy};
use stele_ipc::Color;
use tracing::{error, info};
use vulkano::buffer::BufferContents;
use vulkano::command_buffer::allocator::StandardCommandBufferAllocator;
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, CommandBufferUsage, PrimaryAutoCommandBuffer, RenderPassBeginInfo,
};
use vulkano::device::physical::{PhysicalDevice, PhysicalDeviceType};
use vulkano::device::{
    Device, DeviceCreateInfo, DeviceExtensions, Queue, QueueCreateInfo, QueueFlags,
};
use vulkano::image::view::ImageView;
use vulkano::image::{Image, ImageUsage};
use vulkano::instance::{Instance, InstanceCreateInfo};
use vulkano::pipeline::graphics::GraphicsPipelineCreateInfo;
use vulkano::pipeline::graphics::color_blend::{
    AttachmentBlend, ColorBlendAttachmentState, ColorBlendState,
};
use vulkano::pipeline::graphics::input_assembly::InputAssemblyState;
use vulkano::pipeline::graphics::multisample::MultisampleState;
use vulkano::pipeline::graphics::rasterization::RasterizationState;
use vulkano::pipeline::graphics::vertex_input::{Vertex, VertexDefinition};
use vulkano::pipeline::graphics::viewport::{Viewport, ViewportState};
use vulkano::pipeline::layout::PipelineDescriptorSetLayoutCreateInfo;
use vulkano::pipeline::{
    DynamicState, GraphicsPipeline, PipelineLayout, PipelineShaderStageCreateInfo,
};
use vulkano::render_pass::{Framebuffer, FramebufferCreateInfo, RenderPass, Subpass};
use vulkano::swapchain::{
    Surface, Swapchain, SwapchainAcquireFuture, SwapchainCreateInfo, SwapchainPresentInfo,
};
use vulkano::sync::{self, GpuFuture};
use vulkano::{Validated, VulkanError, VulkanLibrary, single_pass_renderpass, swapchain};

use crate::Error;
use crate::geometry::Size;

mod vertex_shader {
    vulkano_shaders::shader! {
        ty: "vertex",
        path: "./shaders/vertex.glsl",
    }
}

mod fragment_shader {
    vulkano_shaders::shader! {
        ty: "fragment",
        path: "./shaders/fragment.glsl",
    }
}

/// Vulkan renderer.
pub struct Renderer {
    surface: Arc<Surface>,
    device: Arc<Device>,
    queue: Arc<Queue>,

    command_allocator: Arc<StandardCommandBufferAllocator>,

    sized: Option<SizedRenderer>,

    last_frame_end: Option<Box<dyn GpuFuture>>,
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
        let command_allocator =
            Arc::new(StandardCommandBufferAllocator::new(device.clone(), Default::default()));

        // Create Vulkan surface from Wayland surface.
        let surface = Surface::from_window(instance.clone(), Arc::new(surface_handle)).unwrap();

        Ok(Self {
            command_allocator,
            surface,
            device,
            queue,
            last_frame_end: Default::default(),
            sized: Default::default(),
        })
    }

    /// Perform drawing with this renderer mapped.
    pub fn draw<F: FnOnce(&AutoCommandBufferBuilder<PrimaryAutoCommandBuffer>)>(
        &mut self,
        size: Size,
        clear_color: Color,
        fun: F,
    ) {
        let render_pass = self.begin_render_pass(size, clear_color).unwrap();

        fun(&render_pass.command_builder);

        self.end_render_pass(render_pass).unwrap();
    }

    /// Start a new render pass.
    fn begin_render_pass(
        &mut self,
        size: Size,
        clear_color: Color,
    ) -> Result<ActiveRenderPass, Error> {
        let sized = self.sized(size);

        // Get the next framebuffer for rendering.
        let (framebuffer, present_info, framebuffer_future) = sized.next_image()?;
        let pipeline = sized.pipeline.clone();
        let viewport = sized.viewport.clone();

        // Create a new Vulkan command buffer.
        let mut command_builder = AutoCommandBufferBuilder::primary(
            self.command_allocator.clone(),
            self.queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )?;

        // Start the render pass.
        let mut render_pass_info = RenderPassBeginInfo::framebuffer(framebuffer);
        render_pass_info.clear_values = vec![Some(clear_color.into())];
        command_builder.begin_render_pass(render_pass_info, Default::default())?;

        // Update the viewport.
        command_builder.set_viewport(0, smallvec![viewport])?;

        // Bind the graphics pipeline.
        command_builder.bind_pipeline_graphics(pipeline)?;

        Ok(ActiveRenderPass { command_builder, present_info, framebuffer_future })
    }

    /// Finalize a render pass.
    fn end_render_pass(&mut self, mut render_pass: ActiveRenderPass) -> Result<(), Error> {
        // Complete render pass and collect all its Vulkan commands.
        render_pass.command_builder.end_render_pass(Default::default())?;
        let command_buffer = render_pass.command_builder.build()?;

        // Get time when last frame is done and new framebuffer is ready.
        let last_frame = self.last_frame_end.take();
        let frame_ready = last_frame
            .unwrap_or_else(|| sync::now(self.device.clone()).boxed())
            .join(render_pass.framebuffer_future);

        // Flush commands and swap buffers once the frame buffer is ready.
        let frame_done = frame_ready
            .then_execute(self.queue.clone(), command_buffer)?
            .then_swapchain_present(self.queue.clone(), render_pass.present_info)
            .then_signal_fence_and_flush()
            .inspect_err(|err| {
                if matches!(err, Validated::Error(VulkanError::OutOfDate))
                    && let Some(sized) = &mut self.sized
                {
                    sized.size = Size::default();
                }
            })?;
        self.last_frame_end = Some(frame_done.boxed());

        Ok(())
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
    framebuffers: Vec<Arc<Framebuffer>>,
    pipeline: Arc<GraphicsPipeline>,
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

        // Initialize shaders.
        let vertex_shader = vertex_shader::load(device.clone())?.entry_point("main").unwrap();
        let fragment_shader = fragment_shader::load(device.clone())?.entry_point("main").unwrap();
        let vertex_input_state = ImageVertex::per_vertex().definition(&vertex_shader).unwrap();
        let stages = smallvec![
            PipelineShaderStageCreateInfo::new(vertex_shader),
            PipelineShaderStageCreateInfo::new(fragment_shader),
        ];

        // Create pipeline layout.
        let layout_params = PipelineDescriptorSetLayoutCreateInfo::from_stages(&stages)
            .into_pipeline_layout_create_info(device.clone())?;
        let layout = PipelineLayout::new(device.clone(), layout_params)?;

        // Create the graphics pipeline.
        let subpass = Subpass::from(render_pass.clone(), 0).unwrap();
        let pipeline_info = GraphicsPipelineCreateInfo {
            stages,
            vertex_input_state: Some(vertex_input_state),
            input_assembly_state: Some(InputAssemblyState::default()),
            viewport_state: Some(ViewportState::default()),
            rasterization_state: Some(RasterizationState::default()),
            multisample_state: Some(MultisampleState::default()),
            color_blend_state: Some(ColorBlendState::with_attachment_states(
                subpass.num_color_attachments(),
                ColorBlendAttachmentState {
                    blend: Some(AttachmentBlend::alpha()),
                    ..Default::default()
                },
            )),
            dynamic_state: [DynamicState::Viewport].into_iter().collect(),
            subpass: Some(subpass.into()),
            ..GraphicsPipelineCreateInfo::layout(layout)
        };
        let pipeline = GraphicsPipeline::new(device.clone(), None, pipeline_info)?;

        Ok(Self { framebuffers, render_pass, swapchain, pipeline, viewport, size })
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

/// Vertex layout.
#[derive(BufferContents, Vertex)]
#[repr(C)]
struct ImageVertex {
    #[format(R32G32_SFLOAT)]
    position: [f32; 2],
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

/// Data for an in-progress render pass.
struct ActiveRenderPass {
    command_builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer>,
    framebuffer_future: SwapchainAcquireFuture,
    present_info: SwapchainPresentInfo,
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
