//! Floating-point vector rendering backend.

mod math;
mod edge;
pub mod dash;
pub(crate) use math::*;

pub mod analytic;
pub mod canvas;
pub mod canvas_linear;
pub mod context;
pub mod sampler;

pub use context::Canvas;
