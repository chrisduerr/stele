//! Text rendering.

use std::sync::Arc;

use pangocairo::cairo::{Context, Format, ImageSurface, ImageSurfaceDataOwned};
use pangocairo::pango::{EllipsizeMode, FontDescription, Layout, SCALE as PANGO_SCALE};

use crate::geometry::Size;
use crate::ui::renderer::LruMap;

/// Maximum number of cached fonts.
const MAX_LAYOUTS: usize = 10;

/// Font rasterizer.
pub struct Rasterizer {
    layouts: LruMap<Font, Layout>,
}

impl Default for Rasterizer {
    fn default() -> Self {
        Self { layouts: LruMap::new(MAX_LAYOUTS) }
    }
}

impl Rasterizer {
    /// Rasterize a string within a rectangle.
    ///
    /// Only a single line will be rendered. Any content beyond the width of the
    /// rectangle is ellipsized.
    pub fn rasterize(
        &mut self,
        font: &Font,
        color: [f64; 3],
        size: Size<i32>,
        text: &str,
    ) -> (ImageSurfaceDataOwned, Size) {
        // Create target cairo surface.
        let image_surface = ImageSurface::create(Format::ARgb32, size.width, size.height).unwrap();
        let context = Context::new(&image_surface).unwrap();

        // Get the font configuration's layout.
        let layout = self.layout(font);

        // Set maximum text length.
        layout.set_width(size.width * PANGO_SCALE);

        // Calculate offset for vertical text centering.
        let text_height = layout.pixel_size().1;
        let y_offset = (size.height as f64 - text_height as f64) / 2.;

        // Render text.
        layout.set_text(text);
        context.move_to(0., y_offset);
        context.set_source_rgb(color[0], color[1], color[2]);
        pangocairo::functions::show_layout(&context, layout);

        drop(context);

        let size = Size::new(image_surface.width() as u32, image_surface.height() as u32);
        let data = image_surface.take_data().unwrap();

        (data, size)
    }

    /// Get the layout for a font configuration.
    pub fn layout(&mut self, font: &Font) -> &mut Layout {
        // Create layout if it does not exist yet.
        if !self.layouts.contains_key(font) {
            let family = font.family.as_ref().map_or("sans", |family| family);
            let layout = Self::create_layout(family, font.size());

            self.layouts.insert(font.clone(), layout);
        }

        self.layouts.get(font).unwrap()
    }

    /// Create a new pango layout.
    fn create_layout(family: &str, size: f64) -> Layout {
        // Create pango layout.
        let image_surface = ImageSurface::create(Format::ARgb32, 0, 0).unwrap();
        let context = Context::new(&image_surface).unwrap();
        let layout = pangocairo::functions::create_layout(&context);

        // Set font description.
        let font_desc = format!("{family} {size}px");
        let font = FontDescription::from_string(&font_desc);
        layout.set_font_description(Some(&font));

        // Configure layout for single-line, ellipsized rendering.
        layout.set_ellipsize(EllipsizeMode::End);
        layout.set_height(0);

        layout
    }
}

/// Font family and size.
#[derive(Hash, PartialEq, Eq, Clone)]
pub struct Font {
    family: Option<Arc<String>>,
    size: u32,
}

impl Font {
    pub fn new(family: Option<Arc<String>>, size: f64) -> Self {
        let size = (size * 1_000.).round() as u32;
        Self { family, size }
    }

    fn size(&self) -> f64 {
        self.size as f64 / 1_000.
    }
}
