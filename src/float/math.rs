//! Floating-point math dispatch for hosted, hard-float, and soft-float builds.
//!
//! A hardware FPU does not imply hardware transcendental functions. Hosted
//! builds use the platform implementations; no_std builds use direct or
//! hardware-friendly operations where the target guarantees them and retain
//! `libm` for transcendental functions. Its architecture dispatch remains
//! enabled, but only affects operations and targets for which `libm` provides
//! a specialized implementation.

#[cfg(feature = "std")]
pub(crate) fn floor(value: f32) -> f32 { value.floor() }
#[cfg(all(not(feature = "std"),
    any(feature = "native-float", target_abi = "eabihf")))]
pub(crate) fn floor(value: f32) -> f32 {
    if value == 0.0 || !value.is_finite() || value.abs() >= 8_388_608.0 { return value; }
    let integer = value as i32 as f32;
    if integer > value { integer - 1.0 } else { integer }
}
#[cfg(all(not(feature = "std"),
    not(any(feature = "native-float", target_abi = "eabihf"))))]
pub(crate) fn floor(value: f32) -> f32 { libm::floorf(value) }

#[cfg(feature = "std")]
pub(crate) fn ceil(value: f32) -> f32 { value.ceil() }
#[cfg(all(not(feature = "std"),
    any(feature = "native-float", target_abi = "eabihf")))]
pub(crate) fn ceil(value: f32) -> f32 {
    if value == 0.0 || !value.is_finite() || value.abs() >= 8_388_608.0 { return value; }
    let integer = value as i32 as f32;
    if integer < value { integer + 1.0 }
    else if integer == 0.0 && value.is_sign_negative() { -0.0 }
    else { integer }
}
#[cfg(all(not(feature = "std"),
    not(any(feature = "native-float", target_abi = "eabihf"))))]
pub(crate) fn ceil(value: f32) -> f32 { libm::ceilf(value) }

#[cfg(feature = "std")]
pub(crate) fn sqrt(value: f32) -> f32 { value.sqrt() }
#[cfg(not(feature = "std"))]
pub(crate) fn sqrt(value: f32) -> f32 { libm::sqrtf(value) }

pub(crate) fn fmod(value: f32, modulus: f32) -> f32 { value % modulus }

#[cfg(feature = "std")]
pub(crate) fn pow(value: f32, exponent: f32) -> f32 { value.powf(exponent) }
#[cfg(not(feature = "std"))]
pub(crate) fn pow(value: f32, exponent: f32) -> f32 { libm::powf(value, exponent) }

#[cfg(feature = "std")]
pub(crate) fn sin(value: f32) -> f32 { value.sin() }
#[cfg(not(feature = "std"))]
pub(crate) fn sin(value: f32) -> f32 { libm::sinf(value) }

#[cfg(feature = "std")]
pub(crate) fn cos(value: f32) -> f32 { value.cos() }
#[cfg(not(feature = "std"))]
pub(crate) fn cos(value: f32) -> f32 { libm::cosf(value) }

#[cfg(feature = "std")]
pub(crate) fn acos(value: f32) -> f32 { value.acos() }
#[cfg(not(feature = "std"))]
pub(crate) fn acos(value: f32) -> f32 { libm::acosf(value) }

#[cfg(feature = "std")]
pub(crate) fn atan2(y: f32, x: f32) -> f32 { y.atan2(x) }
#[cfg(not(feature = "std"))]
pub(crate) fn atan2(y: f32, x: f32) -> f32 { libm::atan2f(y, x) }

#[cfg(test)] mod tests { use super::*;
    #[test] fn configured_rounding_matches_libm_boundaries() {
        let values = [f32::NEG_INFINITY, -16_777_216.0, -8_388_608.0, -2.5, -1.0,
            -0.75, -0.0, 0.0, 0.75, 1.0, 2.5, 8_388_608.0, 16_777_216.0,
            f32::INFINITY, f32::NAN];
        for value in values {
            assert_eq!(floor(value).to_bits(), libm::floorf(value).to_bits());
            assert_eq!(ceil(value).to_bits(), libm::ceilf(value).to_bits());
        }
    }
}
