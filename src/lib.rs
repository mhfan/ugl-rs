
#![no_std]

extern crate alloc;

pub mod color;      // rgba/rgb, intensity & quantization
pub mod blend;      // color blending & alpha compositing, gamma correction

pub mod sampler;    // can be thought of 2D shaders
pub mod shader;     // reserved for a future optional 3D layer

pub mod geometry;   // shape, curve, free path

pub mod raster;
pub mod stroke;
pub mod flatten;
pub mod analytic;
#[cfg(feature = "fixed")] pub mod raster_fixed;
#[cfg(feature = "fixed")] pub mod tile_fixed;
pub mod canvas;
pub mod canvas_linear;
pub mod edge;

pub mod math;
