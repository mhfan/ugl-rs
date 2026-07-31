//! Paint sampling contracts for the fixed-point rendering backend.

use crate::{color::PremulSRGBA8, sampler::SolidPaint};

pub use super::math::FixedAngle;

/// Produces encoded premultiplied sRGB at integer device-pixel coordinates.
///
/// Implementations sample the center of pixel `(x, y)` without requiring
/// floating-point arithmetic. This is separate from `PaintSampler` so a fixed
/// raster pipeline never silently calls an `f32` sampler.
pub trait FixedPaintSampler {
    fn sample_fixed(&self, x: u32, y: u32) -> PremulSRGBA8;
    fn solid_color_fixed(&self) -> Option<PremulSRGBA8> { None }
}

impl<S: FixedPaintSampler + ?Sized> FixedPaintSampler for &S {
    fn sample_fixed(&self, x: u32, y: u32) -> PremulSRGBA8 {
        (**self).sample_fixed(x, y)
    }
    fn solid_color_fixed(&self) -> Option<PremulSRGBA8> {
        (**self).solid_color_fixed()
    }
}

impl FixedPaintSampler for SolidPaint {
    fn sample_fixed(&self, _x: u32, _y: u32) -> PremulSRGBA8 { self.color() }
    fn solid_color_fixed(&self) -> Option<PremulSRGBA8> { Some(self.color()) }
}
