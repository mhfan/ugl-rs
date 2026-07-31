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

#[cfg(test)] mod tests {
    use super::*;
    use super::super::math::{cordic_turn, integer_sqrt};
    use crate::{color::SRGBA, sampler::{
        ConicGradient, GradientStop, GradientStops, LinearGradient, PaintSampler,
        RadialGradient,
    }};

    const TAU: f32 = core::f32::consts::PI * 2.0;

    fn encoded(color: SRGBA<u8>) -> PremulSRGBA8 { color.premul_encoded() }
    fn red_blue_stops() -> [GradientStop; 2] {
        [GradientStop::new(0.0, SRGBA::red()),
         GradientStop::new(1.0, SRGBA::blue())]
    }

    #[test] fn fixed_linear_gradient_matches_the_encoded_reference_ramp() {
        let stops = red_blue_stops();
        let mut storage = [PremulSRGBA8::zeroed(); 257];
        let stops = GradientStops::with_ramp(&stops, &mut storage).unwrap();
        let ramp = stops.encoded_ramp().unwrap();
        let (from, to) = ((FixedScalar::from_num(2), FixedScalar::from_num(0)),
                          (FixedScalar::from_num(10), FixedScalar::from_num(0)));
        for spread in [SpreadMode::Pad, SpreadMode::Repeat, SpreadMode::Reflect] {
            let fixed = FixedLinearGradient::new(from, to, ramp, spread).unwrap();
            let reference =
                LinearGradient::new((2.0, 0.0), (10.0, 0.0), stops, spread).unwrap();
            for x in 0..32 {
                assert_eq!(fixed.sample_fixed(x, 3),
                    reference.sample(x as f32 + 0.5, 3.5), "spread={spread:?}, x={x}");
            }
        }
    }


    #[test] fn fixed_linear_gradient_validates_geometry_and_widens_extremes() {
        let ramp = [encoded(SRGBA::<u8>::red()), encoded(SRGBA::<u8>::blue())];
        assert_eq!(FixedLinearGradient::new(
            (FixedScalar::from_num(0), FixedScalar::from_num(0)),
            (FixedScalar::from_num(1), FixedScalar::from_num(0)),
            &ramp[..1], SpreadMode::Pad).unwrap_err(), GradientError::RampTooSmall);
        assert_eq!(FixedLinearGradient::new(
            (FixedScalar::from_num(1), FixedScalar::from_num(2)),
            (FixedScalar::from_num(1), FixedScalar::from_num(2)),
            &ramp, SpreadMode::Pad).unwrap_err(), GradientError::DegenerateGeometry);

        let extreme = FixedLinearGradient::new(
            (FixedScalar::from_bits(i32::MIN), FixedScalar::from_bits(i32::MIN)),
            (FixedScalar::from_bits(i32::MAX), FixedScalar::from_bits(i32::MAX)),
            &ramp, SpreadMode::Reflect).unwrap();
        assert!(ramp.contains(&extreme.sample_fixed(u32::MAX, u32::MAX)));
    }


    #[test] fn fixed_concentric_radial_matches_the_encoded_reference_ramp() {
        let stops = red_blue_stops();
        let mut storage = [PremulSRGBA8::zeroed(); 257];
        let stops = GradientStops::with_ramp(&stops, &mut storage).unwrap();
        let ramp = stops.encoded_ramp().unwrap();
        let center = (FixedScalar::from_num(8), FixedScalar::from_num(8));
        for spread in [SpreadMode::Pad, SpreadMode::Repeat, SpreadMode::Reflect] {
            let fixed = FixedRadialGradient::new(
                center, FixedScalar::from_num(8), ramp, spread).unwrap();
            let reference = RadialGradient::new((8.0, 8.0), 8.0, stops, spread).unwrap();
            for y in 0..16 {
                for x in 0..16 {
                    let (actual, expected) = (fixed.sample_fixed(x, y),
                        reference.sample(x as f32 + 0.5, y as f32 + 0.5));
                    let actual = ramp.iter().position(|color| *color == actual).unwrap();
                    let expected = ramp.iter().position(|color| *color == expected).unwrap();
                    assert!(actual.abs_diff(expected) <= 1,
                        "spread={spread:?}, point=({x}, {y}), \
                         actual={actual}, expected={expected}");
                }
            }
        }

        let fixed = FixedRadialGradient::with_radii(center,
            FixedScalar::from_num(8), FixedScalar::ZERO, ramp, SpreadMode::Pad).unwrap();
        let reference = RadialGradient::two_circle(
            (8.0, 8.0), 8.0, (8.0, 8.0), 0.0, stops, SpreadMode::Pad).unwrap();
        for x in 0..16 {
            let (actual, expected) = (fixed.sample_fixed(x, 8),
                reference.sample(x as f32 + 0.5, 8.5));
            let actual = ramp.iter().position(|color| *color == actual).unwrap();
            let expected = ramp.iter().position(|color| *color == expected).unwrap();
            assert!(actual.abs_diff(expected) <= 1);
        }
    }


    #[test] fn fixed_concentric_radial_validates_radii_and_integer_sqrt() {
        let ramp = [encoded(SRGBA::<u8>::red()), encoded(SRGBA::<u8>::blue())];
        let center = (FixedScalar::ZERO, FixedScalar::ZERO);
        assert_eq!(FixedRadialGradient::new(center,
            FixedScalar::from_num(-1), &ramp, SpreadMode::Pad).unwrap_err(),
            GradientError::NegativeRadius);
        assert_eq!(FixedRadialGradient::with_radii(center,
            FixedScalar::from_num(2), FixedScalar::from_num(2),
            &ramp, SpreadMode::Pad).unwrap_err(), GradientError::DegenerateGeometry);

        for root in [0_u128, 1, 2, 3, 255, 65_535, u32::MAX as _] {
            let square = root * root;
            assert_eq!(integer_sqrt(square), root);
            if root != 0 { assert_eq!(integer_sqrt(square - 1), root - 1); }
            assert_eq!(integer_sqrt(square + root), root);
        }
        assert_eq!(integer_sqrt(u128::MAX), u64::MAX as u128);
        let mut value = 0x9e37_79b9_7f4a_7c15_d1b5_4a32_d192_ed03_u128;
        for _ in 0..1_000 {
            value = value.wrapping_mul(0xda94_2042_e4dd_58b5)
                         .wrapping_add(0x94d0_49bb_1331_11eb);
            let root = integer_sqrt(value);
            assert!(root * root <= value);
            if root < u64::MAX as u128 { assert!((root + 1) * (root + 1) > value); }
        }
    }


    #[test] fn fixed_two_circle_radial_matches_quadratic_and_linear_references() {
        fn assert_close(fixed: &FixedRadialGradient<'_>, reference: &RadialGradient<'_>,
            ramp: &[PremulSRGBA8], x: u32, y: u32) {
            let (actual, expected) = (fixed.sample_fixed(x, y),
                reference.sample(x as f32 + 0.5, y as f32 + 0.5));
            match (ramp.iter().position(|color| *color == actual),
                   ramp.iter().position(|color| *color == expected)) {
                (Some(actual), Some(expected)) => assert!(actual.abs_diff(expected) <= 1,
                    "point=({x}, {y}), actual={actual}, expected={expected}"),
                (None, None) => assert_eq!(actual, expected),
                _ => panic!("root validity differs at ({x}, {y}): {actual:?} != {expected:?}"),
            }
        }

        let stop_values = red_blue_stops();
        let mut storage = [PremulSRGBA8::zeroed(); 257];
        let stops = GradientStops::with_ramp(&stop_values, &mut storage).unwrap();
        let ramp = stops.encoded_ramp().unwrap();
        let fixed = FixedScalar::from_num;
        for spread in [SpreadMode::Pad, SpreadMode::Repeat, SpreadMode::Reflect] {
            let radial = FixedRadialGradient::two_circle(
                (fixed(1), fixed(0)), fixed(0), (fixed(0), fixed(0)), fixed(4),
                ramp, spread).unwrap();
            let reference = RadialGradient::two_circle(
                (1.0, 0.0), 0.0, (0.0, 0.0), 4.0, stops, spread).unwrap();
            for y in 0..8 {
                for x in 0..8 { assert_close(&radial, &reference, ramp, x, y); }
            }
        }

        let tangent = FixedRadialGradient::two_circle(
            (fixed(0), fixed(0)), fixed(0), (fixed(1), fixed(0)), fixed(1),
            ramp, SpreadMode::Pad).unwrap();
        let tangent_reference = RadialGradient::two_circle(
            (0.0, 0.0), 0.0, (1.0, 0.0), 1.0, stops, SpreadMode::Pad).unwrap();
        for y in 0..4 {
            for x in 0..4 { assert_close(&tangent, &tangent_reference, ramp, x, y); }
        }

        let near_tangent = FixedRadialGradient::two_circle(
            (fixed(4), fixed(4)), fixed(1),
            (FixedScalar::from_bits(4 * 256 + 257), fixed(4)), fixed(2),
            ramp, SpreadMode::Reflect).unwrap();
        let near_tangent_reference = RadialGradient::two_circle(
            (4.0, 4.0), 1.0, (5.0 + 1.0 / 256.0, 4.0), 2.0,
            stops, SpreadMode::Reflect).unwrap();
        for y in 0..12 {
            for x in 0..12 {
                assert_close(&near_tangent, &near_tangent_reference, ramp, x, y);
            }
        }
    }


    #[test] fn fixed_two_circle_radial_enforces_the_fixed_device_domain() {
        let ramp = [encoded(SRGBA::<u8>::red()), encoded(SRGBA::<u8>::blue())];
        let fixed = FixedScalar::from_num;
        assert_eq!(FixedRadialGradient::new(
            (FixedScalar::from_bits(FIXED_DEVICE_RAW_LIMIT + 1), fixed(0)), fixed(1),
            &ramp, SpreadMode::Pad).unwrap_err(), GradientError::CoordinateOutOfRange);
        let radial = FixedRadialGradient::new(
            (fixed(0), fixed(0)), fixed(1), &ramp, SpreadMode::Pad).unwrap();
        let first_outside_pixel = FIXED_DEVICE_RAW_LIMIT as u32 / 256;
        assert_eq!(radial.sample_fixed(first_outside_pixel, 0),
            PremulSRGBA8::zeroed());
    }


    #[test] fn fixed_conic_cordic_tracks_exact_angles_and_encoded_ramp() {
        assert_eq!(cordic_turn( 1,  0), FixedAngle::ZERO.to_bits());
        assert_eq!(cordic_turn( 0,  1), FixedAngle::QUARTER_TURN.to_bits());
        assert_eq!(cordic_turn(-1,  0), FixedAngle::HALF_TURN.to_bits());
        assert_eq!(cordic_turn( 0, -1), FixedAngle::THREE_QUARTER_TURN.to_bits());
        let mut maximum_error = 0.0_f32;
        for y in -64_i64..=64 {
            for x in -64_i64..=64 {
                if x == 0 && y == 0 { continue; }
                let actual = cordic_turn(x, y) as f32 / 4_294_967_296.0;
                let turn = libm::atan2f(y as _, x as _) / TAU;
                let expected = turn - libm::floorf(turn);
                let difference = (actual - expected).abs();
                maximum_error = maximum_error.max(difference.min(1.0 - difference));
            }
        }
        assert!(maximum_error <= 6e-6, "maximum turn error={maximum_error}");

        let stop_values = red_blue_stops();
        let mut storage = [PremulSRGBA8::zeroed(); 257];
        let stops = GradientStops::with_ramp(&stop_values, &mut storage).unwrap();
        let ramp = stops.encoded_ramp().unwrap();
        let fixed = FixedScalar::from_num;
        for (angle, start_angle) in [
            (FixedAngle::ZERO, 0.0),
            (FixedAngle::QUARTER_TURN, TAU / 4.0),
        ] {
            let conic = FixedConicGradient::new(
                (fixed(16), fixed(16)), angle, ramp).unwrap();
            let reference = ConicGradient::new((16.0, 16.0), start_angle, stops).unwrap();
            for y in 0..32 {
                for x in 0..32 {
                    let (actual, expected) = (conic.sample_fixed(x, y),
                        reference.sample(x as f32 + 0.5, y as f32 + 0.5));
                    let actual = ramp.iter().position(|color| *color == actual).unwrap();
                    let expected = ramp.iter().position(|color| *color == expected).unwrap();
                    assert!(actual.abs_diff(expected) <= 1,
                        "point=({x}, {y}), actual={actual}, expected={expected}");
                }
            }
        }
    }


    #[test] fn fixed_conic_validates_ramp_and_device_domain() {
        let ramp = [encoded(SRGBA::<u8>::red()), encoded(SRGBA::<u8>::blue())];
        let fixed = FixedScalar::from_num;
        assert_eq!(FixedAngle::from_turn_fraction(1, 4), Some(FixedAngle::QUARTER_TURN));
        assert_eq!(FixedAngle::from_turn_fraction(1, 0), None);
        assert_eq!(FixedConicGradient::new((fixed(0), fixed(0)),
            FixedAngle::ZERO, &ramp[..1]).unwrap_err(), GradientError::RampTooSmall);
        assert_eq!(FixedConicGradient::new(
            (FixedScalar::from_bits(FIXED_DEVICE_RAW_LIMIT + 1), fixed(0)),
            FixedAngle::ZERO, &ramp).unwrap_err(), GradientError::CoordinateOutOfRange);
        let conic = FixedConicGradient::new(
            (fixed(0), fixed(0)), FixedAngle::ZERO, &ramp).unwrap();
        assert_eq!(conic.sample_fixed(FIXED_DEVICE_RAW_LIMIT as u32 / 256, 0),
            PremulSRGBA8::zeroed());
    }
}
