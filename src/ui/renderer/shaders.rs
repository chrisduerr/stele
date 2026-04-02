//! Vulkan GLSL shaders.

pub mod texture {
    pub mod vertex {
        vulkano_shaders::shader! {
            ty: "vertex",
            path: "./shaders/texture_vertex.glsl",
        }
    }

    pub mod fragment {
        vulkano_shaders::shader! {
            ty: "fragment",
            path: "./shaders/texture_fragment.glsl",
        }
    }
}

pub mod color {
    pub mod vertex {
        vulkano_shaders::shader! {
            ty: "vertex",
            path: "./shaders/color_vertex.glsl",
        }
    }

    pub mod fragment {
        vulkano_shaders::shader! {
            ty: "fragment",
            path: "./shaders/color_fragment.glsl",
        }
    }
}
