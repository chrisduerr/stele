//! Shared geometry types.

use std::ops::{Add, AddAssign, Mul};

/// 2D object position.
#[derive(PartialEq, Eq, Copy, Clone, Default, Debug)]
pub struct Point<T = i32> {
    pub x: T,
    pub y: T,
}

impl<T> Point<T> {
    pub fn new(x: T, y: T) -> Self {
        Self { x, y }
    }
}

impl<T> From<(T, T)> for Point<T> {
    fn from((x, y): (T, T)) -> Self {
        Self { x, y }
    }
}

impl From<Point<u32>> for Point<f32> {
    fn from(point: Point<u32>) -> Self {
        Self { x: point.x as f32, y: point.y as f32 }
    }
}

impl<T: Add<Output = T>> Add<Point<T>> for Point<T> {
    type Output = Self;

    fn add(mut self, other: Point<T>) -> Self {
        self.x = self.x + other.x;
        self.y = self.y + other.y;
        self
    }
}

impl<T: AddAssign> AddAssign for Point<T> {
    fn add_assign(&mut self, other: Point<T>) {
        self.x += other.x;
        self.y += other.y;
    }
}

impl Mul<f64> for Point<f64> {
    type Output = Self;

    fn mul(mut self, scale: f64) -> Self {
        self.x *= scale;
        self.y *= scale;
        self
    }
}

/// 2D object size.
#[derive(Hash, PartialEq, Eq, Copy, Clone, Default, Debug)]
pub struct Size<T = u32> {
    pub width: T,
    pub height: T,
}

impl<T> Size<T> {
    pub fn new(width: T, height: T) -> Self {
        Self { width, height }
    }
}

impl<T> From<(T, T)> for Size<T> {
    fn from((width, height): (T, T)) -> Self {
        Self { width, height }
    }
}

impl From<stele_ipc::Size> for Size {
    fn from(size: stele_ipc::Size) -> Self {
        Self { width: size.width, height: size.height }
    }
}

impl From<Size> for Size<i32> {
    fn from(size: Size) -> Size<i32> {
        Self { width: size.width as i32, height: size.height as i32 }
    }
}

impl From<Size> for Size<f32> {
    fn from(size: Size) -> Size<f32> {
        Self { width: size.width as f32, height: size.height as f32 }
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
