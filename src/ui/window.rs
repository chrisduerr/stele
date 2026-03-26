//! Wayland window management.

use std::mem;

use smithay_client_toolkit::compositor::{CompositorState, Region};
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use stele_ipc::Config;

use crate::geometry::Size;
use crate::ui::renderer::Renderer;
use crate::wayland::ProtocolStates;
use crate::{Error, State};

/// Default bar size.
const DEFAULT_SIZE: u32 = 40;

/// Wayland window.
pub struct Window {
    queue: QueueHandle<State>,
    connection: Connection,
    window: LayerSurface,
    viewport: WpViewport,

    renderer: Renderer,

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
    ) -> Result<Self, Error> {
        // Create surface's Wayland global handles.
        let surface = protocol_states.compositor.create_surface(&queue);
        if let Some(fractional_scale) = &protocol_states.fractional_scale {
            fractional_scale.fractional_scaling(&queue, &surface);
        }
        let viewport = protocol_states.viewporter.viewport(&queue, &surface);

        // Create the layer shell window.
        let size = config.size.unwrap_or(DEFAULT_SIZE);
        let window = protocol_states.layer.create_layer_surface(
            &queue,
            surface.clone(),
            config.layer.into(),
            Some("panel"),
            None,
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
            stalled: true,
            dirty: true,
            scale: 1.,
            initial_configure_done: Default::default(),
        })
    }

    /// Redraw the window.
    pub fn draw(&mut self) {
        // Stall rendering if nothing changed since last redraw.
        if !mem::take(&mut self.dirty) || !self.initial_configure_done {
            self.stalled = true;
            return;
        }

        // Update viewporter logical render size.
        //
        // NOTE: This must be done every time we draw with Sway; it is not
        // persisted when drawing with the same surface multiple times.
        self.viewport.set_destination(self.size.width as i32, self.size.height as i32);

        // Mark entire window as damaged.
        let wl_surface = self.window.wl_surface();
        wl_surface.damage(0, 0, self.size.width as i32, self.size.height as i32);

        // Render the window content.
        let clear_color = self.config.background.unwrap_or_default();
        let physical_size = self.size * self.scale;
        self.renderer.draw(physical_size, clear_color, |_| {});

        // Request a new frame.
        wl_surface.frame(&self.queue, wl_surface.clone());

        // Apply surface changes.
        wl_surface.commit();
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
}
