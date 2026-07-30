
//! Allocation-free paint samplers.
//!
//! Sampling currently uses device-space `f32` pixel centers as the reference
//! implementation. The compositor is generic over this trait, so later fixed
//! coordinate samplers can be introduced without changing premultiplied color
//! storage or raster coverage.

use crate::color::{PremulRGBA, RGBA};

/// Produces premultiplied source colors at device-space positions.
///
/// Implementations should be small values borrowed by the compositor. Calls are
/// statically dispatched; no trait object or allocation is required.
pub trait PaintSampler {
    fn sample(&self, x: f32, y: f32) -> PremulRGBA<u8>;

    /// Reports a position-independent color to enable span and tile fast paths.
    fn solid_color(&self) -> Option<PremulRGBA<u8>> { None }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolidPaint { color: PremulRGBA<u8> }

impl SolidPaint {
    pub fn new(color: RGBA<u8>) -> Self { Self { color: color.premul() } }
    pub fn premultiplied(color: PremulRGBA<u8>) -> Self { Self { color } }
    pub fn color(&self) -> PremulRGBA<u8> { self.color }
}

impl From<RGBA<u8>> for SolidPaint {
    fn from(color: RGBA<u8>) -> Self { Self::new(color) }
}

impl From<PremulRGBA<u8>> for SolidPaint {
    fn from(color: PremulRGBA<u8>) -> Self { Self::premultiplied(color) }
}

impl PaintSampler for SolidPaint {
    fn sample(&self, _x: f32, _y: f32) -> PremulRGBA<u8> { self.color }
    fn solid_color(&self) -> Option<PremulRGBA<u8>> { Some(self.color) }
}

#[cfg(test)] mod tests { use super::*;

    #[test] fn solid_paint_is_position_independent_and_premultiplied() {
        let paint = SolidPaint::new(RGBA::new(200, 100, 50, 128));
        assert_eq!(paint.sample(0.5, 0.5), (100, 50, 25, 128).into());
        assert_eq!(paint.sample(-100.0, 200.0), paint.solid_color().unwrap());
    }
}
