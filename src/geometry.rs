//! Shared geometry types.

use std::ops::Mul;

/// 2D object size.
#[derive(PartialEq, Eq, Copy, Clone, Default, Debug)]
pub struct Size<T = u32> {
    pub width: T,
    pub height: T,
}

impl<T> From<(T, T)> for Size<T> {
    fn from((width, height): (T, T)) -> Self {
        Self { width, height }
    }
}

impl<T> From<Size<T>> for [T; 2] {
    fn from(size: Size<T>) -> Self {
        [size.width, size.height]
    }
}

impl From<Size<u32>> for [f32; 2] {
    fn from(size: Size<u32>) -> Self {
        [size.width as f32, size.height as f32]
    }
}

impl Mul<f64> for Size {
    type Output = Self;

    fn mul(mut self, scale: f64) -> Self {
        self.width = (self.width as f64 * scale).round() as u32;
        self.height = (self.height as f64 * scale).round() as u32;
        self
    }
}
