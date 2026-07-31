
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
pub mod canvas;
pub mod canvas_linear;
pub mod context;
pub mod edge;

#[cfg(feature = "fixed")] pub mod fixed;
#[cfg(feature = "fixed")] pub use fixed::{ math,
    flatten as flatten_fixed, raster as raster_fixed,
    stroke as stroke_fixed, tile as tile_fixed,
};
