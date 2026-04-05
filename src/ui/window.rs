//! Wayland window management.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::{fs, mem};

use smallvec::SmallVec;
use smithay_client_toolkit::compositor::{CompositorState, Region};
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use stele_ipc::{Alignment, Color, Config, LayerContent, LayerFont, Margin, Module, ModuleLayer};
use tracing::error;

use crate::geometry::{Point, Size};
use crate::ui::renderer::{ActiveRenderPass, ImageResourceId, Renderer, Texture};
use crate::wayland::ProtocolStates;
use crate::{Error, State};

/// Default bar size.
const DEFAULT_SIZE: u32 = 35;

/// Wayland window.
pub struct Window {
    queue: QueueHandle<State>,
    connection: Connection,
    window: LayerSurface,
    viewport: WpViewport,

    background_textures: Vec<Texture>,
    renderer: Renderer,

    modules: Modules,
    config: Config,

    size: Size,
    scale: f64,

    initial_configure_done: bool,
    stalled: bool,
    dirty: bool,
}

impl Window {
    pub fn new(
        protocol_states: &ProtocolStates,
        connection: Connection,
        queue: QueueHandle<State>,
        config: Config,
        modules: HashMap<Arc<String>, Module>,
    ) -> Result<Self, Error> {
        // Create surface's Wayland global handles.
        let surface = protocol_states.compositor.create_surface(&queue);
        if let Some(fractional_scale) = &protocol_states.fractional_scale {
            fractional_scale.fractional_scaling(&queue, &surface);
        }
        let viewport = protocol_states.viewporter.viewport(&queue, &surface);

        // Get target output from its name.
        let output = protocol_states.output.outputs().find(|output| {
            protocol_states.output.info(output).and_then(|info| info.name) == config.output
        });

        // Create the layer shell window.
        let size = config.size.unwrap_or(DEFAULT_SIZE);
        let window = protocol_states.layer.create_layer_surface(
            &queue,
            surface.clone(),
            config.layer.into(),
            Some("panel"),
            output.as_ref(),
        );
        window.set_anchor(config.edge.into());
        window.set_size(0, size);
        window.set_exclusive_zone(size as i32);
        window.commit();

        // Create Vulkan renderer.
        let renderer = Renderer::new(connection.clone(), surface)?;

        // Default to a reasonable default size.
        let size = Size { width: 360, height: 720 };

        Ok(Self {
            connection,
            renderer,
            viewport,
            config,
            window,
            queue,
            size,
            modules: modules.into(),
            stalled: true,
            dirty: true,
            scale: 1.,
            initial_configure_done: Default::default(),
            background_textures: Default::default(),
        })
    }

    /// Redraw the window.
    pub fn draw(&mut self) {
        // Stall rendering if nothing changed since last redraw.
        if !mem::take(&mut self.dirty) || !self.initial_configure_done {
            self.stalled = true;
            return;
        }

        // Get background colors and textures.
        let clear_color = self.prepare_background();

        // Prepare modules for rendering.
        self.prepare_modules();

        // Update viewporter logical render size.
        //
        // NOTE: This must be done every time we draw with Sway; it is not
        // persisted when drawing with the same surface multiple times.
        self.viewport.set_destination(self.size.width as i32, self.size.height as i32);

        // Mark entire window as damaged.
        let wl_surface = self.window.wl_surface();
        wl_surface.damage(0, 0, self.size.width as i32, self.size.height as i32);

        // Render the window content.
        let physical_size = self.size * self.scale;
        self.renderer.draw(physical_size, clear_color, |render_pass| {
            // Draw background.
            let point = Point::new(0., 0.);
            for texture in self.background_textures.iter().cloned() {
                if let Err(err) = render_pass.draw_texture(texture, point, physical_size.into()) {
                    error!("Failed to draw texture: {err}");
                }
            }

            // Draw modules.
            Self::draw_modules(render_pass, &self.modules.start, Point::default());
            Self::draw_modules(render_pass, &self.modules.center, self.modules.center_offset);
            Self::draw_modules(render_pass, &self.modules.end, self.modules.end_offset);
        });

        // Request a new frame.
        wl_surface.frame(&self.queue, wl_surface.clone());

        // Apply surface changes.
        wl_surface.commit();
    }

    /// Update background textures and get the desired clear color.
    fn prepare_background(&mut self) -> Color {
        let physical_size = self.size * self.scale;

        let mut clear_color = Color::default();
        self.background_textures.clear();

        for background in &self.config.backgrounds {
            let texture = match background {
                // Colors override all previous backgrounds.
                LayerContent::Color(color) => {
                    self.background_textures.clear();
                    clear_color = *color;
                    continue;
                },
                // Upload image/SVG using its path.
                LayerContent::Path(path) => self.renderer.load_resource(physical_size, path),
                // Upload image using its file content.
                &LayerContent::Image { id, data } => self.renderer.load_image(id.into(), data),
                // Upload SVG using its file content.
                &LayerContent::Svg { id, data } => {
                    self.renderer.load_svg(physical_size, id.into(), data)
                },
                // Text is ignored for the background.
                LayerContent::Text(_) => continue,
            };

            // Add texture to the upcoming render run.
            match texture {
                Ok(texture) => self.background_textures.push(texture),
                Err(err) => error!("Failed to load background texture: {err}"),
            }
        }

        clear_color
    }

    /// Prepare all modules for rendering.
    fn prepare_modules(&mut self) {
        let physical_size = self.size * self.scale;

        // Reset render order.
        self.modules.start.clear();
        self.modules.center.clear();
        self.modules.end.clear();

        let mut center_width = 0;
        let mut end_width = 0;

        // Ensure modules are ready for rendering.
        for module in self.modules.configured.values_mut() {
            module.prepare(&mut self.renderer, self.scale, physical_size.height);

            // Ignore modules which failed preparation.
            let render_module = match module.render_module.clone() {
                Some(render_module) => render_module,
                None => continue,
            };

            match module.module.alignment {
                Alignment::Start => self.modules.start.push(render_module),
                Alignment::Center => {
                    center_width += render_module.size.width;
                    self.modules.center.push(render_module);
                },
                Alignment::End => {
                    end_width += render_module.size.width;
                    self.modules.end.push(render_module);
                },
            }
        }

        // Order modules within their alignment.
        self.modules.start.sort_unstable_by_key(|module| module.index);
        self.modules.center.sort_unstable_by_key(|module| module.index);
        self.modules.end.sort_unstable_by_key(|module| module.index);

        // Calculate offset for module alignments.
        let center_offset = (physical_size.width as f32 - center_width as f32) / 2.;
        self.modules.center_offset = Point::new(center_offset, 0.);
        let end_offset = physical_size.width as f32 - end_width as f32;
        self.modules.end_offset = Point::new(end_offset, 0.);
    }

    /// Draw a set of renderable modules.
    fn draw_modules(
        render_pass: &mut ActiveRenderPass<'_>,
        modules: &[Arc<RenderModule>],
        mut offset: Point<f32>,
    ) {
        for module in modules {
            for layer in &module.layers {
                let point = layer.point + offset;
                match &layer.content {
                    RenderLayerContent::Texture(texture) => {
                        if let Err(err) =
                            render_pass.draw_texture(texture.clone(), point, layer.size.into())
                        {
                            error!("Failed to draw texture: {err}");
                        }
                    },
                    RenderLayerContent::Color(color) => {
                        if let Err(err) = render_pass.draw_color(*color, point, layer.size.into()) {
                            error!("Failed to draw color: {err}");
                        }
                    },
                    _ => (),
                }
            }

            // Update position for the next module;
            offset.x += module.size.width as f32;
        }
    }

    /// Unstall the renderer.
    ///
    /// This will render a new frame if there currently is no frame request
    /// pending.
    pub fn unstall(&mut self) {
        if !mem::take(&mut self.stalled) {
            return;
        }

        self.draw();
        let _ = self.connection.flush();
    }

    /// Update the window's logical size.
    pub fn set_size(&mut self, compositor: &CompositorState, size: Size) {
        if self.size == size {
            return;
        }

        self.initial_configure_done = true;
        self.size = size;
        self.dirty = true;

        // Force module redraw.
        self.clear_module_cache();

        // Update the window's opaque region.
        //
        // This is done here since it can only change on resize, but the commit happens
        // atomically on redraw.
        if let Ok(region) = Region::new(compositor) {
            region.add(0, 0, size.width as i32, size.height as i32);
            self.window.wl_surface().set_opaque_region(Some(region.wl_region()));
        }

        self.unstall();
    }

    /// Update the window's DPI factor.
    pub fn set_scale_factor(&mut self, scale: f64) {
        if self.scale == scale {
            return;
        }

        self.scale = scale;
        self.dirty = true;

        // Force module redraw.
        self.clear_module_cache();

        self.unstall();
    }

    /// Update the window configuration.
    pub fn update_config(&mut self, config: Config) {
        self.dirty |= self.config != config;

        if self.config.edge != config.edge {
            self.window.set_anchor(config.edge.into());
        }

        if self.config.size != config.size {
            let size = config.size.unwrap_or(DEFAULT_SIZE);
            self.window.set_size(0, size);
            self.window.set_exclusive_zone(size as i32);
        }

        if self.config.layer != config.layer {
            self.window.set_layer(config.layer.into());
        }

        self.config = config;

        self.unstall();
    }

    /// Update the configuration of a module.
    pub fn update_module(&mut self, module: Module) {
        // Remove modules with no layers.
        if module.layers.is_empty() {
            self.modules.configured.remove(&module.id);
            return;
        }

        self.modules.configured.insert(module.id.clone(), module.into());

        self.dirty = true;
        self.unstall();
    }

    /// Reset render cache for all modules.
    fn clear_module_cache(&mut self) {
        for module in self.modules.configured.values_mut() {
            module.render_module = None;
        }
    }
}

/// Bar module components.
struct Modules {
    configured: HashMap<Arc<String>, BarModule>,

    start: Vec<Arc<RenderModule>>,

    center: Vec<Arc<RenderModule>>,
    center_offset: Point<f32>,

    end: Vec<Arc<RenderModule>>,
    end_offset: Point<f32>,
}

impl From<HashMap<Arc<String>, Module>> for Modules {
    fn from(modules: HashMap<Arc<String>, Module>) -> Self {
        Self {
            configured: modules.into_iter().map(|(id, module)| (id, module.into())).collect(),
            center_offset: Default::default(),
            end_offset: Default::default(),
            center: Default::default(),
            start: Default::default(),
            end: Default::default(),
        }
    }
}

/// Container for tracking module render state.
struct BarModule {
    render_module: Option<Arc<RenderModule>>,
    module: Module,
}

impl BarModule {
    /// Prepare module and its layers for rendering.
    fn prepare(&mut self, renderer: &mut Renderer, scale: f64, height: u32) {
        // Ignore modules with cached render state.
        if self.render_module.is_some() {
            return;
        }

        // Create layer container.
        let mut render_module = RenderModule::new(&self.module, height);
        let module_id = &self.module.id;

        // Collect layers and determine background color dimensions.
        render_module.layers.reserve(self.module.layers.len());
        for (i, layer) in self.module.layers.iter().enumerate().rev() {
            let mut layer = match RenderLayer::new(renderer, scale, layer) {
                Ok(layer) => layer,
                Err(err) => {
                    error!("Failed layout for module {:?}, layer index {i:?}: {err}", module_id);
                    continue;
                },
            };

            // Update size for background color layers.
            layer.update_background_size(render_module.size);

            // Update total module size.
            let layer_width = layer.size.width + layer.margin.left + layer.margin.right;
            let layer_height = layer.size.height + layer.margin.top + layer.margin.bottom;
            render_module.size.width = render_module.size.width.max(layer_width);
            render_module.size.height = render_module.size.height.max(layer_height);

            render_module.layers.push(layer);
        }
        render_module.layers = render_module.layers.into_iter().rev().collect();

        // Layout and render layer content.
        let mut parent_offset = Point::default();
        let mut parent_size = Size::new(0, height);
        for (i, layer) in render_module.layers.iter_mut().enumerate() {
            if let Err(err) = layer.layout(renderer, parent_offset, parent_size) {
                error!("Failed layout for module {:?}, layer index {i:?}: {err}", module_id);
                continue;
            }

            // Update parent location and size for the next layer.
            parent_offset = layer.point;
            parent_size = layer.size;
        }

        self.render_module = Some(Arc::new(render_module));
    }
}

impl From<Module> for BarModule {
    fn from(module: Module) -> Self {
        Self { module, render_module: Default::default() }
    }
}

/// Renderable module data.
struct RenderModule {
    layers: SmallVec<[RenderLayer; 10]>,
    size: Size,
    index: u8,
}

impl RenderModule {
    fn new(module: &Module, height: u32) -> Self {
        RenderModule { layers: SmallVec::new(), size: Size::new(0, height), index: module.index }
    }
}

/// Renderable module layer.
struct RenderLayer {
    content: RenderLayerContent,
    alignment: Alignment,
    point: Point<f32>,
    font: LayerFont,
    margin: Margin,
    size: Size,
}

impl RenderLayer {
    fn new(renderer: &mut Renderer, scale: f64, layer: &ModuleLayer) -> Result<Self, Error> {
        // Convert module coordinates from logical to physical space.

        let mut size = Size::<u32>::from(layer.size) * scale;

        let mut font = layer.font.clone();
        font.size = font.size.map(|size| size * scale);

        let mut margin = layer.margin;
        margin.top = (margin.top as f64 * scale).round() as u32;
        margin.right = (margin.right as f64 * scale).round() as u32;
        margin.bottom = (margin.bottom as f64 * scale).round() as u32;
        margin.left = (margin.left as f64 * scale).round() as u32;

        // Get module content and determine initial module size.
        let content = match &layer.content {
            // Update size for text components.
            LayerContent::Text(text) => {
                // Update variable dimensions.
                if let Some(text_size) = renderer.text_size(&font, text) {
                    if size.width == 0 {
                        size.width = text_size.width;
                    }
                    if size.height == 0 {
                        size.height = text_size.height;
                    }
                }

                RenderLayerContent::Text(text.clone())
            },
            // Handle image and SVG layers.
            LayerContent::Path(_) | LayerContent::Image { .. } | LayerContent::Svg { .. } => {
                // Normalize paths by loading the data.
                let (id, data, is_image) = match &layer.content {
                    LayerContent::Path(path) => {
                        let is_image = path.as_ref().extension().is_some_and(|ext| ext != "svg");
                        let data = fs::read(&**path)?;
                        (path.into(), Cow::Owned(data), is_image)
                    },
                    &LayerContent::Image { id, data } => (id.into(), data.into(), true),
                    &LayerContent::Svg { id, data } => (id.into(), data.into(), false),
                    _ => unreachable!(),
                };

                // Update rendered size for images.
                if is_image {
                    // Get Vulkan image texture.
                    let texture = renderer.load_image(id, &data)?;

                    // Update variable dimensions.
                    let texture_size = texture.image().extent();
                    if size.width == 0 {
                        size.width = texture_size[0];
                    }
                    if size.height == 0 {
                        size.height = texture_size[1];
                    }

                    RenderLayerContent::Texture(texture)
                } else {
                    RenderLayerContent::Svg(id, data)
                }
            },
            LayerContent::Color(color) => RenderLayerContent::Color(*color),
        };

        Ok(Self {
            content,
            margin,
            font,
            size,
            alignment: layer.alignment,
            point: Point::default(),
        })
    }

    /// Update background color size based on its children.
    fn update_background_size(&mut self, child_size: Size) {
        // Ignore non-color content.
        if !matches!(self.content, RenderLayerContent::Color(_)) {
            return;
        }

        self.size.width = self.size.width.max(child_size.width);
        self.size.height = self.size.height.max(child_size.height);
    }

    /// Layout this module's content.
    fn layout(
        &mut self,
        renderer: &mut Renderer,
        parent_point: Point<f32>,
        parent_size: Size,
    ) -> Result<(), Error> {
        // Finalize size calculation for SVGs and text.
        match &self.content {
            RenderLayerContent::Svg(..) => {
                if self.size.width == 0 {
                    self.size.width = parent_size.width;
                }
                if self.size.height == 0 {
                    self.size.height = parent_size.height;
                }
            },
            RenderLayerContent::Text(_) if self.size.height == 0 => {
                self.size.height = parent_size.height;
            },
            _ => (),
        }

        // Ignore empty layers.
        if self.size.width == 0 || self.size.height == 0 {
            return Err(Error::EmptyLayer);
        }

        // Prepare layer content for rendering.
        match &self.content {
            RenderLayerContent::Color(color) => {
                self.content = RenderLayerContent::Color(*color);
            },
            RenderLayerContent::Svg(id, path) => {
                let texture = renderer
                    .load_svg(self.size, id.clone(), path)
                    .inspect_err(|err| error!("Failed to load svg {path:?}: {err}"))?;
                self.content = RenderLayerContent::Texture(texture);
            },
            RenderLayerContent::Text(text) => {
                let texture = renderer
                    .load_text(self.font.clone(), self.size, text.clone())
                    .inspect_err(|err| error!("Failed to render text: {err}"))?;
                self.content = RenderLayerContent::Texture(texture);
            },
            RenderLayerContent::Texture(_) => (),
        }

        // Align layer within its parent.
        let parent_width = parent_size.width.saturating_sub(self.margin.left + self.margin.right);
        if parent_size.width != 0 {
            let x_delta = parent_width as f32 - self.size.width as f32;
            match self.alignment {
                Alignment::Start => (),
                Alignment::Center => self.point.x += x_delta / 2.,
                Alignment::End => self.point.x += x_delta,
            }
        }
        let parent_height = parent_size.height.saturating_sub(self.margin.top + self.margin.bottom);
        let y_delta = parent_height as f32 - self.size.height as f32;
        self.point.y += y_delta / 2.;

        // Apply margin offset.
        self.point.x += self.margin.left as f32;

        // Position layer relative to the module origin.
        self.point += parent_point;

        Ok(())
    }
}

/// Renderable module layer content.
enum RenderLayerContent {
    /// SVG content pending layout.
    Svg(ImageResourceId, Cow<'static, [u8]>),
    /// Text content pending layout.
    Text(Arc<String>),

    /// Vulkan GPU image.
    Texture(Texture),
    /// Single-color background.
    Color(Color),
}

impl From<Texture> for RenderLayerContent {
    fn from(texture: Texture) -> Self {
        Self::Texture(texture)
    }
}

impl From<Color> for RenderLayerContent {
    fn from(color: Color) -> Self {
        Self::Color(color)
    }
}
