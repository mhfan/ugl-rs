//! Fixed-point geometry preparation and rasterization.

/// Q24.8 coordinate scalar used by the fixed backend.
pub type Scalar = fixed::types::I24F8;

/// Raw Q24.8 coordinate magnitude supported by the bounded render path.
pub const DEVICE_RAW_LIMIT: i32 = 1 << 29;

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum TransformError { Overflow }

impl crate::common::geometry::ScalarConstants for Scalar {
    const ZERO: Self = Self::ZERO;
    const  ONE: Self = Self::ONE;
}

impl crate::common::geometry::Affine<Scalar> {
    /// Transforms a Q24.8 point with widened multiply-add and checked conversion.
    pub fn try_transform_point(&self, point: crate::common::geometry::Point<Scalar>) ->
        Result<crate::common::geometry::Point<Scalar>, TransformError> {
        let transform = |first: Scalar, x: Scalar, second: Scalar, y: Scalar,
            translation: Scalar| {
            const FRACTION_BITS: u32 = 8;
            const SCALE: i128 = 1 << FRACTION_BITS;
            let value = first.to_bits() as i128 * x.to_bits() as i128
                + second.to_bits() as i128 * y.to_bits() as i128
                + ((translation.to_bits() as i128) << FRACTION_BITS);
            let rounded = if value < 0 {
                (value - SCALE / 2) / SCALE
            } else { (value + SCALE / 2) / SCALE };
            i32::try_from(rounded).map(Scalar::from_bits)
                .map_err(|_| TransformError::Overflow)
        };
        Ok((transform(self.a, point.x, self.c, point.y, self.e)?,
            transform(self.b, point.x, self.d, point.y, self.f)?).into())
    }
}

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
