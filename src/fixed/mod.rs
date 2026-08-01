//! Fixed-point geometry preparation and rasterization.

/// Q24.8 coordinate scalar used by the fixed backend.
pub type Scalar = fixed::types::I24F8;

/// Raw Q24.8 coordinate magnitude supported by the bounded render path.
pub const DEVICE_RAW_LIMIT: i32 = 1 << 29;

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum TransformError { Overflow }

pub mod flatten;
pub mod math;
pub mod raster;
pub mod sampler;
pub mod stroke;
pub mod tile;
pub mod context;
pub mod canvas;
pub mod dash;

pub use context::{Canvas, CanvasRef};
