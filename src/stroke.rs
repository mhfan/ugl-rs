//! Stroke expansion options and scalar reference implementation.

use crate::{edge::{Edge, EdgeSink}, geometry::Point};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LineCap { #[default] Butt, Round, Square, }

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LineJoin { #[default] Miter, Round, Bevel, }

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum StrokeError {
    NonFiniteWidth, NonPositiveWidth, NonFiniteMiterLimit, MiterLimitTooSmall,
    NonFiniteTolerance, NonPositiveTolerance, ArcSegmentLimitZero,
}

/// Validated device-space stroke parameters.
#[derive(Clone, Copy, Debug, PartialEq)] pub struct StrokeOptions {
    width: f32, miter_limit: f32, tolerance: f32, max_arc_segments: u16,
    cap: LineCap, join: LineJoin,
}

impl StrokeOptions {
    pub fn new(width: f32) -> Result<Self, StrokeError> {
        if !width.is_finite() { return Err(StrokeError::NonFiniteWidth); }
        if  width <= 0.0 { return Err(StrokeError::NonPositiveWidth); }
        Ok(Self { width, ..Self::default() })
    }

    pub fn with_cap(mut self, cap: LineCap) -> Self { self.cap = cap; self }
    pub fn with_join(mut self, join: LineJoin) -> Self { self.join = join; self }

    pub fn with_miter_limit(mut self, miter_limit: f32) -> Result<Self, StrokeError> {
        if !miter_limit.is_finite() { return Err(StrokeError::NonFiniteMiterLimit); }
        if miter_limit < 1.0 { return Err(StrokeError::MiterLimitTooSmall); }
        self.miter_limit = miter_limit;   Ok(self)
    }

    pub fn with_tolerance(mut self, tolerance: f32) -> Result<Self, StrokeError> {
        if !tolerance.is_finite() { return Err(StrokeError::NonFiniteTolerance); }
        if tolerance <= 0.0 { return Err(StrokeError::NonPositiveTolerance); }
        self.tolerance = tolerance;   Ok(self)
    }

    pub fn with_max_arc_segments(mut self, maximum: u16) -> Result<Self, StrokeError> {
        if maximum == 0 { return Err(StrokeError::ArcSegmentLimitZero); }
        self.max_arc_segments = maximum;   Ok(self)
    }

    pub fn width(&self) -> f32 { self.width }
    pub fn half_width(&self) -> f32 { self.width * 0.5 }
    pub fn miter_limit(&self) -> f32 { self.miter_limit }
    pub fn tolerance(&self) -> f32 { self.tolerance }
    pub fn max_arc_segments(&self) -> u16 { self.max_arc_segments }
    pub fn cap(&self) -> LineCap { self.cap }
    pub fn join(&self) -> LineJoin { self.join }
}

impl Default for StrokeOptions {
    fn default() -> Self {
        Self { width: 1.0, miter_limit: 4.0, tolerance: 0.25, max_arc_segments: 64,
               cap: LineCap::Butt, join: LineJoin::Miter }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)] pub enum StrokeExpandError<E> {
    NonFinitePoint, ArcSegmentLimit { needed: usize, maximum: u16 }, Sink(E),
}

/// Expands one line into a consistently wound closed fill contour.
pub fn stroke_line<S: EdgeSink>(from: Point, to: Point, options: StrokeOptions,
    sink: &mut S) -> Result<(), StrokeExpandError<S::Error>> {
    if !point_is_finite(from) || !point_is_finite(to) {
        return Err(StrokeExpandError::NonFinitePoint);
    }
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = libm::sqrtf(dx * dx + dy * dy);
    if !length.is_finite() { return Err(StrokeExpandError::NonFinitePoint); }
    if length == 0.0 { return stroke_point(from, options, sink); }
    let (ux, uy, radius) = (dx / length, dy / length, options.half_width());
    let normal: Point = (-uy * radius, ux * radius).into();
    let extension = if options.cap == LineCap::Square { radius } else { 0.0 };
    let (start, end): (Point, Point) = (
        (from.x - ux * extension, from.y - uy * extension).into(),
        (to.x + ux * extension, to.y + uy * extension).into(),
    );
    if options.cap != LineCap::Round {
        return emit_polygon(&[
            (start.x + normal.x, start.y + normal.y).into(),
            (start.x - normal.x, start.y - normal.y).into(),
            (end.x - normal.x, end.y - normal.y).into(),
            (end.x + normal.x, end.y + normal.y).into(),
        ], sink);
    }

    let segments = arc_segments(radius, options)
        .map_err(|(needed, maximum)| StrokeExpandError::ArcSegmentLimit { needed, maximum })?;
    let angle = libm::atan2f(uy, ux);
    let mut contour = EdgeContour::new(sink);
    contour.point((from.x + normal.x, from.y + normal.y).into())?;
    contour.arc(from, radius, angle + core::f32::consts::FRAC_PI_2,
        core::f32::consts::PI, segments)?;
    contour.point((to.x - normal.x, to.y - normal.y).into())?;
    contour.arc(to, radius, angle - core::f32::consts::FRAC_PI_2,
        core::f32::consts::PI, segments)?;
    contour.close()
}

/// Applies the documented cap behavior to a point-only contour.
pub fn stroke_point<S: EdgeSink>(point: Point, options: StrokeOptions, sink: &mut S) ->
    Result<(), StrokeExpandError<S::Error>> {
    if !point_is_finite(point) { return Err(StrokeExpandError::NonFinitePoint); }
    let radius = options.half_width();
    match options.cap {
        LineCap::Butt => Ok(()),
        LineCap::Square => emit_polygon(&[
            (point.x - radius, point.y - radius).into(),
            (point.x + radius, point.y - radius).into(),
            (point.x + radius, point.y + radius).into(),
            (point.x - radius, point.y + radius).into(),
        ], sink),
        LineCap::Round => {
            let segments = arc_segments(radius, options)
                .map_err(|(needed, maximum)|
                    StrokeExpandError::ArcSegmentLimit { needed, maximum })? * 2;
            let mut contour = EdgeContour::new(sink);
            contour.point((point.x + radius, point.y).into())?;
            contour.arc(point, radius, 0.0, core::f32::consts::PI * 2.0, segments)?;
            contour.close()
        }
    }
}

fn point_is_finite(point: Point) -> bool { point.x.is_finite() && point.y.is_finite() }

fn arc_segments(radius: f32, options: StrokeOptions) -> Result<usize, (usize, u16)> {
    let tolerance = options.tolerance().min(radius);
    let maximum_angle = 2.0 * libm::acosf((1.0 - tolerance / radius).clamp(-1.0, 1.0));
    let needed = libm::ceilf(core::f32::consts::PI / maximum_angle).max(2.0) as usize;
    if needed > options.max_arc_segments() as usize {
        Err((needed, options.max_arc_segments()))
    } else { Ok(needed) }
}

fn emit_polygon<S: EdgeSink>(points: &[Point], sink: &mut S) ->
    Result<(), StrokeExpandError<S::Error>> {
    let mut contour = EdgeContour::new(sink);
    for point in points { contour.point(*point)?; }
    contour.close()
}

struct EdgeContour<'a, S> {
    sink: &'a mut S, first: Option<Point>, previous: Option<Point>,
}

impl<'a, S> EdgeContour<'a, S> {
    fn new(sink: &'a mut S) -> Self { Self { sink, first: None, previous: None } }
}

impl<S: EdgeSink> EdgeContour<'_, S> {
    fn point(&mut self, point: Point) -> Result<(), StrokeExpandError<S::Error>> {
        if let Some(previous) = self.previous {
            if let Some(edge) = Edge::from_line(previous, point) {
                self.sink.edge(edge).map_err(StrokeExpandError::Sink)?;
            }
        } else { self.first = Some(point); }
        self.previous = Some(point);   Ok(())
    }

    fn arc(&mut self, center: Point, radius: f32, start: f32, sweep: f32,
        segments: usize) -> Result<(), StrokeExpandError<S::Error>> {
        for index in 1..=segments {
            let angle = start + sweep * index as f32 / segments as f32;
            self.point((center.x + radius * libm::cosf(angle),
                        center.y + radius * libm::sinf(angle)).into())?;
        }   Ok(())
    }

    fn close(self) -> Result<(), StrokeExpandError<S::Error>> {
        if let (Some(previous), Some(first)) = (self.previous, self.first)
            && let Some(edge) = Edge::from_line(previous, first) {
            self.sink.edge(edge).map_err(StrokeExpandError::Sink)?;
        }   Ok(())
    }
}

#[cfg(test)] mod tests { use super::*;
    use alloc::vec::Vec;
    use core::convert::Infallible;

    fn collect_line(from: impl Into<Point>, to: impl Into<Point>, cap: LineCap) -> Vec<Edge> {
        let mut edges = Vec::new();
        stroke_line(from.into(), to.into(), StrokeOptions::new(2.0).unwrap().with_cap(cap),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }).unwrap();
        edges
    }

    #[test] fn stroke_options_reject_invalid_geometric_states() {
        assert_eq!(StrokeOptions::new(0.0), Err(StrokeError::NonPositiveWidth));
        assert_eq!(StrokeOptions::new(f32::INFINITY), Err(StrokeError::NonFiniteWidth));
        assert_eq!(StrokeOptions::new(2.0).unwrap().with_miter_limit(0.5),
                   Err(StrokeError::MiterLimitTooSmall));
        assert_eq!(StrokeOptions::new(2.0).unwrap().with_miter_limit(f32::NAN),
                   Err(StrokeError::NonFiniteMiterLimit));
        assert_eq!(StrokeOptions::new(2.0).unwrap().with_tolerance(0.0),
                   Err(StrokeError::NonPositiveTolerance));
        assert_eq!(StrokeOptions::new(2.0).unwrap().with_max_arc_segments(0),
                   Err(StrokeError::ArcSegmentLimitZero));
    }

    #[test] fn stroke_options_use_device_space_defaults_and_builders() {
        let options = StrokeOptions::new(6.0).unwrap()
            .with_cap(LineCap::Round).with_join(LineJoin::Bevel)
            .with_miter_limit(8.0).unwrap().with_tolerance(0.125).unwrap()
            .with_max_arc_segments(32).unwrap();
        assert_eq!((options.width(), options.half_width(), options.miter_limit()),
                   (6.0, 3.0, 8.0));
        assert_eq!((options.tolerance(), options.max_arc_segments()), (0.125, 32));
        assert_eq!((options.cap(), options.join()), (LineCap::Round, LineJoin::Bevel));
    }

    #[test] fn line_caps_expand_to_expected_bounds_without_allocation() {
        let bounds = |edges: &[Edge]| edges.iter().flat_map(|edge|
            [edge.upper.x, edge.lower.x]).fold((f32::INFINITY, f32::NEG_INFINITY),
                |(minimum, maximum), x| (minimum.min(x), maximum.max(x)));
        assert_eq!(bounds(&collect_line((2.0, 3.0), (6.0, 3.0), LineCap::Butt)), (2.0, 6.0));
        assert_eq!(bounds(&collect_line((2.0, 3.0), (6.0, 3.0), LineCap::Square)), (1.0, 7.0));
        let (minimum, maximum) =
            bounds(&collect_line((2.0, 3.0), (6.0, 3.0), LineCap::Round));
        assert!(minimum > 1.0 && minimum - 1.0 <= StrokeOptions::default().tolerance());
        assert!(maximum < 7.0 && 7.0 - maximum <= StrokeOptions::default().tolerance());
    }

    #[test] fn point_only_contours_follow_cap_semantics_and_arc_limits() {
        let mut edges = Vec::new();
        stroke_point((4.0, 5.0).into(), StrokeOptions::new(2.0).unwrap(),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }).unwrap();
        assert!(edges.is_empty());
        stroke_point((4.0, 5.0).into(),
            StrokeOptions::new(2.0).unwrap().with_cap(LineCap::Square),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }).unwrap();
        assert_eq!(edges.len(), 2);

        let options = StrokeOptions::new(100.0).unwrap().with_cap(LineCap::Round)
            .with_tolerance(1e-4).unwrap().with_max_arc_segments(2).unwrap();
        assert!(matches!(stroke_point((0.0, 0.0).into(), options,
            &mut |_: Edge| Ok::<_, Infallible>(())),
            Err(StrokeExpandError::ArcSegmentLimit { maximum: 2, .. })));
    }

    #[test] fn invalid_geometry_and_sink_errors_are_explicit() {
        assert_eq!(stroke_line((f32::NAN, 0.0).into(), (1.0, 0.0).into(),
            StrokeOptions::default(), &mut |_| Ok::<_, &'static str>(())),
            Err(StrokeExpandError::NonFinitePoint));
        assert_eq!(stroke_line((0.0, 0.0).into(), (1.0, 1.0).into(),
            StrokeOptions::default(), &mut |_| Err("full")),
            Err(StrokeExpandError::Sink("full")));
    }
}
