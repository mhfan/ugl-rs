
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "f32")] pub mod float;
pub mod common;

pub use common::{color, dash, edge, geometry, raster, stroke};
pub(crate) use common::{paint, render};

#[cfg(feature = "f32")] pub mod blend; // color blending & alpha compositing

#[cfg(feature = "f32")] pub use float::sampler;
#[cfg(not(feature = "f32"))] pub mod sampler {
    pub use crate::paint::{GradientError, SolidPaint, SpreadMode};
}
pub mod shader;     // reserved for a future optional 3D layer

pub mod flatten;
#[cfg(feature = "f32")] pub use float::{analytic, canvas, canvas_linear, context};

pub use render::{Pixmap, PixmapError, RenderError};
#[cfg(feature = "f32")] pub use float::Canvas;

#[cfg(feature = "fixed")] pub mod fixed;
