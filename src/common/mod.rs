//! Backend-neutral geometry, color, coverage, and rendering support.

pub mod color;
pub mod dash;
pub mod geometry;
pub mod raster;
pub mod stroke;

pub(crate) mod render;
pub use render::{GradientError, Pixmap, PixmapError, RenderError, SolidPaint, SpreadMode};
