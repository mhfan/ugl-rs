//! Paint sampling contracts for the fixed-point rendering backend.

use crate::{color::PremulSRGBA8, geometry::{FIXED_DEVICE_RAW_LIMIT, FixedScalar, Point},
    sampler::{GradientError, SolidPaint, SpreadMode}};
use super::math::{cordic_turn, integer_sqrt_u64, scaled_integer_sqrt};

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

/// Allocation-free, no-FPU linear gradient over a caller-provided encoded ramp.
///
/// Geometry is Q24.8. Projection and ramp selection use exact widened integer
/// arithmetic; the selected ramp entry is nearest to the mapped parameter.
#[derive(Clone, Copy, Debug)] pub struct FixedLinearGradient<'a> {
    from: [i32; 2], delta: [i64; 2], length_squared: i128,
    ramp: &'a [PremulSRGBA8], spread: SpreadMode,
}

impl<'a> FixedLinearGradient<'a> {
    pub fn new(from: impl Into<Point<FixedScalar>>, to: impl Into<Point<FixedScalar>>,
        ramp: &'a [PremulSRGBA8], spread: SpreadMode) ->
        Result<Self, GradientError> {
        validate_fixed_ramp(ramp)?;
        let (from, to) = (from.into(), to.into());
        let from = [from.x.to_bits(), from.y.to_bits()];
        let delta = [
            to.x.to_bits() as i64 - from[0] as i64,
            to.y.to_bits() as i64 - from[1] as i64,
        ];
        let length_squared = delta[0] as i128 * delta[0] as i128 +
                             delta[1] as i128 * delta[1] as i128;
        if length_squared == 0 { return Err(GradientError::DegenerateGeometry); }
        Ok(Self { from, delta, length_squared, ramp, spread })
    }

    pub fn ramp(&self) -> &'a [PremulSRGBA8] { self.ramp }
    pub fn spread(&self) -> SpreadMode { self.spread }

    fn ramp_index(&self, x: u32, y: u32) -> usize {
        const HALF_PIXEL_RAW: i128 = 1 << 7;
        const SUBPIXEL_SCALE: i128 = 1 << 8;
        let point = [
            x as i128 * SUBPIXEL_SCALE + HALF_PIXEL_RAW - self.from[0] as i128,
            y as i128 * SUBPIXEL_SCALE + HALF_PIXEL_RAW - self.from[1] as i128,
        ];
        let parameter = point[0] * self.delta[0] as i128 +
                        point[1] * self.delta[1] as i128;
        fixed_ramp_index(parameter, self.length_squared, self.ramp.len(), self.spread)
    }
}

impl FixedPaintSampler for FixedLinearGradient<'_> {
    fn sample_fixed(&self, x: u32, y: u32) -> PremulSRGBA8 {
        self.ramp[self.ramp_index(x, y)]
    }
}

/// Allocation-free, no-FPU two-circle radial gradient.
///
/// Geometry uses Q24.8 within [`FIXED_DEVICE_RAW_LIMIT`]. Root solving,
/// spread, and ramp mapping use widened integer arithmetic.
#[derive(Clone, Copy, Debug)] pub struct FixedRadialGradient<'a> {
    start: [i32; 2], center_delta: [i64; 2],
    start_radius: i64, radius_delta: i64, quadratic: i128,
    ramp: &'a [PremulSRGBA8], spread: SpreadMode,
}

impl<'a> FixedRadialGradient<'a> {
    /// Creates a concentric gradient from radius zero to `radius`.
    pub fn new(center: impl Into<Point<FixedScalar>>, radius: FixedScalar,
        ramp: &'a [PremulSRGBA8], spread: SpreadMode) ->
        Result<Self, GradientError> {
        let center = center.into();
        Self::two_circle(center, FixedScalar::ZERO, center, radius, ramp, spread)
    }

    /// Creates a concentric gradient between two non-negative radii.
    pub fn with_radii(center: impl Into<Point<FixedScalar>>,
        start_radius: FixedScalar, end_radius: FixedScalar,
        ramp: &'a [PremulSRGBA8], spread: SpreadMode) ->
        Result<Self, GradientError> {
        let center = center.into();
        Self::two_circle(center, start_radius, center, end_radius, ramp, spread)
    }

    /// Creates the general gradient between two circles.
    pub fn two_circle(start: impl Into<Point<FixedScalar>>, start_radius: FixedScalar,
        end: impl Into<Point<FixedScalar>>, end_radius: FixedScalar,
        ramp: &'a [PremulSRGBA8], spread: SpreadMode) ->
        Result<Self, GradientError> {
        validate_fixed_ramp(ramp)?;
        if start_radius < FixedScalar::ZERO || end_radius < FixedScalar::ZERO {
            return Err(GradientError::NegativeRadius);
        }
        let (start, end) = (start.into(), end.into());
        let raw = [start.x.to_bits(), start.y.to_bits(), end.x.to_bits(),
                   end.y.to_bits(), start_radius.to_bits(), end_radius.to_bits()];
        if raw.iter().any(|value|
            value.unsigned_abs() > FIXED_DEVICE_RAW_LIMIT as u32) {
            return Err(GradientError::CoordinateOutOfRange);
        }
        let start = [start.x.to_bits(), start.y.to_bits()];
        let center_delta = [
            end.x.to_bits() as i64 - start[0] as i64,
            end.y.to_bits() as i64 - start[1] as i64,
        ];
        let radius_delta = end_radius.to_bits() as i64 - start_radius.to_bits() as i64;
        if center_delta == [0, 0] && radius_delta == 0 {
            return Err(GradientError::DegenerateGeometry);
        }
        let quadratic = center_delta[0] as i128 * center_delta[0] as i128 +
                        center_delta[1] as i128 * center_delta[1] as i128 -
                        radius_delta as i128 * radius_delta as i128;
        Ok(Self { start, center_delta, start_radius: start_radius.to_bits() as _,
            radius_delta, quadratic, ramp, spread })
    }

    pub fn ramp(&self) -> &'a [PremulSRGBA8] { self.ramp }
    pub fn spread(&self) -> SpreadMode { self.spread }

    fn concentric_ramp_index(&self, x: u32, y: u32) -> Option<usize> {
        const HALF_PIXEL_RAW: i64 = 1 << 7;
        const SUBPIXEL_SCALE: u64 = 1 << 8;
        let (x, y) = (x as u64 * SUBPIXEL_SCALE + HALF_PIXEL_RAW as u64,
                      y as u64 * SUBPIXEL_SCALE + HALF_PIXEL_RAW as u64);
        if x > FIXED_DEVICE_RAW_LIMIT as u64 || y > FIXED_DEVICE_RAW_LIMIT as u64 {
            return None;
        }
        let (dx, dy) = (x as i64 - self.start[0] as i64,
                        y as i64 - self.start[1] as i64);
        let squared = (dx * dx + dy * dy) as u64;
        let floor = integer_sqrt_u64(squared);
        let distance = if squared - floor * floor > floor { floor + 1 } else { floor };
        let (mut parameter, mut denominator) =
            (distance as i64 - self.start_radius, self.radius_delta);
        if denominator < 0 {
            parameter = -parameter;
            denominator = -denominator;
        }
        Some(fixed_ramp_index_i64(
            parameter, denominator, self.ramp.len(), self.spread))
    }

    fn parameter(&self, x: u32, y: u32) -> Option<(i128, i128)> {
        const HALF_PIXEL_RAW: i64 = 1 << 7;
        const SUBPIXEL_SCALE: u64 = 1 << 8;
        let (x, y) = (x as u64 * SUBPIXEL_SCALE + HALF_PIXEL_RAW as u64,
                      y as u64 * SUBPIXEL_SCALE + HALF_PIXEL_RAW as u64);
        if x > FIXED_DEVICE_RAW_LIMIT as u64 || y > FIXED_DEVICE_RAW_LIMIT as u64 {
            return None;
        }
        let point = [x as i64 - self.start[0] as i64,
                     y as i64 - self.start[1] as i64];
        let linear_half = point[0] as i128 * self.center_delta[0] as i128 +
                          point[1] as i128 * self.center_delta[1] as i128 +
                          self.start_radius as i128 * self.radius_delta as i128;
        let constant = point[0] as i128 * point[0] as i128 +
                       point[1] as i128 * point[1] as i128 -
                       self.start_radius as i128 * self.start_radius as i128;
        if self.quadratic == 0 {
            if linear_half == 0 {
                return (constant == 0).then_some((0, 1));
            }
            let ratio = normalize_ratio(constant, linear_half * 2)?;
            return self.valid_radius(ratio).then_some(ratio);
        }
        let discriminant = linear_half * linear_half - self.quadratic * constant;
        if discriminant < 0 { return None; }
        let (root, scale) = scaled_integer_sqrt(discriminant as _);
        let (linear_half, quadratic) =
            (linear_half * scale as i128, self.quadratic * scale as i128);
        let first = normalize_ratio(linear_half + root as i128, quadratic)?;
        let second = normalize_ratio(linear_half - root as i128, quadratic)?;
        debug_assert_eq!(first.1, second.1);
        [first, second].into_iter().filter(|ratio| self.valid_radius(*ratio))
            .max_by_key(|ratio| ratio.0)
    }

    fn valid_radius(&self, (numerator, denominator): (i128, i128)) -> bool {
        self.start_radius as i128 * denominator +
            numerator * self.radius_delta as i128 >= 0
    }
}

impl FixedPaintSampler for FixedRadialGradient<'_> {
    fn sample_fixed(&self, x: u32, y: u32) -> PremulSRGBA8 {
        if self.center_delta == [0, 0] {
            return self.concentric_ramp_index(x, y)
                .map_or_else(PremulSRGBA8::zeroed, |index| self.ramp[index]);
        }
        self.parameter(x, y).map_or_else(PremulSRGBA8::zeroed,
            |(parameter, denominator)| self.ramp[
                fixed_ramp_index(parameter, denominator, self.ramp.len(), self.spread)])
    }
}

fn normalize_ratio(mut numerator: i128, mut denominator: i128) -> Option<(i128, i128)> {
    if denominator == 0 { return None; }
    if denominator < 0 {
        numerator = -numerator;
        denominator = -denominator;
    }
    Some((numerator, denominator))
}

fn validate_fixed_ramp(ramp: &[PremulSRGBA8]) -> Result<(), GradientError> {
    if ramp.len() < 2 { return Err(GradientError::RampTooSmall); }
    if ramp.len() > u32::MAX as usize { return Err(GradientError::RampTooLarge); }
    Ok(())
}

fn fixed_ramp_index(parameter: i128, denominator: i128, ramp_len: usize,
    spread: SpreadMode) -> usize {
    debug_assert!(denominator > 0);
    if let (Ok(parameter), Ok(denominator)) =
        (i64::try_from(parameter), i64::try_from(denominator)) {
        return fixed_ramp_index_i64(parameter, denominator, ramp_len, spread);
    }
    let mapped = match spread {
        SpreadMode::Pad => parameter.clamp(0, denominator),
        SpreadMode::Repeat => parameter.rem_euclid(denominator),
        SpreadMode::Reflect => {
            let period = parameter.rem_euclid(denominator * 2);
            if period <= denominator { period } else { denominator * 2 - period }
        }
    };
    let scale = (ramp_len - 1) as i128;
    ((mapped * scale + denominator / 2) / denominator) as _
}

/// Narrow equivalent of `fixed_ramp_index` for the concentric radial hot path.
fn fixed_ramp_index_i64(parameter: i64, denominator: i64, ramp_len: usize,
    spread: SpreadMode) -> usize {
    debug_assert!(denominator > 0);
    let mapped = match spread {
        SpreadMode::Pad => parameter.clamp(0, denominator),
        SpreadMode::Repeat => parameter.rem_euclid(denominator),
        SpreadMode::Reflect => {
            let period = parameter.rem_euclid(denominator * 2);
            if period <= denominator { period } else { denominator * 2 - period }
        }
    } as u64;
    let (scale, denominator) = ((ramp_len - 1) as u64, denominator as u64);
    ((mapped * scale + denominator / 2) / denominator) as _
}

/// Allocation-free, no-FPU conic gradient using a 16-step integer CORDIC.
#[derive(Clone, Copy, Debug)] pub struct FixedConicGradient<'a> {
    center: [i32; 2], start_angle: FixedAngle,
    ramp: &'a [PremulSRGBA8],
}

impl<'a> FixedConicGradient<'a> {
    pub fn new(center: impl Into<Point<FixedScalar>>, start_angle: FixedAngle,
        ramp: &'a [PremulSRGBA8]) -> Result<Self, GradientError> {
        validate_fixed_ramp(ramp)?;
        let center = center.into();
        let center = [center.x.to_bits(), center.y.to_bits()];
        if center.iter().any(|value|
            value.unsigned_abs() > FIXED_DEVICE_RAW_LIMIT as u32) {
            return Err(GradientError::CoordinateOutOfRange);
        }
        Ok(Self { center, start_angle, ramp })
    }

    pub fn ramp(&self) -> &'a [PremulSRGBA8] { self.ramp }
    pub fn start_angle(&self) -> FixedAngle { self.start_angle }

    fn ramp_index(&self, x: u32, y: u32) -> Option<usize> {
        const HALF_PIXEL_RAW: i64 = 1 << 7;
        const SUBPIXEL_SCALE: u64 = 1 << 8;
        const FULL_TURN: u64 = 1_u64 << 32;
        let (x, y) = (x as u64 * SUBPIXEL_SCALE + HALF_PIXEL_RAW as u64,
                      y as u64 * SUBPIXEL_SCALE + HALF_PIXEL_RAW as u64);
        if x > FIXED_DEVICE_RAW_LIMIT as u64 || y > FIXED_DEVICE_RAW_LIMIT as u64 {
            return None;
        }
        let angle = cordic_turn(x as i64 - self.center[0] as i64,
                                y as i64 - self.center[1] as i64);
        let parameter = angle.wrapping_sub(self.start_angle.to_bits()) as u64;
        let scale = (self.ramp.len() - 1) as u64;
        Some(((parameter * scale + FULL_TURN / 2) / FULL_TURN) as _)
    }
}

impl FixedPaintSampler for FixedConicGradient<'_> {
    fn sample_fixed(&self, x: u32, y: u32) -> PremulSRGBA8 {
        self.ramp_index(x, y).map_or_else(PremulSRGBA8::zeroed,
            |index| self.ramp[index])
    }
}
