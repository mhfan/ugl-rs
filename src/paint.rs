//! Paint value types shared by numeric backends.

use crate::color::{PremulSRGBA8, SRGBA};

#[derive(Clone, Copy, Debug, PartialEq)] pub struct SolidPaint {
    encoded: PremulSRGBA8,
    #[cfg(feature = "f32")]
    linear: crate::color::LinearPremulRGBA<f32>,
}

impl SolidPaint {
    pub fn new(color: SRGBA<u8>) -> Self { Self {
        encoded: color.premul_encoded(),
        #[cfg(feature = "f32")]
        linear: color.to_linear().premul(),
    } }
    pub fn premultiplied(color: PremulSRGBA8) -> Self { Self {
        encoded: color,
        #[cfg(feature = "f32")]
        linear: color.to_linear(),
    } }
    pub fn color(&self) -> PremulSRGBA8 { self.encoded }
    #[cfg(feature = "f32")]
    pub fn linear_color(&self) -> crate::color::LinearPremulRGBA<f32> {
        self.linear
    }
}

impl From<SRGBA<u8>> for SolidPaint { fn from(color: SRGBA<u8>) -> Self { Self::new(color) } }

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum GradientError {
    EmptyStops, NonFiniteOffset, OffsetOutOfRange, UnorderedStops,
    RampTooSmall, RampTooLarge, NonFiniteGeometry, CoordinateOutOfRange,
    NegativeRadius, DegenerateGeometry,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SpreadMode { #[default] Pad, Repeat, Reflect }
