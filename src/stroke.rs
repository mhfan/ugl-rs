//! Stroke expansion options and scalar reference implementation.

use core::f32::consts::{FRAC_PI_2, PI};
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
    width: f32, miter_limit: f32, cap: LineCap, join: LineJoin,
    tolerance: f32, max_arc_segments: u16,
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
        if  !miter_limit.is_finite() { return Err(StrokeError::NonFiniteMiterLimit); }
        if   miter_limit < 1.0       { return Err(StrokeError::MiterLimitTooSmall); }
        self.miter_limit = miter_limit;   Ok(self)
    }

    pub fn with_tolerance(mut self, tolerance: f32) -> Result<Self, StrokeError> {
        if  !tolerance.is_finite() { return Err(StrokeError::NonFiniteTolerance); }
        if   tolerance <= 0.0      { return Err(StrokeError::NonPositiveTolerance); }
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
    fn default() -> Self { Self {
            width: 1.0, miter_limit: 4.0, cap: LineCap::Butt, join: LineJoin::Miter,
            tolerance: 0.25, max_arc_segments: 64,
    } }
}

#[derive(Clone, Copy, Debug, PartialEq)] pub enum StrokeExpandError<E> {
    NonFinitePoint, ArcSegmentLimit { needed: usize, maximum: u16 }, Sink(E),
}

/// Expands one line into a consistently wound closed fill contour.
pub fn stroke_line<S: EdgeSink>(from: Point, to: Point, options: StrokeOptions,
    sink: &mut S) -> Result<(), StrokeExpandError<S::Error>> {
    stroke_polyline(&[from, to], false, options, sink)
}

/// Expands an open or closed polyline without allocating an intermediate path.
pub fn stroke_polyline<S: EdgeSink>(points: &[Point], closed: bool, options: StrokeOptions,
    sink: &mut S) -> Result<(), StrokeExpandError<S::Error>> {
    if points.iter().any(|point| !point_is_finite(*point)) {
        return Err(StrokeExpandError::NonFinitePoint);
    }
    if (options.cap == LineCap::Round || options.join == LineJoin::Round) && !points.is_empty() {
        arc_segments(options.half_width(), options).map_err(|(needed, maximum)|
            StrokeExpandError::ArcSegmentLimit { needed, maximum })?;
    }
    let Some(&point) = points.first() else { return Ok(()) };
    let mut first = None;
    let mut previous = None;
    let segment_slots = points.len().saturating_sub(1) +
                        usize::from(closed && points.len() > 1);
    for index in 0..segment_slots {
        let (from, to) = segment_at(points, index);
        unit_vector(from, to).map_err(|()| StrokeExpandError::NonFinitePoint)?;
    }
    for index in 0..segment_slots {
        let (from, to) = segment_at(points, index);
        let Some(unit) = unit_vector(from, to).map_err(|()|
            StrokeExpandError::NonFinitePoint)? else { continue };
        emit_segment_body(from, to, unit, options.half_width(), sink)?;
        if let Some((previous_to, previous_unit)) = previous {
            emit_join(previous_to, previous_unit, unit, options, sink)?;
        } else { first = Some((from, unit)); }
        previous = Some((to, unit));
    }
    let (Some((first_point, first_unit)), Some((last_point, last_unit))) =
        (first, previous) else { return stroke_point(point, options, sink) };
    if closed {
        emit_join(first_point, last_unit, first_unit, options, sink)
    } else {
        emit_cap(first_point, first_unit, true, options, sink)?;
        emit_cap(last_point, last_unit, false, options, sink)
    }
}

/// Applies the documented cap behavior to a point-only contour.
pub fn stroke_point<S: EdgeSink>(point: Point, options: StrokeOptions,
    sink: &mut S) -> Result<(), StrokeExpandError<S::Error>> {
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
            contour.arc(point, radius, 0.0, PI * 2.0, segments)?;
            contour.close()
        }
    }
}

fn point_is_finite(point: Point) -> bool { point.x.is_finite() && point.y.is_finite() }

fn segment_at(points: &[Point], index: usize) -> (Point, Point) {
    if index + 1 < points.len() {
        (points[index], points[index + 1])
    } else { (points[points.len() - 1], points[0]) }
}

fn unit_vector(from: Point, to: Point) -> Result<Option<Point>, ()> {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = libm::sqrtf(dx * dx + dy * dy);
    if !length.is_finite() { return Err(()); }
    Ok((length != 0.0).then(|| (dx / length, dy / length).into()))
}

fn arc_segments(radius: f32, options: StrokeOptions) -> Result<usize, (usize, u16)> {
    let tolerance = options.tolerance().min(radius);
    let maximum_angle = 2.0 * libm::acosf((1.0 - tolerance / radius).clamp(-1.0, 1.0));
    let needed = libm::ceilf(PI / maximum_angle).max(2.0) as usize;
    if  needed > options.max_arc_segments() as usize {
        Err((needed, options.max_arc_segments()))
    } else { Ok(needed) }
}

fn emit_segment_body<S: EdgeSink>(from: Point, to: Point, unit: Point, radius: f32,
    sink: &mut S) -> Result<(), StrokeExpandError<S::Error>> {
    let normal: Point = (-unit.y * radius, unit.x * radius).into();
    emit_polygon(&[
        (from.x + normal.x, from.y + normal.y).into(),
        (from.x - normal.x, from.y - normal.y).into(),
        (to.x - normal.x, to.y - normal.y).into(),
        (to.x + normal.x, to.y + normal.y).into(),
    ], sink)
}

fn emit_cap<S: EdgeSink>(point: Point, unit: Point, start: bool, options: StrokeOptions,
    sink: &mut S) -> Result<(), StrokeExpandError<S::Error>> {
    let (radius, direction) = (options.half_width(), if start { -1.0 } else { 1.0 });
    match options.cap {
        LineCap::Butt => Ok(()),
        LineCap::Square => {
            let end: Point = (point.x + unit.x * radius * direction,
                              point.y + unit.y * radius * direction).into();
            emit_segment_body(point, end, unit, radius, sink)
        }
        LineCap::Round => {
            let segments = arc_segments(radius, options).map_err(|(needed, maximum)|
                StrokeExpandError::ArcSegmentLimit { needed, maximum })?;
            let angle = libm::atan2f(unit.y, unit.x);
            let start_angle = if start { angle + FRAC_PI_2 } else { angle - FRAC_PI_2 };
            let mut contour = EdgeContour::new(sink);
            contour.point(point)?;
            contour.point((point.x + radius * libm::cosf(start_angle),
                           point.y + radius * libm::sinf(start_angle)).into())?;
            contour.arc(point, radius, start_angle, PI, segments)?;
            contour.close()
        }
    }
}

fn emit_join<S: EdgeSink>(point: Point, before: Point, after: Point,
    options: StrokeOptions, sink: &mut S) -> Result<(), StrokeExpandError<S::Error>> {
    let cross = before.x * after.y - before.y * after.x;
    let dot = before.x * after.x + before.y * after.y;
    if cross == 0.0 {
        return if dot < 0.0 && options.join == LineJoin::Round {
            stroke_point(point, options.with_cap(LineCap::Round), sink)
        } else { Ok(()) };
    }
    let (radius, side) = (options.half_width(), if cross > 0.0 { -1.0 } else { 1.0 });
    let before_outer: Point =
        (point.x - before.y * radius * side, point.y + before.x * radius * side).into();
    let after_outer: Point =
        (point.x - after.y * radius * side, point.y + after.x * radius * side).into();
    match options.join {
        LineJoin::Bevel => emit_polygon(&[point, before_outer, after_outer], sink),
        LineJoin::Round => {
            let base_segments = arc_segments(radius, options).map_err(|(needed, maximum)|
                StrokeExpandError::ArcSegmentLimit { needed, maximum })?;
            let start = libm::atan2f(before_outer.y - point.y, before_outer.x - point.x);
            let end = libm::atan2f(after_outer.y - point.y, after_outer.x - point.x);
            let mut sweep = end - start;
            if cross > 0.0 && sweep < 0.0 { sweep += PI * 2.0; }
            if cross < 0.0 && sweep > 0.0 { sweep -= PI * 2.0; }
            let segments = libm::ceilf(base_segments as f32 * sweep.abs() / PI)
                .max(1.0) as usize;
            let mut contour = EdgeContour::new(sink);
            contour.point(point)?;
            contour.point(before_outer)?;
            contour.arc(point, radius, start, sweep, segments)?;
            contour.close()
        }
        LineJoin::Miter => {
            let delta: Point =
                (after_outer.x - before_outer.x, after_outer.y - before_outer.y).into();
            let distance = (delta.x * after.y - delta.y * after.x) / cross;
            let miter: Point = (before_outer.x + before.x * distance,
                                before_outer.y + before.y * distance).into();
            let (dx, dy) = (miter.x - point.x, miter.y - point.y);
            let limit = radius * options.miter_limit();
            if dx * dx + dy * dy <= limit * limit {
                emit_polygon(&[point, before_outer, miter, after_outer], sink)
            } else { emit_polygon(&[point, before_outer, after_outer], sink) }
        }
    }
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

    #[test] fn polyline_joins_support_bevel_round_miter_and_fallback() {
        let points = [(2.0, 4.0).into(), (4.0, 4.0).into(), (4.0, 6.0).into()];
        let collect = |join, miter_limit| {
            let mut edges = Vec::new();
            let options = StrokeOptions::new(2.0).unwrap().with_join(join)
                .with_miter_limit(miter_limit).unwrap();
            stroke_polyline(&points, false, options, &mut |edge| {
                edges.push(edge); Ok::<_, Infallible>(())
            }).unwrap();
            edges
        };
        let has_corner = |edges: &[Edge]| edges.iter().any(|edge|
            [edge.upper, edge.lower].contains(&(5.0, 3.0).into()));
        let bevel = collect(LineJoin::Bevel, 4.0);
        let round = collect(LineJoin::Round, 4.0);
        let miter = collect(LineJoin::Miter, 4.0);
        let fallback = collect(LineJoin::Miter, 1.0);
        assert!(!has_corner(&bevel) && !has_corner(&round));
        assert!(has_corner(&miter) && !has_corner(&fallback));
        assert!(round.len() > bevel.len());
    }

    #[test] fn polylines_ignore_repeated_points_and_closed_contours_have_no_caps() {
        let collect = |points: &[Point], closed| {
            let mut edges = Vec::new();
            stroke_polyline(points, closed,
                StrokeOptions::new(2.0).unwrap().with_cap(LineCap::Square),
                &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }).unwrap();
            edges
        };
        let plain = collect(&[(2.0, 3.0).into(), (6.0, 3.0).into()], false);
        let repeated = collect(&[
            (2.0, 3.0).into(), (2.0, 3.0).into(),
            (6.0, 3.0).into(), (6.0, 3.0).into(),
        ], false);
        assert_eq!(plain, repeated);

        let closed = collect(&[(2.0, 3.0).into(), (6.0, 3.0).into()], true);
        let x_bounds = |edges: &[Edge]| edges.iter().flat_map(|edge|
            [edge.upper.x, edge.lower.x]).fold((f32::INFINITY, f32::NEG_INFINITY),
            |(minimum, maximum), x| (minimum.min(x), maximum.max(x)));
        assert_eq!(x_bounds(&plain), (1.0, 7.0));
        assert_eq!(x_bounds(&closed), (2.0, 6.0));
    }

    #[test] fn polyline_preflight_rejects_arc_budget_and_overflow_before_writing() {
        let mut edges = Vec::new();
        let options = StrokeOptions::new(100.0).unwrap().with_join(LineJoin::Round)
            .with_tolerance(1e-4).unwrap().with_max_arc_segments(2).unwrap();
        assert!(matches!(stroke_polyline(&[
            (0.0, 0.0).into(), (1.0, 0.0).into(), (1.0, 1.0).into(),
        ], false, options, &mut |edge| {
            edges.push(edge); Ok::<_, Infallible>(())
        }), Err(StrokeExpandError::ArcSegmentLimit { .. })));
        assert!(edges.is_empty());

        assert_eq!(stroke_polyline(&[
            (0.0, 0.0).into(), (1.0, 0.0).into(),
            (f32::MAX, f32::MAX).into(), (-f32::MAX, -f32::MAX).into(),
        ], false, StrokeOptions::default(), &mut |edge| {
            edges.push(edge); Ok::<_, Infallible>(())
        }), Err(StrokeExpandError::NonFinitePoint));
        assert!(edges.is_empty());
    }
}
