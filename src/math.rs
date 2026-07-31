//! Numeric building blocks shared by rendering backends.
//!
//! Geometry no longer exposes a third-party matrix type. The optional re-export
//! remains for clients experimenting with fixed-point coordinates.

//  number: Q-number/Fixed/float/double, performance/fast vs quality/accurate
//  linear algebra, affine transformation, trigonometry

//  https://github.com/dimforge/nalgebra/blob/main/nalgebra-glm/src/aliases.rs
//pub use nalgebra_glm::TMat;

//  https://en.wikipedia.org/wiki/Q_(number_format)
//  https://johnmcfarlane.github.io/cnl/
//  https://gitlab.com/tspiteri/fixed
#[cfg(feature = "fixed")] pub use fixed::traits::Fixed;

#[cfg(feature = "fixed")]
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

#[cfg(feature = "fixed")]
pub(crate) fn integer_sqrt_u64(value: u64) -> u64 {
    if value < 2 { return value; }
    let mut estimate = 1_u64 << (64 - value.leading_zeros()).div_ceil(2);
    loop {
        let next = (estimate + value / estimate) / 2;
        if next >= estimate { return estimate; }
        estimate = next;
    }
}

#[cfg(feature = "fixed")]
pub(crate) fn scaled_integer_sqrt(value: u128) -> (u128, u128) {
    const MAX_FRACTION_BITS: u32 = 16;
    if let Ok(value) = u64::try_from(value) {
        let fraction_bits = (value.leading_zeros() / 2).min(MAX_FRACTION_BITS);
        let scaled = value << (fraction_bits * 2);
        let floor = integer_sqrt_u64(scaled);
        let root = if scaled - floor * floor > floor { floor + 1 } else { floor };
        return (root as _, 1 << fraction_bits);
    }
    let fraction_bits = (value.leading_zeros() / 2).min(MAX_FRACTION_BITS);
    let scaled = value << (fraction_bits * 2);
    let floor = integer_sqrt(scaled);
    let root = if scaled - floor * floor > floor { floor + 1 } else { floor };
    (root, 1 << fraction_bits)
}
