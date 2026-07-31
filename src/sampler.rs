
//! Allocation-free paint samplers.
//!
//! Sampling currently uses device-space `f32` pixel centers as the reference
//! implementation. The compositor is generic over this trait, so later fixed
//! coordinate samplers can be introduced without changing premultiplied color
//! storage or raster coverage.

/*  Samplers can be though of as 2D shaders. Sampler is a first class citizen in *ugl-rs*,
    think of them as an object, that can be sampled in the normalized unit square.

    Sampler can be anything, that can be sampled such as:
      a fixed color
      a gradient (linear/radial/conic)
      a texture (image)
 */

use crate::{color::{PremulRGBA, RGBA}, geometry::{Affine, Point}};

/// Produces premultiplied source colors at device-space positions.
///
/// Implementations should be small values borrowed by the compositor. Calls are
/// statically dispatched; no trait object or allocation is required.
pub trait PaintSampler {
    fn sample(&self, x: f32, y: f32) -> PremulRGBA<u8>;

    /// Reports a position-independent color to enable span and tile fast paths.
    fn solid_color(&self) -> Option<PremulRGBA<u8>> { None }
}

impl<S: PaintSampler + ?Sized> PaintSampler for &S {
    fn sample(&self, x: f32, y: f32) -> PremulRGBA<u8> { (**self).sample(x, y) }
    fn solid_color(&self) -> Option<PremulRGBA<u8>> { (**self).solid_color() }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum PaintTransformError {
    NonInvertibleTransform,
}

/// Maps device-space samples through a precomputed inverse paint transform.
#[derive(Clone, Copy, Debug)] pub struct TransformedPaint<S> {
    sampler: S, device_to_paint: Affine,
}

impl<S> TransformedPaint<S> {
    pub fn new(sampler: S, paint_to_device: Affine) -> Result<Self, PaintTransformError> {
        let device_to_paint = paint_to_device.inverse()
            .ok_or(PaintTransformError::NonInvertibleTransform)?;
        Ok(Self { sampler, device_to_paint })
    }

    pub fn sampler(&self) -> &S { &self.sampler }
    pub fn device_to_paint(&self) -> Affine { self.device_to_paint }
}

impl<S: PaintSampler> PaintSampler for TransformedPaint<S> {
    fn sample(&self, x: f32, y: f32) -> PremulRGBA<u8> {
        let point = self.device_to_paint.transform_point((x, y).into());
        self.sampler.sample(point.x, point.y)
    }

    fn solid_color(&self) -> Option<PremulRGBA<u8>> { self.sampler.solid_color() }
}

#[derive(Clone, Copy, Debug, PartialEq)] pub struct SolidPaint { color: PremulRGBA<u8> }

impl SolidPaint {
    pub fn new(color: RGBA<u8>) -> Self { Self { color: color.premul() } }
    pub fn premultiplied(color: PremulRGBA<u8>) -> Self { Self { color } }
    pub fn color(&self) -> PremulRGBA<u8> { self.color }
}

impl From<RGBA<u8>> for SolidPaint { fn from(color: RGBA<u8>) -> Self { Self::new(color) } }

impl From<PremulRGBA<u8>> for SolidPaint {
    fn from(color: PremulRGBA<u8>) -> Self { Self::premultiplied(color) }
}

impl PaintSampler for SolidPaint {
    fn sample(&self, _x: f32, _y: f32) -> PremulRGBA<u8> { self.color }
    fn solid_color(&self) -> Option<PremulRGBA<u8>> { Some(self.color) }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop { offset: f32, color: PremulRGBA<u8> }

impl GradientStop {
    pub fn new(offset: f32, color: RGBA<u8>) -> Self {
        Self { offset, color: color.premul() }
    }

    pub fn premultiplied(offset: f32, color: PremulRGBA<u8>) -> Self { Self { offset, color } }

    pub fn offset(&self) -> f32 { self.offset }
    pub fn color(&self) -> PremulRGBA<u8> { self.color }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum GradientError {
    EmptyStops, NonFiniteOffset, OffsetOutOfRange, UnorderedStops,
    NonFiniteGeometry, NegativeRadius, DegenerateGeometry,
}

/// Validated, caller-owned gradient stops.
#[derive(Clone, Copy, Debug)] pub struct GradientStops<'a> { stops: &'a [GradientStop] }

impl<'a> GradientStops<'a> {
    pub fn new(stops: &'a [GradientStop]) -> Result<Self, GradientError> {
        if stops.is_empty() { return Err(GradientError::EmptyStops); }
        let mut previous = 0.0;
        for (index, stop) in stops.iter().enumerate() {
            if !stop.offset.is_finite() { return Err(GradientError::NonFiniteOffset); }
            if !(0.0..=1.0).contains(&stop.offset) {
                return Err(GradientError::OffsetOutOfRange);
            }
            if index != 0 && stop.offset < previous {
                return Err(GradientError::UnorderedStops);
            }   previous =   stop.offset;
        }   Ok(Self { stops })
    }

    pub fn as_slice(&self) -> &'a [GradientStop] { self.stops }

    fn sample(&self, t: f32) -> PremulRGBA<u8> {
        let upper = self.stops.partition_point(|stop| stop.offset <= t);
        if  upper == 0 { return self.stops[0].color; }
        if  upper == self.stops.len() { return self.stops[upper - 1].color; }
        let (from, to) = (self.stops[upper - 1], self.stops[upper]);
        let extent = to.offset - from.offset;
        if  extent == 0.0 { return to.color; }
        let position = (t - from.offset) / extent;
        let (from, to) = (from.color.to_array(), to.color.to_array());
        let lerp = |from: u8, to: u8| {
            (from as f32 + (to as f32 - from as f32) * position + 0.5)
                .clamp(0.0, u8::MAX as _) as u8
        };
        (lerp(from[0], to[0]), lerp(from[1], to[1]),
         lerp(from[2], to[2]), lerp(from[3], to[3])).into()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SpreadMode { #[default] Pad, Repeat, Reflect }

impl SpreadMode {
    fn map(self, t: f32) -> f32 {
        match self {
            Self::Pad => t.clamp(0.0, 1.0),
            Self::Repeat  => t - libm::floorf(t),
            Self::Reflect => {
                let period = t - libm::floorf(t * 0.5) * 2.0;
                if  period <= 1.0 { period } else { 2.0 - period }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)] pub struct LinearGradient<'a> {
    from: Point, delta: Point, inverse_length_squared: f32,
    stops: GradientStops<'a>, spread: SpreadMode,
}

impl<'a> LinearGradient<'a> {
    pub fn new(from: impl Into<Point>, to: impl Into<Point>, stops: GradientStops<'a>,
        spread: SpreadMode) -> Result<Self, GradientError> {
        let (from, to) = (from.into(), to.into());
        if !from.x.is_finite() || !from.y.is_finite() ||
             !to.x.is_finite() ||   !to.y.is_finite() {
            return Err(GradientError::NonFiniteGeometry);
        }
        let delta: Point = (to.x - from.x, to.y - from.y).into();
        let length_squared = delta.x * delta.x + delta.y * delta.y;
        if !length_squared.is_finite() { return Err(GradientError::NonFiniteGeometry) }
        if  length_squared == 0.0      { return Err(GradientError::DegenerateGeometry) }
        Ok(Self { from, delta, stops, spread,
            inverse_length_squared: length_squared.recip(),
        })
    }
}

impl PaintSampler for LinearGradient<'_> {
    fn sample(&self, x: f32, y: f32) -> PremulRGBA<u8> {
        let t = ((x - self.from.x) * self.delta.x  +
                 (y - self.from.y) * self.delta.y) * self.inverse_length_squared;
        self.stops.sample(self.spread.map(t))
    }
}

/// A two-circle radial gradient.
///
/// Circle center and radius are linearly interpolated by the gradient
/// parameter. Samples outside the cone formed by the circles are transparent.
#[derive(Clone, Copy, Debug)] pub struct RadialGradient<'a> {
    start: Point, center_delta: Point, start_radius: f32, radius_delta: f32,
    quadratic: f32, stops: GradientStops<'a>, spread: SpreadMode,
}

impl<'a> RadialGradient<'a> {
    /// Creates a concentric gradient from radius zero to `radius`.
    pub fn new(center: impl Into<Point>, radius: f32, stops: GradientStops<'a>,
        spread: SpreadMode) -> Result<Self, GradientError> {
        let center = center.into();
        Self::two_circle(center, 0.0, center, radius, stops, spread)
    }

    pub fn two_circle(start: impl Into<Point>, start_radius: f32,
        end: impl Into<Point>, end_radius: f32, stops: GradientStops<'a>,
        spread: SpreadMode) -> Result<Self, GradientError> {
        let (start, end) = (start.into(), end.into());
        if  !start.x.is_finite() || !start.y.is_finite() ||
              !end.x.is_finite() ||   !end.y.is_finite() ||
            !start_radius.is_finite() || !end_radius.is_finite() {
            return Err(GradientError::NonFiniteGeometry);
        }
        if start_radius < 0.0 || end_radius < 0.0 {
            return Err(GradientError::NegativeRadius);
        }
        let radius_delta = end_radius - start_radius;
        let center_delta: Point = (end.x - start.x, end.y - start.y).into();
        let quadratic = center_delta.x * center_delta.x +
                        center_delta.y * center_delta.y - radius_delta * radius_delta;
        if !center_delta.x.is_finite() || !center_delta.y.is_finite() ||
           !radius_delta.is_finite() || !quadratic.is_finite() {
            return Err(GradientError::NonFiniteGeometry);
        }
        if center_delta.x == 0.0 && center_delta.y == 0.0 && radius_delta == 0.0 {
            return Err(GradientError::DegenerateGeometry);
        }
        Ok(Self { start, center_delta, start_radius, radius_delta, quadratic, stops, spread })
    }

    fn parameter(&self, x: f32, y: f32) -> Option<f32> {
        let point: Point = (x - self.start.x, y - self.start.y).into();
        let linear = -2.0 * (point.x * self.center_delta.x +
                             point.y * self.center_delta.y +
                            self.start_radius * self.radius_delta);
        let constant = point.x * point.x + point.y * point.y -
                       self.start_radius * self.start_radius;
        if self.quadratic == 0.0 {
            if linear == 0.0 { return if constant == 0.0 { Some(0.0) } else { None } }
            let t = -constant / linear;
            return (self.start_radius + t * self.radius_delta >= 0.0).then_some(t);
        }
        let discriminant = linear * linear - 4.0 * self.quadratic * constant;
        if  discriminant < 0.0 || !discriminant.is_finite() { return None; }
        let root = libm::sqrtf(discriminant);
        let q = -0.5 * (linear + root.copysign(linear));
        let (first, second) = if q == 0.0 {
            let root = -linear / (2.0 * self.quadratic);
            (root, root)
        } else { (q / self.quadratic, constant / q) };

        [first, second].into_iter().filter(|t| t.is_finite() &&
            self.start_radius + *t * self.radius_delta >= 0.0).max_by(|a, b| a.total_cmp(b))
    }
}

impl PaintSampler for RadialGradient<'_> {
    fn sample(&self, x: f32, y: f32) -> PremulRGBA<u8> {
        self.parameter(x, y).map_or_else(PremulRGBA::zeroed,
            |t| self.stops.sample(self.spread.map(t)))
    }
}

/// A full-turn conic gradient around `center`.
#[derive(Clone, Copy, Debug)] pub struct ConicGradient<'a> {
    center: Point, start_turn: f32, stops: GradientStops<'a>,
}

const TAU: f32 = core::f32::consts::PI * 2.0;
impl<'a> ConicGradient<'a> {
    /// Creates a conic gradient whose zero stop lies at `start_angle` radians.
    pub fn new(center: impl Into<Point>, start_angle: f32, stops: GradientStops<'a>) ->
        Result<Self, GradientError> {
        let center = center.into();
        if !center.x.is_finite() || !center.y.is_finite() || !start_angle.is_finite() {
            return Err(GradientError::NonFiniteGeometry);
        }   Ok(Self { center, start_turn: start_angle / TAU, stops })
    }
}

impl PaintSampler for ConicGradient<'_> {
    fn sample(&self, x: f32, y: f32) -> PremulRGBA<u8> {
        let turn =  libm::atan2f(y - self.center.y, x - self.center.x) / TAU;
        self.stops.sample(SpreadMode::Repeat.map(turn - self.start_turn))
    }
}

#[cfg(test)] mod tests { use super::*;
    #[test] fn solid_paint_is_position_independent_and_premultiplied() {
        let paint = SolidPaint::new(RGBA::new(200, 100, 50, 128));
        assert_eq!(paint.sample(0.5, 0.5), (100, 50, 25, 128).into());
        assert_eq!(paint.sample(-100.0, 200.0), paint.solid_color().unwrap());
    }

    fn red_blue_stops() -> [GradientStop; 2] {
        [GradientStop::new(0.0, RGBA::red()), GradientStop::new(1.0, RGBA::blue())]
    }

    #[test] fn gradient_stops_validate_and_interpolate_premultiplied_colors() {
        assert_eq!(GradientStops::new(&[]).unwrap_err(), GradientError::EmptyStops);
        assert_eq!(GradientStops::new(&[GradientStop::new(f32::NAN, RGBA::red())])
            .unwrap_err(), GradientError::NonFiniteOffset);
        assert_eq!(GradientStops::new(&[GradientStop::new(1.25, RGBA::red())])
            .unwrap_err(), GradientError::OffsetOutOfRange);
        assert_eq!(GradientStops::new(&[GradientStop::new(0.75, RGBA::red()),
                                        GradientStop::new(0.25, RGBA::blue()),
        ]).unwrap_err(), GradientError::UnorderedStops);

        let stops = [GradientStop::new(0.0, RGBA::new(255, 0, 0, 0)),
                     GradientStop::new(1.0, RGBA::new(0, 0, 255, 255))];
        assert_eq!(GradientStops::new(&stops).unwrap().sample(0.5), (0, 0, 128, 128).into());

        let single = [GradientStop::new(0.4, RGBA::green())];
        let single = GradientStops::new(&single).unwrap();
        assert_eq!(single.sample(-10.0), single.sample(10.0));
        let hard = [GradientStop::new(0.0, RGBA::red()),
                    GradientStop::new(0.5, RGBA::red()),
                    GradientStop::new(0.5, RGBA::blue()),
                    GradientStop::new(1.0, RGBA::blue())];
        let hard = GradientStops::new(&hard).unwrap();
        assert_eq!(hard.sample(0.5 - f32::EPSILON), RGBA::<u8>::red().premul());
        assert_eq!(hard.sample(0.5), RGBA::<u8>::blue().premul());
    }

    #[test] fn linear_gradient_projects_device_coordinates_and_spreads() {
        let stops = red_blue_stops();
        let stops = GradientStops::new(&stops).unwrap();
        let pad = LinearGradient::new((1.0, 2.0), (5.0, 2.0),
            stops, SpreadMode::Pad).unwrap();
        assert_eq!(pad.sample(1.0, 100.0), RGBA::<u8>::red().premul());
        assert_eq!(pad.sample(3.0, -100.0), (128, 0, 128, 255).into());
        assert_eq!(pad.sample(8.0, 2.0), RGBA::<u8>::blue().premul());

        let repeat  = LinearGradient::new((0.0, 0.0), (1.0, 0.0),
            stops, SpreadMode::Repeat).unwrap();
        let reflect = LinearGradient::new((0.0, 0.0), (1.0, 0.0),
            stops, SpreadMode::Reflect).unwrap();
        assert_eq!(repeat.sample(1.25, 0.0), repeat.sample(0.25, 0.0));
        assert_eq!(reflect.sample(1.25, 0.0), reflect.sample(0.75, 0.0));
        assert_eq!(reflect.sample(-0.25, 0.0), reflect.sample(0.25, 0.0));
    }

    #[test] fn radial_gradient_supports_concentric_and_focal_circles() {
        let stops = red_blue_stops();
        let stops = GradientStops::new(&stops).unwrap();
        let radial =
            RadialGradient::new((2.0, 3.0), 4.0, stops, SpreadMode::Pad).unwrap();
        assert_eq!(radial.sample(2.0, 3.0), RGBA::<u8>::red().premul());
        assert_eq!(radial.sample(4.0, 3.0), (128, 0, 128, 255).into());
        assert_eq!(radial.sample(10.0, 3.0), RGBA::<u8>::blue().premul());

        let focal = RadialGradient::two_circle((1.0, 0.0), 0.0, (0.0, 0.0), 4.0,
            stops, SpreadMode::Pad).unwrap();
        assert_eq!(focal.sample( 1.0, 0.0), RGBA::<u8>:: red().premul());
        assert_eq!(focal.sample(-4.0, 0.0), RGBA::<u8>::blue().premul());
        assert_eq!(RadialGradient::new((0.0, 0.0), -1.0, stops, SpreadMode::Pad)
            .unwrap_err(), GradientError::NegativeRadius);
        assert_eq!(RadialGradient::two_circle((0.0, 0.0), 1.0, (0.0, 0.0), 1.0,
            stops, SpreadMode::Pad).unwrap_err(), GradientError::DegenerateGeometry);

        let tangent = RadialGradient::two_circle((0.0, 0.0), 0.0, (1.0, 0.0), 1.0,
            stops, SpreadMode::Pad).unwrap();
        assert_eq!(tangent.sample(0.5, 0.0), (191, 0, 64, 255).into());
        assert_eq!(tangent.sample(0.0, 1.0), PremulRGBA::zeroed());
    }

    #[test] fn conic_gradient_wraps_a_full_turn_from_its_start_angle() {
        let stops = red_blue_stops();
        let stops = GradientStops::new(&stops).unwrap();
        let conic = ConicGradient::new((2.0, 3.0), 0.0, stops).unwrap();
        assert_eq!(conic.sample(3.0, 3.0), RGBA::<u8>::red().premul());
        assert_eq!(conic.sample(2.0, 4.0), (191, 0, 64, 255).into());
        assert_eq!(conic.sample(1.0, 3.0), (128, 0, 128, 255).into());
        assert_eq!(conic.sample(2.0, 2.0), (64, 0, 191, 255).into());

        let rotated = ConicGradient::new((2.0, 3.0), TAU / 4.0, stops).unwrap();
        assert_eq!(rotated.sample(2.0, 4.0), RGBA::<u8>::red().premul());
        assert_eq!(conic.sample(3.0, 3.0 + 1e-4), RGBA::<u8>::red().premul());
        assert_eq!(conic.sample(3.0, 3.0 - 1e-4), RGBA::<u8>::blue().premul());
    }

    #[test] fn transformed_paint_maps_device_coordinates_and_preserves_solid_fast_path() {
        let stops = red_blue_stops();
        let gradient = LinearGradient::new((0.0, 0.0), (2.0, 0.0),
            GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
        let transformed = TransformedPaint::new(&gradient,
            Affine::new(2.0, 0.0, 0.0, 1.0, 10.0, 0.0)).unwrap();
        assert_eq!(transformed.sample(10.0, 0.0), RGBA::<u8>::red().premul());
        assert_eq!(transformed.sample(12.0, 0.0), (128, 0, 128, 255).into());
        assert_eq!(transformed.sample(14.0, 0.0), RGBA::<u8>::blue().premul());

        let solid = TransformedPaint::new(SolidPaint::new(RGBA::green()),
            Affine::translate(5.0, 7.0)).unwrap();
        assert_eq!(solid.solid_color(), Some(RGBA::<u8>::green().premul()));
        assert_eq!(TransformedPaint::new(solid,
            Affine::new(1.0, 2.0, 2.0, 4.0, 0.0, 0.0)).unwrap_err(),
            PaintTransformError::NonInvertibleTransform);

        let radial = RadialGradient::new((0.0, 0.0), 2.0,
            GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
        let ellipse = TransformedPaint::new(radial,
            Affine::new(2.0, 0.0, 0.0, 1.0, 0.0, 0.0)).unwrap();
        assert_eq!(ellipse.sample(2.0, 0.0), ellipse.sample(0.0, 1.0));
    }

    #[test] fn randomized_gradient_samples_remain_valid_premultiplied_colors() {
        let stops = [GradientStop::new(0.0, RGBA::new(240, 20, 80, 32)),
                     GradientStop::new(0.3, RGBA::new(10, 220, 40, 160)),
                     GradientStop::new(1.0, RGBA::new(30, 60, 250, 224))];
        let stops = GradientStops::new(&stops).unwrap();
        let linear = LinearGradient::new((-2.0, 1.0), (3.0, 4.0),
            stops, SpreadMode::Reflect).unwrap();
        let radial = RadialGradient::two_circle((0.5, -0.5), 0.25, (1.0, 1.5), 4.0,
            stops, SpreadMode::Repeat).unwrap();
        let conic = ConicGradient::new((-1.0, 2.0), 0.37, stops).unwrap();
        let mut state = 0xA341_316C_u32;
        for _ in 0..2048 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = (state >> 8) as f32 / 0x00FF_FFFF_u32 as f32 * 40.0 - 20.0;
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let y = (state >> 8) as f32 / 0x00FF_FFFF_u32 as f32 * 40.0 - 20.0;
            for color in [linear.sample(x, y), radial.sample(x, y), conic.sample(x, y)] {
                let [r, g, b, a] = color.to_array();
                assert!(r <= a && g <= a && b <= a);
            }
        }
    }
}
