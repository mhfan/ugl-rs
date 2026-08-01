
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod common;
pub mod shader;     // reserved for a future optional 3D layer

#[cfg(feature = "f32")]   pub mod float;
#[cfg(feature = "fixed")] pub mod fixed;

pub(crate) use common::{color, dash, edge, geometry, raster, render, stroke};
#[cfg(feature = "f32")] pub(crate) use float::{analytic, canvas, sampler};
pub(crate) use render::PixmapError;

/// Canvas for the default enabled backend (`f32` takes precedence when both exist).
#[cfg(feature = "f32")] pub use float::Canvas;
#[cfg(all(feature = "fixed", not(feature = "f32")))] pub use fixed::Canvas;
