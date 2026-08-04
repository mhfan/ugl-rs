//! Fixed-point geometry preparation and rasterization.

use core::ops::{Add, Sub};
use crate::common::geometry::{Affine, Point, ScalarConstants};

/// Compact Q24.8 coordinate scalar used by the fixed backend.
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
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)] pub struct Scalar(i32);

impl Scalar {
    pub const FRAC_BITS: u32 = 8;
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(1 << Self::FRAC_BITS);
    pub const MIN: Self = Self(i32::MIN);
    pub const MAX: Self = Self(i32::MAX);

    pub const fn from_bits(bits: i32) -> Self { Self(bits) }
    pub const fn to_bits(self) -> i32 { self.0 }

    /// Converts an `i32`, `u32`, `usize`, `f32`, or `f64` to Q24.8.
    ///
    /// Integer inputs must fit exactly. Floating-point inputs must be finite and
    /// are rounded to nearest, with halfway cases rounded to even.
    ///
    /// # Panics
    ///
    /// Panics when the input is non-finite or outside the Q24.8 range.
    #[track_caller]
    pub fn from_num(value: impl ScalarSource) -> Self { value.into_scalar() }

    /// Converts this value through a standard [`From`] implementation.
    pub fn to_num<T: From<Self>>(self) -> T { self.into() }
}

#[doc(hidden)] pub trait ScalarSource { fn into_scalar(self) -> Scalar; }

macro_rules! scalar_from_integer { ($($type:ty),+ $(,)?) => { $(
    impl ScalarSource for $type {
        #[track_caller] fn into_scalar(self) -> Scalar {
            let value = i128::try_from(self).expect("integer does not fit Scalar");
            let bits = value.checked_mul(Scalar::ONE.0 as _)
                .and_then(|value| i32::try_from(value).ok())
                .expect("integer does not fit Scalar");
            Scalar(bits)
        }
    }
)+ }; }

scalar_from_integer!(i32, u32, usize);

fn scalar_from_f64(value: f64) -> Scalar {
    assert!(value.is_finite(), "non-finite value cannot be converted to Scalar");
    let scaled = value * Scalar::ONE.0 as f64;
    assert!(scaled >= i32::MIN as f64 - 1.0 && scaled <= i32::MAX as f64 + 1.0,
        "floating-point value does not fit Scalar");
    let truncated = scaled as i64;
    let fraction = scaled - truncated as f64;
    let magnitude = if fraction < 0.0 { -fraction } else { fraction };
    let adjust = magnitude > 0.5 || magnitude == 0.5 && truncated & 1 != 0;
    let rounded = truncated + if adjust { if fraction < 0.0 { -1 } else { 1 } } else { 0 };
    Scalar(i32::try_from(rounded).expect("floating-point value does not fit Scalar"))
}

impl ScalarSource for f32 {
    fn into_scalar(self) -> Scalar { scalar_from_f64(self as _) }
}
impl ScalarSource for f64 {
    fn into_scalar(self) -> Scalar { scalar_from_f64(self) }
}
impl From<Scalar> for f32 {
    fn from(value: Scalar) -> Self { value.0 as Self / Scalar::ONE.0 as Self }
}
impl From<Scalar> for f64 {
    fn from(value: Scalar) -> Self { value.0 as Self / Scalar::ONE.0 as Self }
}

impl Add for Scalar { type Output = Self;
    fn add(self, rhs: Self) -> Self { Self(self.0 + rhs.0) }
}
impl Sub for Scalar { type Output = Self;
    fn sub(self, rhs: Self) -> Self { Self(self.0 - rhs.0) }
}

pub(crate) const COORD_FRAC_BITS: u32 = Scalar::FRAC_BITS;
pub(crate) const COORD_SCALE: i32 = Scalar::ONE.to_bits();
pub(crate) const HALF_PIXEL_RAW: i32 = COORD_SCALE / 2;

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
            const SCALE: i64 = COORD_SCALE as _;
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
    #[test] fn scalar_has_compact_layout_and_explicit_q24_8_conversions() {
        assert_eq!(core::mem::size_of::<Scalar>(), core::mem::size_of::<i32>());
        assert_eq!(core::mem::align_of::<Scalar>(), core::mem::align_of::<i32>());
        assert_eq!(Scalar::from_num(7_i32).to_bits(), 7 * 256);
        assert_eq!(Scalar::from_num(7_u32).to_bits(), 7 * 256);
        assert_eq!(Scalar::from_num(7_usize).to_bits(), 7 * 256);
        assert_eq!(Scalar::from_num(-8_388_608_i32), Scalar::MIN);
        assert_eq!(Scalar::from_num( 8_388_607_i32).to_bits(), i32::MAX - 255);
        assert_eq!(Scalar::from_num( 1.0 / 512.0).to_bits(),  0);
        assert_eq!(Scalar::from_num( 3.0 / 512.0).to_bits(),  2);
        assert_eq!(Scalar::from_num(-1.0 / 512.0).to_bits(),  0);
        assert_eq!(Scalar::from_num(-3.0 / 512.0).to_bits(), -2);
        assert_eq!(Scalar::from_bits(320).to_num::<f32>(), 1.25);
    }

    #[test] #[should_panic(expected = "non-finite value")]
    fn scalar_rejects_non_finite_input() { let _ = Scalar::from_num(f32::NAN); }

    #[test] #[should_panic(expected = "integer does not fit Scalar")]
    fn scalar_rejects_out_of_range_integer() { let _ = Scalar::from_num(8_388_608_i32); }

    #[test] fn decomposed_affine_matches_full_width_rounding() {
        let reference = |a: Scalar, x: Scalar, c: Scalar, y: Scalar, e: Scalar| {
            let value = a.to_bits() as i128 * x.to_bits() as i128 +
                        c.to_bits() as i128 * y.to_bits() as i128 +
                        e.to_bits() as i128 * COORD_SCALE as i128;
            let rounded = if value < 0 {
                (value - HALF_PIXEL_RAW as i128) / COORD_SCALE as i128
            } else {
                (value + HALF_PIXEL_RAW as i128) / COORD_SCALE as i128
            };
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
