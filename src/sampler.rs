
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

use crate::{color::{PRGB32, RGBA}, geometry::Point};

/// Produces premultiplied source colors at device-space positions.
///
/// Implementations should be small values borrowed by the compositor. Calls are
/// statically dispatched; no trait object or allocation is required.
pub trait PaintSampler {
    fn sample(&self, x: f32, y: f32) -> PRGB32<u8>;

    /// Reports a position-independent color to enable span and tile fast paths.
    fn solid_color(&self) -> Option<PRGB32<u8>> { None }
}

#[derive(Clone, Copy, Debug, PartialEq)] pub struct SolidPaint { color: PRGB32<u8> }

impl SolidPaint {
    pub fn new(color: RGBA<u8>) -> Self { Self { color: color.premul() } }
    pub fn premultiplied(color: PRGB32<u8>) -> Self { Self { color } }
    pub fn color(&self) -> PRGB32<u8> { self.color }
}

impl From<RGBA<u8>> for SolidPaint { fn from(color: RGBA<u8>) -> Self { Self::new(color) } }

impl From<PRGB32<u8>> for SolidPaint {
    fn from(color: PRGB32<u8>) -> Self { Self::premultiplied(color) }
}

impl PaintSampler for SolidPaint {
    fn sample(&self, _x: f32, _y: f32) -> PRGB32<u8> { self.color }
    fn solid_color(&self) -> Option<PRGB32<u8>> { Some(self.color) }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop { offset: f32, color: PRGB32<u8> }

impl GradientStop {
    pub fn new(offset: f32, color: RGBA<u8>) -> Self {
        Self { offset, color: color.premul() }
    }

    pub fn premultiplied(offset: f32, color: PRGB32<u8>) -> Self { Self { offset, color } }

    pub fn offset(&self) -> f32 { self.offset }
    pub fn color(&self) -> PRGB32<u8> { self.color }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum GradientError {
    EmptyStops, NonFiniteOffset, OffsetOutOfRange, UnorderedStops,
    NonFiniteGeometry, DegenerateGeometry,
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

    fn sample(&self, t: f32) -> PRGB32<u8> {
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
    fn sample(&self, x: f32, y: f32) -> PRGB32<u8> {
        let t = ((x - self.from.x) * self.delta.x  +
                 (y - self.from.y) * self.delta.y) * self.inverse_length_squared;
        self.stops.sample(self.spread.map(t))
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
        assert_eq!(GradientStops::new(&[GradientStop::new(0.75, RGBA::red()),
                                        GradientStop::new(0.25, RGBA::blue()),
        ]).unwrap_err(), GradientError::UnorderedStops);

        let stops = [GradientStop::new(0.0, RGBA::new(255, 0, 0, 0)),
                     GradientStop::new(1.0, RGBA::new(0, 0, 255, 255))];
        assert_eq!(GradientStops::new(&stops).unwrap().sample(0.5), (0, 0, 128, 128).into());
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
}
