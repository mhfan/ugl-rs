
//! Allocation-free paint samplers.
//!
//! Sampling currently uses device-space `f32` pixel centers as the reference
//! implementation. Gradient stops are decoded to linear-light premultiplied
//! colors and interpolated there. The compatibility sampler encodes at its
//! RGBA8 boundary; the linear sampler retains `f32` through compositing.

/*  Samplers can be though of as 2D shaders. Sampler is a first class citizen in *ugl-rs*,
    think of them as an object, that can be sampled in the normalized unit square.

    Sampler can be anything, that can be sampled such as:
      a fixed color
      a gradient (linear/radial/conic)
      a texture (image)
 */

use crate::{color::{EncodedPremulSRGBA8, LinearPremulRGBA, SRGBA, RGBA},
    geometry::{Affine, Point}};

/// Produces explicitly encoded premultiplied sRGB at device-space positions.
///
/// Implementations should be small values borrowed by the compositor. Calls are
/// statically dispatched; no trait object or allocation is required.
pub trait PaintSampler {
    fn sample(&self, x: f32, y: f32) -> EncodedPremulSRGBA8;

    /// Reports a position-independent color to enable span and tile fast paths.
    fn solid_color(&self) -> Option<EncodedPremulSRGBA8> { None }
}

/// Produces premultiplied linear-light colors without an encoded round trip.
///
/// This separate trait makes the working color space explicit. Implementing
/// [`PaintSampler`] alone does not opt a sampler into linear compositing.
pub trait LinearPaintSampler {
    fn sample_linear(&self, x: f32, y: f32) -> LinearPremulRGBA<f32>;
    fn solid_color_linear(&self) -> Option<LinearPremulRGBA<f32>> { None }

    /// Samples an affine sequence without requiring caller-owned scratch.
    ///
    /// Implementations must call `emit` exactly `len` times, in order.
    fn sample_linear_span(&self, x: f32, y: f32, dx: f32, dy: f32, len: u32,
        mut emit: impl FnMut(LinearPremulRGBA<f32>)) {
        for offset in 0..len {
            emit(self.sample_linear(x + offset as f32 * dx, y + offset as f32 * dy));
        }
    }
}

impl<S: PaintSampler + ?Sized> PaintSampler for &S {
    fn sample(&self, x: f32, y: f32) -> EncodedPremulSRGBA8 { (**self).sample(x, y) }
    fn solid_color(&self) -> Option<EncodedPremulSRGBA8> { (**self).solid_color() }
}

impl<S: LinearPaintSampler + ?Sized> LinearPaintSampler for &S {
    fn sample_linear(&self, x: f32, y: f32) -> LinearPremulRGBA<f32> {
        (**self).sample_linear(x, y)
    }
    fn solid_color_linear(&self) -> Option<LinearPremulRGBA<f32>> {
        (**self).solid_color_linear()
    }
    fn sample_linear_span(&self, x: f32, y: f32, dx: f32, dy: f32, len: u32,
        emit: impl FnMut(LinearPremulRGBA<f32>)) {
        (**self).sample_linear_span(x, y, dx, dy, len, emit)
    }
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
    fn sample(&self, x: f32, y: f32) -> EncodedPremulSRGBA8 {
        let point = self.device_to_paint.transform_point((x, y).into());
        self.sampler.sample(point.x, point.y)
    }

    fn solid_color(&self) -> Option<EncodedPremulSRGBA8> { self.sampler.solid_color() }
}

impl<S: LinearPaintSampler> LinearPaintSampler for TransformedPaint<S> {
    fn sample_linear(&self, x: f32, y: f32) -> LinearPremulRGBA<f32> {
        let point = self.device_to_paint.transform_point((x, y).into());
        self.sampler.sample_linear(point.x, point.y)
    }

    fn solid_color_linear(&self) -> Option<LinearPremulRGBA<f32>> {
        self.sampler.solid_color_linear()
    }

    fn sample_linear_span(&self, x: f32, y: f32, dx: f32, dy: f32, len: u32,
        emit: impl FnMut(LinearPremulRGBA<f32>)) {
        let start = self.device_to_paint.transform_point((x, y).into());
        let step = self.device_to_paint.transform_vector((dx, dy).into());
        self.sampler.sample_linear_span(
            start.x, start.y, step.x, step.y, len, emit);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolidPaint {
    encoded: EncodedPremulSRGBA8, linear: LinearPremulRGBA<f32>,
}

impl SolidPaint {
    pub fn new(color: RGBA<u8>) -> Self { Self::from_srgba(color.into()) }
    pub fn from_srgba(color: SRGBA<u8>) -> Self {
        Self { encoded: color.premul_encoded(), linear: color.to_linear().premul() }
    }
    pub fn premultiplied(color: EncodedPremulSRGBA8) -> Self {
        Self { encoded: color, linear: color.to_linear() }
    }
    pub fn color(&self) -> EncodedPremulSRGBA8 { self.encoded }
    pub fn linear_color(&self) -> LinearPremulRGBA<f32> { self.linear }
}

impl From<RGBA<u8>> for SolidPaint { fn from(color: RGBA<u8>) -> Self { Self::new(color) } }
impl From<SRGBA<u8>> for SolidPaint { fn from(color: SRGBA<u8>) -> Self { Self::from_srgba(color) } }

impl PaintSampler for SolidPaint {
    fn sample(&self, _x: f32, _y: f32) -> EncodedPremulSRGBA8 { self.encoded }
    fn solid_color(&self) -> Option<EncodedPremulSRGBA8> { Some(self.encoded) }
}

impl LinearPaintSampler for SolidPaint {
    fn sample_linear(&self, _x: f32, _y: f32) -> LinearPremulRGBA<f32> { self.linear }
    fn solid_color_linear(&self) -> Option<LinearPremulRGBA<f32>> { Some(self.linear) }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop { offset: f32, color: LinearPremulRGBA<f32> }

impl GradientStop {
    pub fn new(offset: f32, color: RGBA<u8>) -> Self {
        Self::from_srgba(offset, color.into())
    }

    pub fn from_srgba(offset: f32, color: SRGBA<u8>) -> Self {
        Self { offset, color: color.to_linear().premul() }
    }

    pub fn offset(&self) -> f32 { self.offset }
    pub fn color(&self) -> LinearPremulRGBA<f32> { self.color }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum GradientError {
    EmptyStops, NonFiniteOffset, OffsetOutOfRange, UnorderedStops,
    RampTooSmall, NonFiniteGeometry, NegativeRadius, DegenerateGeometry,
}

/// Validated, caller-owned gradient stops.
#[derive(Clone, Copy, Debug)] pub struct GradientStops<'a> {
    stops: &'a [GradientStop],
    encoded_ramp: Option<&'a [EncodedPremulSRGBA8]>,
    linear_ramp: Option<&'a [LinearPremulRGBA<f32>]>,
}

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
        }   Ok(Self { stops, encoded_ramp: None, linear_ramp: None })
    }

    /// Builds an encoded lookup ramp once for the high-throughput sampling path.
    ///
    /// This approximates the exact linear-light interpolation performed by
    /// [`Self::new`]. Smooth gradients converge with ramp resolution; repeated
    /// stops used for hard transitions are quantized to one ramp interval.
    pub fn with_ramp(stops: &'a [GradientStop],
        ramp: &'a mut [EncodedPremulSRGBA8]) -> Result<Self, GradientError> {
        Self::new(stops)?;
        if ramp.len() < 2 { return Err(GradientError::RampTooSmall); }
        let scale = (ramp.len() - 1) as f32;
        for (index, color) in ramp.iter_mut().enumerate() {
            *color = Self::sample_stops(stops, index as f32 / scale);
        }
        Ok(Self { stops, encoded_ramp: Some(ramp), linear_ramp: None })
    }

    /// Builds a premultiplied linear-light lookup ramp for linear framebuffers.
    ///
    /// Each entry occupies 16 bytes. This avoids both stop lookup and transfer
    /// conversion while retaining a fully linear sampling and compositing path.
    pub fn with_linear_ramp(stops: &'a [GradientStop],
        ramp: &'a mut [LinearPremulRGBA<f32>]) -> Result<Self, GradientError> {
        Self::new(stops)?;
        if ramp.len() < 2 { return Err(GradientError::RampTooSmall); }
        let scale = (ramp.len() - 1) as f32;
        for (index, color) in ramp.iter_mut().enumerate() {
            *color = Self::sample_linear_stops(stops, index as f32 / scale);
        }
        Ok(Self { stops, encoded_ramp: None, linear_ramp: Some(ramp) })
    }

    pub fn as_slice(&self) -> &'a [GradientStop] { self.stops }

    fn sample(&self, t: f32) -> EncodedPremulSRGBA8 {
        let Some(ramp) = self.encoded_ramp else { return Self::sample_stops(self.stops, t); };
        let index = (t.clamp(0.0, 1.0) * (ramp.len() - 1) as f32 + 0.5) as usize;
        ramp[index]
    }

    fn sample_stops(stops: &[GradientStop], t: f32) -> EncodedPremulSRGBA8 {
        Self::sample_linear_stops(stops, t).to_encoded_srgba8()
    }

    fn sample_linear(&self, t: f32) -> LinearPremulRGBA<f32> {
        let Some(ramp) = self.linear_ramp else {
            return Self::sample_linear_stops(self.stops, t);
        };
        let index = (t.clamp(0.0, 1.0) * (ramp.len() - 1) as f32 + 0.5) as usize;
        ramp[index]
    }

    fn sample_linear_stops(stops: &[GradientStop], t: f32) -> LinearPremulRGBA<f32> {
        let upper = stops.partition_point(|stop| stop.offset <= t);
        if  upper == 0 { return stops[0].color; }
        if  upper == stops.len() { return stops[upper - 1].color; }
        let (from, to) = (stops[upper - 1], stops[upper]);
        let extent = to.offset - from.offset;
        if  extent == 0.0 { return to.color; }
        let position = (t - from.offset) / extent;
        from.color.lerp(to.color, position)
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
    fn sample(&self, x: f32, y: f32) -> EncodedPremulSRGBA8 {
        let t = ((x - self.from.x) * self.delta.x  +
                 (y - self.from.y) * self.delta.y) * self.inverse_length_squared;
        self.stops.sample(self.spread.map(t))
    }
}

impl LinearPaintSampler for LinearGradient<'_> {
    fn sample_linear(&self, x: f32, y: f32) -> LinearPremulRGBA<f32> {
        let t = ((x - self.from.x) * self.delta.x +
                 (y - self.from.y) * self.delta.y) * self.inverse_length_squared;
        self.stops.sample_linear(self.spread.map(t))
    }

    fn sample_linear_span(&self, x: f32, y: f32, dx: f32, dy: f32, len: u32,
        mut emit: impl FnMut(LinearPremulRGBA<f32>)) {
        let start = ((x - self.from.x) * self.delta.x +
                     (y - self.from.y) * self.delta.y) * self.inverse_length_squared;
        let step = (dx * self.delta.x + dy * self.delta.y) * self.inverse_length_squared;
        for offset in 0..len {
            emit(self.stops.sample_linear(
                self.spread.map(start + offset as f32 * step)));
        }
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
        if self.is_concentric() {
            return self.concentric_parameter(point.x * point.x + point.y * point.y);
        }
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

    fn is_concentric(&self) -> bool {
        self.center_delta.x == 0.0 && self.center_delta.y == 0.0
    }

    fn concentric_parameter(&self, distance_squared: f32) -> Option<f32> {
        if !distance_squared.is_finite() { return None; }
        let distance = libm::sqrtf(distance_squared.max(0.0));
        let parameter = (distance - self.start_radius) / self.radius_delta;
        parameter.is_finite().then_some(parameter)
    }
}

impl PaintSampler for RadialGradient<'_> {
    fn sample(&self, x: f32, y: f32) -> EncodedPremulSRGBA8 {
        self.parameter(x, y).map_or_else(EncodedPremulSRGBA8::zeroed,
            |t| self.stops.sample(self.spread.map(t)))
    }
}

impl LinearPaintSampler for RadialGradient<'_> {
    fn sample_linear(&self, x: f32, y: f32) -> LinearPremulRGBA<f32> {
        self.parameter(x, y).map_or_else(LinearPremulRGBA::default,
            |t| self.stops.sample_linear(self.spread.map(t)))
    }

    fn sample_linear_span(&self, x: f32, y: f32, dx: f32, dy: f32, len: u32,
        mut emit: impl FnMut(LinearPremulRGBA<f32>)) {
        if !self.is_concentric() {
            for offset in 0..len {
                emit(self.sample_linear(x + offset as f32 * dx, y + offset as f32 * dy));
            }   return;
        }
        let (x, y) = (x - self.start.x, y - self.start.y);
        let step_squared = dx * dx + dy * dy;
        let (mut distance_squared, mut distance_step) = (
            x * x + y * y,
            2.0 * (x * dx + y * dy) + step_squared,
        );
        let second_difference = 2.0 * step_squared;
        for _ in 0..len {
            emit(self.concentric_parameter(distance_squared).map_or_else(
                LinearPremulRGBA::default,
                |t| self.stops.sample_linear(self.spread.map(t))));
            distance_squared += distance_step;
            distance_step += second_difference;
        }
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
    fn sample(&self, x: f32, y: f32) -> EncodedPremulSRGBA8 {
        let turn =  libm::atan2f(y - self.center.y, x - self.center.x) / TAU;
        self.stops.sample(SpreadMode::Repeat.map(turn - self.start_turn))
    }
}

impl LinearPaintSampler for ConicGradient<'_> {
    fn sample_linear(&self, x: f32, y: f32) -> LinearPremulRGBA<f32> {
        let turn = libm::atan2f(y - self.center.y, x - self.center.x) / TAU;
        self.stops.sample_linear(SpreadMode::Repeat.map(turn - self.start_turn))
    }
}

#[cfg(test)] mod tests { use super::*;
    fn encoded(color: RGBA<u8>) -> EncodedPremulSRGBA8 {
        SRGBA::from(color).premul_encoded()
    }

    fn linear(r: f32, g: f32, b: f32, a: f32) -> EncodedPremulSRGBA8 {
        crate::color::LinearRGBA::new(r, g, b, a).premul().to_encoded_srgba8()
    }

    #[test] fn solid_paint_is_position_independent_and_premultiplied() {
        let paint = SolidPaint::new(RGBA::new(200, 100, 50, 128));
        assert_eq!(paint.sample(0.5, 0.5),
            EncodedPremulSRGBA8::new(100, 50, 25, 128).unwrap());
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
        assert_eq!(GradientStops::new(&stops).unwrap().sample(0.5),
            EncodedPremulSRGBA8::new(0, 0, 128, 128).unwrap());

        let single = [GradientStop::new(0.4, RGBA::green())];
        let single = GradientStops::new(&single).unwrap();
        assert_eq!(single.sample(-10.0), single.sample(10.0));
        let hard = [GradientStop::new(0.0, RGBA::red()),
                    GradientStop::new(0.5, RGBA::red()),
                    GradientStop::new(0.5, RGBA::blue()),
                    GradientStop::new(1.0, RGBA::blue())];
        let hard = GradientStops::new(&hard).unwrap();
        assert_eq!(hard.sample(0.5 - f32::EPSILON), encoded(RGBA::<u8>::red()));
        assert_eq!(hard.sample(0.5), encoded(RGBA::<u8>::blue()));
    }

    #[test] fn gradient_ramp_validates_storage_and_tracks_exact_sampling() {
        let stops = red_blue_stops();
        let mut too_small = [EncodedPremulSRGBA8::zeroed(); 1];
        assert_eq!(GradientStops::with_ramp(&stops, &mut too_small).unwrap_err(),
            GradientError::RampTooSmall);

        let exact = GradientStops::new(&stops).unwrap();
        let mut storage = [EncodedPremulSRGBA8::zeroed(); 1024];
        let ramp = GradientStops::with_ramp(&stops, &mut storage).unwrap();
        for step in 0..=256 {
            let t = step as f32 / 256.0;
            let (actual, expected) = (ramp.sample(t).to_array(), exact.sample(t).to_array());
            for channel in 0..4 {
                assert!(actual[channel].abs_diff(expected[channel]) <= 1,
                    "t={t}, actual={actual:?}, expected={expected:?}");
            }
        }
        for step in 0..=256 {
            let t = step as f32 / 256.0;
            assert_eq!(ramp.sample_linear(t), exact.sample_linear(t));
        }

        let mut linear_storage = [LinearPremulRGBA::default(); 1024];
        let ramp = GradientStops::with_linear_ramp(&stops, &mut linear_storage).unwrap();
        for step in 0..=256 {
            let t = step as f32 / 256.0;
            let (actual, expected) =
                (ramp.sample_linear(t).to_array(), exact.sample_linear(t).to_array());
            for channel in 0..4 {
                assert!((actual[channel] - expected[channel]).abs() <= 1.0 / 1023.0,
                    "t={t}, actual={actual:?}, expected={expected:?}");
            }
            assert_eq!(ramp.sample(t), exact.sample(t));
        }
    }

    #[test] fn linear_gradient_projects_device_coordinates_and_spreads() {
        let stops = red_blue_stops();
        let stops = GradientStops::new(&stops).unwrap();
        let pad = LinearGradient::new((1.0, 2.0), (5.0, 2.0),
            stops, SpreadMode::Pad).unwrap();
        assert_eq!(pad.sample(1.0, 100.0), encoded(RGBA::<u8>::red()));
        assert_eq!(pad.sample(3.0, -100.0), linear(0.5, 0.0, 0.5, 1.0));
        assert_eq!(pad.sample(8.0, 2.0), encoded(RGBA::<u8>::blue()));

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
        assert_eq!(radial.sample(2.0, 3.0), encoded(RGBA::<u8>::red()));
        assert_eq!(radial.sample(4.0, 3.0), linear(0.5, 0.0, 0.5, 1.0));
        assert_eq!(radial.sample(10.0, 3.0), encoded(RGBA::<u8>::blue()));

        let focal = RadialGradient::two_circle((1.0, 0.0), 0.0, (0.0, 0.0), 4.0,
            stops, SpreadMode::Pad).unwrap();
        assert_eq!(focal.sample( 1.0, 0.0), encoded(RGBA::<u8>:: red()));
        assert_eq!(focal.sample(-4.0, 0.0), encoded(RGBA::<u8>::blue()));
        assert_eq!(RadialGradient::new((0.0, 0.0), -1.0, stops, SpreadMode::Pad)
            .unwrap_err(), GradientError::NegativeRadius);
        assert_eq!(RadialGradient::two_circle((0.0, 0.0), 1.0, (0.0, 0.0), 1.0,
            stops, SpreadMode::Pad).unwrap_err(), GradientError::DegenerateGeometry);

        let tangent = RadialGradient::two_circle((0.0, 0.0), 0.0, (1.0, 0.0), 1.0,
            stops, SpreadMode::Pad).unwrap();
        assert_eq!(tangent.sample(0.5, 0.0), linear(0.75, 0.0, 0.25, 1.0));
        assert_eq!(tangent.sample(0.0, 1.0), EncodedPremulSRGBA8::zeroed());
    }

    #[test] fn conic_gradient_wraps_a_full_turn_from_its_start_angle() {
        let stops = red_blue_stops();
        let stops = GradientStops::new(&stops).unwrap();
        let conic = ConicGradient::new((2.0, 3.0), 0.0, stops).unwrap();
        assert_eq!(conic.sample(3.0, 3.0), encoded(RGBA::<u8>::red()));
        assert_eq!(conic.sample(2.0, 4.0), linear(0.75, 0.0, 0.25, 1.0));
        assert_eq!(conic.sample(1.0, 3.0), linear(0.5, 0.0, 0.5, 1.0));
        assert_eq!(conic.sample(2.0, 2.0), linear(0.25, 0.0, 0.75, 1.0));

        let rotated = ConicGradient::new((2.0, 3.0), TAU / 4.0, stops).unwrap();
        assert_eq!(rotated.sample(2.0, 4.0), encoded(RGBA::<u8>::red()));
        assert_eq!(conic.sample(3.0, 3.0 + 1e-4), encoded(RGBA::<u8>::red()));
        assert_eq!(conic.sample(3.0, 3.0 - 1e-4), encoded(RGBA::<u8>::blue()));
    }

    #[test] fn transformed_paint_maps_device_coordinates_and_preserves_solid_fast_path() {
        let stops = red_blue_stops();
        let gradient = LinearGradient::new((0.0, 0.0), (2.0, 0.0),
            GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
        let transformed = TransformedPaint::new(&gradient,
            Affine::new(2.0, 0.0, 0.0, 1.0, 10.0, 0.0)).unwrap();
        assert_eq!(transformed.sample(10.0, 0.0), encoded(RGBA::<u8>::red()));
        assert_eq!(transformed.sample(12.0, 0.0), linear(0.5, 0.0, 0.5, 1.0));
        assert_eq!(transformed.sample(14.0, 0.0), encoded(RGBA::<u8>::blue()));

        let solid = TransformedPaint::new(SolidPaint::new(RGBA::green()),
            Affine::translate(5.0, 7.0)).unwrap();
        assert_eq!(solid.solid_color(), Some(encoded(RGBA::<u8>::green())));
        assert_eq!(TransformedPaint::new(solid,
            Affine::new(1.0, 2.0, 2.0, 4.0, 0.0, 0.0)).unwrap_err(),
            PaintTransformError::NonInvertibleTransform);

        let radial = RadialGradient::new((0.0, 0.0), 2.0,
            GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
        let ellipse = TransformedPaint::new(radial,
            Affine::new(2.0, 0.0, 0.0, 1.0, 0.0, 0.0)).unwrap();
        assert_eq!(ellipse.sample(2.0, 0.0), ellipse.sample(0.0, 1.0));
    }

    #[test] fn specialized_gradient_span_stepping_matches_point_sampling() {
        fn assert_span<S: LinearPaintSampler>(sampler: &S, start: Point, step: Point) {
            let mut actual = [LinearPremulRGBA::default(); 512];
            let mut count = 0;
            sampler.sample_linear_span(start.x, start.y, step.x, step.y, actual.len() as _,
                |color| { actual[count] = color; count += 1; });
            assert_eq!(count, actual.len());
            for (offset, actual) in actual.into_iter().enumerate() {
                let expected = sampler.sample_linear(
                    start.x + offset as f32 * step.x,
                    start.y + offset as f32 * step.y);
                let (actual, expected) = (actual.to_array(), expected.to_array());
                for channel in 0..4 {
                    assert!((actual[channel] - expected[channel]).abs() <= 1e-4,
                        "offset={offset}, actual={actual:?}, expected={expected:?}");
                }
            }
        }

        let stops = red_blue_stops();
        let stops = GradientStops::new(&stops).unwrap();
        for spread in [SpreadMode::Pad, SpreadMode::Repeat, SpreadMode::Reflect] {
            let gradient = LinearGradient::new((-2.0, 1.0), (5.0, 4.0), stops, spread).unwrap();
            assert_span(&gradient, (-3.25, 2.75).into(), (0.5, -0.125).into());
            let transformed = TransformedPaint::new(gradient,
                Affine::new(1.5, 0.25, -0.5, 2.0, 3.0, -4.0)).unwrap();
            assert_span(&transformed, (-3.25, 2.75).into(), (0.5, -0.125).into());

            let radial = RadialGradient::two_circle(
                (1.0, -2.0), 0.5, (1.0, -2.0), 6.0, stops, spread).unwrap();
            assert_span(&radial, (1.0, -2.0).into(), (0.25, 0.125).into());
            assert_span(&radial, (-4.5, -2.0).into(), (0.5, 0.0).into());
            let transformed = TransformedPaint::new(radial,
                Affine::new(1.5, 0.25, -0.5, 2.0, 3.0, -4.0)).unwrap();
            assert_span(&transformed, (-3.25, 2.75).into(), (0.5, -0.125).into());
        }
    }

    #[test] fn exact_gradient_samplers_encode_only_at_the_compatibility_boundary() {
        fn assert_boundary<S: PaintSampler + LinearPaintSampler>(sampler: &S,
            points: &[(f32, f32)]) {
            for &(x, y) in points {
                assert_eq!(sampler.sample(x, y),
                    sampler.sample_linear(x, y).to_encoded_srgba8(), "point=({x}, {y})");
            }
        }

        let stops = [GradientStop::from_srgba(0.0, SRGBA::new(240, 20, 80, 32)),
                     GradientStop::from_srgba(0.35, SRGBA::new(10, 220, 40, 160)),
                     GradientStop::from_srgba(1.0, SRGBA::new(30, 60, 250, 224))];
        let stops = GradientStops::new(&stops).unwrap();
        let points = [(-4.25, -2.5), (-0.25, 0.75), (0.5, 0.5), (2.25, 3.75), (8.0, 4.0)];
        for spread in [SpreadMode::Pad, SpreadMode::Repeat, SpreadMode::Reflect] {
            let linear = LinearGradient::new((-1.0, 0.5), (4.0, 3.0), stops, spread).unwrap();
            let radial = RadialGradient::two_circle((0.5, -0.5), 0.25,
                (1.0, 1.5), 4.0, stops, spread).unwrap();
            assert_boundary(&linear, &points);
            assert_boundary(&radial, &points);
        }
        let conic = ConicGradient::new((-1.0, 2.0), 0.37, stops).unwrap();
        assert_boundary(&conic, &points);
        let transformed = TransformedPaint::new(conic,
            Affine::new(1.5, 0.25, -0.5, 2.0, 3.0, -4.0)).unwrap();
        assert_boundary(&transformed, &points);
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
