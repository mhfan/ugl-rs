
#![no_std]

extern crate alloc;

pub mod color;      // rgba/rgb, intensity & quantization
pub mod blend;      // color blending & alpha compositing, gamma correction

pub mod sampler;    // can be thought of 2D shaders
pub mod shader;     // reserved for a future optional 3D layer

pub mod geometry;   // shape, curve, free path

pub mod raster;
pub mod stroke;
pub mod dash;
pub mod flatten;
pub mod analytic;
#[cfg(feature = "fixed")] pub mod fixed;
#[cfg(feature = "fixed")] pub mod flatten_fixed {
    pub use crate::fixed::flatten::*;
}
#[cfg(feature = "fixed")] pub mod raster_fixed {
    pub use crate::fixed::raster::*;
}
#[cfg(feature = "fixed")] pub mod stroke_fixed {
    pub use crate::fixed::stroke::*;
}
#[cfg(feature = "fixed")] pub mod tile_fixed {
    pub use crate::fixed::tile::*;
}
pub mod canvas;
pub mod canvas_linear;
pub mod context;
pub mod edge;

/// Compatibility exports for fixed-point numeric helpers.
#[cfg(feature = "fixed")] pub mod math {
    pub use crate::fixed::math::{Fixed, FixedAngle};
}
