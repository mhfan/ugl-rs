//! Numeric building blocks for the fixed-point rendering backend.

use core::cmp::Ordering;

//  number: Q-number/Fixed/float/double, performance/fast vs quality/accurate
//  linear algebra, affine transformation, trigonometry

//  https://github.com/dimforge/nalgebra/blob/main/nalgebra-glm/src/aliases.rs
//pub use nalgebra_glm::TMat;

//  https://en.wikipedia.org/wiki/Q_(number_format)
//  https://johnmcfarlane.github.io/cnl/
//  https://gitlab.com/tspiteri/fixed

/// Divides by a positive denominator and rounds to nearest, with ties away from zero.
pub(crate) fn round_div_i64(numerator: i64, denominator: i64) -> i64 {
    debug_assert!(denominator > 0);
    let (quotient, remainder) = (
        numerator.div_euclid(denominator), numerator.rem_euclid(denominator),
    );
    match remainder.cmp(&(denominator - remainder)) {
        Ordering::Equal if numerator >= 0 => quotient + 1,
        Ordering::Equal | Ordering::Less => quotient,
        Ordering::Greater => quotient + 1,
    }
}

/// Unsigned counterpart of [`round_div_i64`].
pub(crate) fn round_div_u32(numerator: u32, denominator: u32) -> u32 {
    debug_assert!(denominator > 0);
    let (quotient, remainder) = (numerator / denominator, numerator % denominator);
    quotient + u32::from(remainder >= denominator - remainder)
}

/// Wide unsigned counterpart of [`round_div_i64`].
pub(crate) fn round_div_u64(numerator: u64, denominator: u64) -> u64 {
    debug_assert!(denominator > 0);
    let (quotient, remainder) = (numerator / denominator, numerator % denominator);
    quotient + u64::from(remainder >= denominator - remainder)
}

/// Unsigned binary angle where the complete `u32` range represents one turn.
///
/// ```
/// use ugl_rs::fixed::math::Angle;
///
/// assert_eq!(Angle::from_turn_fraction(1, 4), Some(Angle::QUARTER_TURN));
/// assert_eq!(Angle::from_turn_fraction(1, 0), None);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] pub struct Angle(u32);

impl Angle {
    pub const ZERO: Self = Self(0);
    pub const QUARTER_TURN: Self = Self(1 << 30);
    pub const HALF_TURN: Self = Self(1 << 31);
    pub const THREE_QUARTER_TURN: Self = Self(3 << 30);

    pub const fn from_bits(bits: u32) -> Self { Self(bits) }
    pub const fn from_turn_fraction(numerator: u32, denominator: u32) -> Option<Self> {
        if denominator == 0 { return None; }
        let turns = ((numerator as u64) << 32) / denominator as u64;
        Some(Self(turns as u32))
    }
    pub const fn to_bits(self) -> u32 { self.0 }
}

const CORDIC_ATAN_TURNS: [i64; 16] = [
    0x2000_0000, 0x12e4_051d, 0x09fb_385b, 0x0511_11d4,
    0x028b_0d43, 0x0145_d7e1, 0x00a2_f61e, 0x0051_7c55,
    0x0028_be53, 0x0014_5f2f, 0x000a_2f98, 0x0005_17cc,
    0x0002_8be6, 0x0001_45f3, 0x0000_a2fa, 0x0000_517d,
];

pub(crate) fn cordic_turn(mut x: i64, mut y: i64) -> u32 {
    if y == 0 { return if x < 0 { Angle::HALF_TURN.0 } else { 0 }; }
    if x == 0 {
        return if y < 0 {
            Angle::THREE_QUARTER_TURN.0
        } else { Angle::QUARTER_TURN.0 };
    }
    let magnitude = x.unsigned_abs().max(y.unsigned_abs());
    let shift = 48_u32.saturating_sub(64 - magnitude.leading_zeros());
    x <<= shift;
    y <<= shift;
    let mut angle = 0_i64;
    if x < 0 {
        x = -x;
        y = -y;
        angle = Angle::HALF_TURN.0 as _;
    }
    for (shift, increment) in CORDIC_ATAN_TURNS.into_iter().enumerate() {
        if y == 0 { break; }
        let (old_x, old_y) = (x, y);
        if old_y > 0 {
            x = old_x + (old_y >> shift);
            y = old_y - (old_x >> shift);
            angle += increment;
        } else {
            x = old_x - (old_y >> shift);
            y = old_y + (old_x >> shift);
            angle -= increment;
        }
    }
    angle.rem_euclid(1_i64 << 32) as _
}

/// Returns `(cos, sin)` in signed Q1.30.
pub(crate) fn cordic_unit_vector(angle: Angle) -> (i32, i32) {
    const CORDIC_GAIN_INVERSE_Q30: i32 = 0x26dd_3b6a;
    const ONE_Q30: i32 = 1 << 30;
    match angle.to_bits() {
        0x0000_0000 => return ( ONE_Q30, 0),
        0x4000_0000 => return (0,  ONE_Q30),
        0x8000_0000 => return (-ONE_Q30, 0),
        0xc000_0000 => return (0, -ONE_Q30),
        _ => {}
    }
    let mut angle = angle.to_bits() as i32 as i64;
    let mut sign = 1;
    if angle > Angle::QUARTER_TURN.0 as i64 {
        angle -= Angle::HALF_TURN.0 as i64;
        sign = -1;
    } else if angle < -(Angle::QUARTER_TURN.0 as i64) {
        angle += Angle::HALF_TURN.0 as i64;
        sign = -1;
    }
    let (mut x, mut y) = (CORDIC_GAIN_INVERSE_Q30, 0);
    for (shift, increment) in CORDIC_ATAN_TURNS.into_iter().enumerate() {
        let (old_x, old_y) = (x, y);
        if angle >= 0 {
            x = old_x - (old_y >> shift);
            y = old_y + (old_x >> shift);
            angle -= increment;
        } else {
            x = old_x + (old_y >> shift);
            y = old_y - (old_x >> shift);
            angle += increment;
        }
    }
    (x * sign, y * sign)
}

pub(crate) fn integer_sqrt(value: u128) -> u128 {
    if value <= u64::MAX as u128 {
        return integer_sqrt_u64(value as u64) as u128;
    }
    if value < 2 { return value; }
    let mut estimate = 1_u128 << (128 - value.leading_zeros()).div_ceil(2);
    loop {
        let next = (estimate + value / estimate) / 2;
        if next >= estimate { return estimate; }
        estimate = next;
    }
}

pub(crate) fn integer_sqrt_u64(value: u64) -> u64 {
    if value < 2 { return value; }
    let mut estimate = 1_u64 << (64 - value.leading_zeros()).div_ceil(2);
    loop {
        let next = (estimate + value / estimate) / 2;
        if next >= estimate { return estimate; }
        estimate = next;
    }
}

pub(crate) fn scaled_integer_sqrt_u64(value: u64) -> (u64, u64) {
    const MAX_FRACTION_BITS: u32 = 16;
    let fraction_bits = (value.leading_zeros() / 2).min(MAX_FRACTION_BITS);
    let scaled = value << (fraction_bits * 2);
    let floor = integer_sqrt_u64(scaled);
    let root = if scaled - floor * floor > floor { floor + 1 } else { floor };
    (root, 1 << fraction_bits)
}

pub(crate) fn scaled_integer_sqrt(value: u128) -> (u128, u128) {
    const MAX_FRACTION_BITS: u32 = 16;
    if let Ok(value) = u64::try_from(value) {
        let (root, scale) = scaled_integer_sqrt_u64(value);
        return (root as _, scale as _);
    }
    let fraction_bits = (value.leading_zeros() / 2).min(MAX_FRACTION_BITS);
    let scaled = value << (fraction_bits * 2);
    let floor = integer_sqrt(scaled);
    let root = if scaled - floor * floor > floor { floor + 1 } else { floor };
    (root, 1 << fraction_bits)
}

#[cfg(test)] mod tests { use super::*;
    #[test] fn round_div_uses_symmetric_half_away_rounding() {
        assert_eq!(round_div_i64( 1, 2),  1);
        assert_eq!(round_div_i64(-1, 2), -1);
        assert_eq!(round_div_i64( 2, 3),  1);
        assert_eq!(round_div_i64(-2, 3), -1);
        assert_eq!(round_div_i64(i64::MIN, 1), i64::MIN);

        assert_eq!(round_div_u32(1, 2), 1);
        assert_eq!(round_div_u32(1, 3), 0);
        assert_eq!(round_div_u32(u32::MAX, 1), u32::MAX);

        assert_eq!(round_div_u64(1, 2), 1);
        assert_eq!(round_div_u64(1, 3), 0);
        assert_eq!(round_div_u64(u64::MAX, 1), u64::MAX);
    }
}

#[cfg(all(test, feature = "f32"))] mod refer_tests {
    use super::*; use crate::float::{cos, sin};
    #[test] fn rotation_cordic_tracks_unit_circle() {
        let mut maximum_error = 0.0_f32;
        for step in 0..65_536_u32 {
            let angle = Angle::from_bits(step << 16);
            let (x, y) = cordic_unit_vector(angle);
            let radians = step as f32 / 65_536.0 * core::f32::consts::TAU;
            maximum_error = maximum_error
                .max((x as f32 / (1_u32 << 30) as f32 - cos(radians)).abs())
                .max((y as f32 / (1_u32 << 30) as f32 - sin(radians)).abs());
        }
        assert!(maximum_error <= 4e-5, "maximum unit-vector error={maximum_error}");
    }
}
