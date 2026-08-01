
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "f32")] mod float;
mod state;
mod paint;

pub mod color;      // rgba/rgb, intensity & quantization
#[cfg(feature = "f32")] pub mod blend; // color blending & alpha compositing

#[cfg(feature = "f32")] pub mod sampler;
#[cfg(not(feature = "f32"))] pub mod sampler {
    pub use crate::paint::{GradientError, SolidPaint, SpreadMode};
}
pub mod shader;     // reserved for a future optional 3D layer

pub mod geometry;   // shape, curve, free path

pub mod raster;
pub mod stroke;
pub mod dash;
pub mod flatten;
#[cfg(feature = "f32")] pub mod analytic;
#[cfg(feature = "f32")] pub mod canvas;
#[cfg(not(feature = "f32"))] #[path = "fixed/support.rs"] pub mod canvas;
#[cfg(feature = "f32")] pub mod canvas_linear;
#[cfg(feature = "f32")] pub mod context;
pub mod edge;

pub use canvas::{Pixmap, PixmapError};
#[cfg(feature = "f32")] pub use context::Canvas;

#[cfg(feature = "fixed")] pub mod fixed;
