//! Fixed-point geometry preparation and rasterization.

use crate::common::geometry::{Affine, Point, ScalarConstants};

/// Q24.8 coordinate scalar used by the fixed backend.
///
/// Shared geometry containers accept fixed coordinates without conversion:
///
/// ```
/// use ugl_rs::{common::geometry::{Affine, PathBuilder}, fixed::Scalar};
///
/// let (one, half) = (Scalar::from_num(1), Scalar::from_num(0.5));
/// let transform = Affine::<Scalar>::translate(half, one);
/// assert_eq!(transform.try_transform_point((one, half).into()).unwrap(),
///     (Scalar::from_num(1.5), Scalar::from_num(1.5)).into());
///
/// let mut path = PathBuilder::<Scalar>::new();
/// path.move_to((Scalar::ZERO, Scalar::ZERO)).line_to((one, half));
/// assert_eq!(path.build().len(), 2);
/// ```
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
    ///
    /// Intermediate products are widened, and half-subpixel results round
    /// symmetrically away from zero:
    ///
    /// ```
    /// use ugl_rs::{common::geometry::Affine, fixed::{Scalar, TransformError}};
    ///
    /// let raw = Scalar::from_bits;
    /// let half = Affine::new(raw(128), raw(0), raw(0), raw(128), raw(0), raw(0));
    /// assert_eq!(half.try_transform_point((raw(1), raw(-1)).into()).unwrap(),
    ///     (raw(1), raw(-1)).into());
    ///
    /// let maximum = Scalar::MAX;
    /// let overflow = Affine::new(maximum, Scalar::ZERO, Scalar::ZERO,
    ///     maximum, maximum, maximum);
    /// assert_eq!(overflow.try_transform_point((maximum, maximum).into()),
    ///     Err(TransformError::Overflow));
    /// ```
    pub fn try_transform_point(&self, point: Point<Scalar>) ->
        Result<Point<Scalar>, TransformError> {
        let transform = |first: Scalar, x: Scalar, second: Scalar, y: Scalar,
            translation: Scalar| {
            const SCALE: i64 = 1 << 8;
            let (first, second) = (first.to_bits() as i64 * x.to_bits() as i64,
                second.to_bits() as i64 * y.to_bits() as i64);
            let (mut quotient, remainder) = (translation.to_bits() as i64 +
                first.div_euclid(SCALE) + second.div_euclid(SCALE),
                first.rem_euclid(SCALE) + second.rem_euclid(SCALE));
            quotient += remainder.div_euclid(SCALE);
            let remainder = remainder.rem_euclid(SCALE);
            let rounded = quotient + i64::from(remainder > SCALE / 2 ||
                remainder == SCALE / 2 && quotient >= 0);
            i32::try_from(rounded).map(Scalar::from_bits)
                .map_err(|_| TransformError::Overflow)
        };
        Ok((transform(self.a, point.x, self.c, point.y, self.e)?,
            transform(self.b, point.x, self.d, point.y, self.f)?).into())
    }
}

#[cfg(test)] mod tests { use super::*;
    #[test] fn decomposed_affine_matches_full_width_rounding() {
        let reference = |a: Scalar, x: Scalar, c: Scalar, y: Scalar, e: Scalar| {
            let value = a.to_bits() as i128 * x.to_bits() as i128 +
                        c.to_bits() as i128 * y.to_bits() as i128 +
                        ((e.to_bits() as i128) << 8);
            let rounded = if value < 0 { (value - 128) / 256 }
                          else         { (value + 128) / 256 };
            i32::try_from(rounded).map(Scalar::from_bits)
                .map_err(|_| TransformError::Overflow)
        };
        let (mut state, zero) = (0x6d2b_79f5_u32, Scalar::ZERO);
        let mut next = || { state ^= state << 13; state ^= state >> 17; state ^= state << 5;
            Scalar::from_bits(state as _) };
        for _ in 0..20_000 {
            let (a, x, c, y, e) = (next(), next(), next(), next(), next());
            let transform = Affine::new(a, zero, c, zero, e, zero);
            assert_eq!(transform.try_transform_point((x, y).into()).map(|point| point.x),
                reference(a, x, c, y, e));
        }
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
