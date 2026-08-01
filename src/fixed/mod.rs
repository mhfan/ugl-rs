//! Fixed-point geometry preparation and rasterization.

use crate::common::geometry::{Affine, Point, ScalarConstants};

/// Q24.8 coordinate scalar used by the fixed backend.
pub type Scalar = fixed::types::I24F8;

/// Raw Q24.8 coordinate magnitude supported by the bounded render path.
pub const DEVICE_RAW_LIMIT: i32 = 1 << 29;

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum TransformError { Overflow }

impl ScalarConstants for Scalar {
    const ZERO: Self = Self::ZERO;
    const  ONE: Self = Self::ONE;
}

impl Affine<Scalar> {
    /// Transforms a Q24.8 point with widened multiply-add and checked conversion.
    pub fn try_transform_point(&self, point: Point<Scalar>) ->
        Result<Point<Scalar>, TransformError> {
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

#[cfg(test)] mod tests { use super::*;
    use crate::common::geometry::PathBuilder;

    #[test] fn geometry_reuses_generic_point_path_and_affine_types() {
        let (one, half) = (Scalar::from_num(1), Scalar::from_num(0.5));
        let transform = Affine::<Scalar>::translate(half, one);
        assert_eq!(transform.try_transform_point((one, half).into()).unwrap(),
            (Scalar::from_num(1.5), Scalar::from_num(1.5)).into());

        let mut builder = PathBuilder::<Scalar>::new();
        builder.move_to((Scalar::ZERO, Scalar::ZERO)).line_to((one, half));
        assert_eq!(builder.build().len(), 2);
    }

    #[test] fn affine_widens_rounds_symmetrically_and_checks_output() {
        let raw = Scalar::from_bits;
        let half_scale = Affine::new(raw(128), raw(0), raw(0), raw(128), raw(0), raw(0));
        assert_eq!(half_scale.try_transform_point((raw(1), raw(-1)).into()).unwrap(),
            (raw(1), raw(-1)).into());

        let maximum = Scalar::MAX;
        let overflow = Affine::new(maximum, Scalar::ZERO, Scalar::ZERO,
            maximum, maximum, maximum);
        assert_eq!(overflow.try_transform_point((maximum, maximum).into()),
            Err(TransformError::Overflow));
    }
}
