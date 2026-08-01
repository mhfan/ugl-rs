//! Floating-point stroke expansion.

use core::f32::consts::{FRAC_PI_2, PI};
use crate::{common::{edge::{Edge, EdgeSink}, geometry::{Affine, Path, Point},
        stroke::{FlattenedStrokePath, LineCap, LineJoin, StrokePathWorkspace,
            StrokeWorkspaceError, flatten_stroke_path_with}},
    float::{acos, atan2, ceil, cos, sin, sqrt,
        flatten::{flatten_path, FlattenError, FlattenOptions}}};

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum StrokeError {
    NonFiniteWidth, NonPositiveWidth, NonFiniteMiterLimit, MiterLimitTooSmall,
    NonFiniteTolerance, NonPositiveTolerance, ArcSegmentLimitZero,
}

/// Validated device-space stroke parameters.
///
/// ```
/// use ugl_rs::{common::stroke::{LineCap, LineJoin},
///     float::stroke::{StrokeError, StrokeOptions}};
///
/// let options = StrokeOptions::new(6.0).unwrap()
///     .with_cap(LineCap::Round).with_join(LineJoin::Bevel)
///     .with_miter_limit(8.0).unwrap()
///     .with_tolerance(0.125).unwrap()
///     .with_max_arc_segments(32).unwrap();
/// assert_eq!((options.width(), options.half_width()), (6.0, 3.0));
/// assert_eq!((options.cap(), options.join()), (LineCap::Round, LineJoin::Bevel));
/// assert_eq!(StrokeOptions::new(0.0), Err(StrokeError::NonPositiveWidth));
/// ```
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

/// Flattens a transformed path into caller-owned, compact stroke storage.
pub fn flatten_stroke_path<'a>(path: &Path, transform: Affine, options: FlattenOptions,
    workspace: &'a mut StrokePathWorkspace<'_>) ->
    Result<FlattenedStrokePath<'a>, FlattenError<StrokeWorkspaceError>> {
    flatten_stroke_path_with(workspace,
        |sink| flatten_path(path, transform, options, sink))
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
    if (options.join == LineJoin::Round || !closed && options.cap == LineCap::Round) &&
        !points.is_empty() {
        arc_segments(options.half_width(), options).map_err(|(needed, maximum)|
            StrokeExpandError::ArcSegmentLimit { needed, maximum })?;
    }
    if !closed && points.len() >= 2 && points.windows(2).all(|pair| pair[0] != pair[1]) &&
        points.windows(3).all(|triple| {
            let (ax, ay) = (triple[1].x - triple[0].x, triple[1].y - triple[0].y);
            let (bx, by) = (triple[2].x - triple[1].x, triple[2].y - triple[1].y);
            ax * by != ay * bx || ax * bx + ay * by >= 0.0
        }) {
        return stroke_open_outline(points, options, sink);
    }
    let Some(&point) = points.first() else { return Ok(()) };
    let (mut first, mut previous) = (None, None);
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
        (first, previous) else {
            return if closed { Ok(()) } else { stroke_point(point, options, sink) };
        };
    if closed {
        emit_join(first_point, last_unit, first_unit, options, sink)
    } else {
        emit_cap(first_point, first_unit, true, options, sink)?;
        emit_cap(last_point, last_unit, false, options, sink)
    }
}

/// Emits the two sides and caps of a simple open stroke as one contour.
///
/// The general expansion below deliberately unions independent segment and
/// join polygons.  That is a useful fallback for degenerate input and round
/// geometry, but multiplies the edge count.  A non-round open polyline has a
/// direct boundary representation, so emit that boundary once.
fn stroke_open_outline<S: EdgeSink>(points: &[Point], options: StrokeOptions,
    sink: &mut S) -> Result<(), StrokeExpandError<S::Error>> {
    let radius = options.half_width();
    let first_unit = unit_vector(points[0], points[1])
        .map_err(|()| StrokeExpandError::NonFinitePoint)?.unwrap();
    let last = points.len() - 1;
    let last_unit = unit_vector(points[last - 1], points[last])
        .map_err(|()| StrokeExpandError::NonFinitePoint)?.unwrap();
    let extension = if options.cap == LineCap::Square { radius } else { 0.0 };
    let mut contour = EdgeContour::new(sink);

    contour.point(offset_endpoint(points[0], first_unit, radius, 1.0, -extension))?;
    for index in 1..last {
        let before = unit_vector(points[index - 1], points[index])
            .map_err(|()| StrokeExpandError::NonFinitePoint)?.unwrap();
        let after = unit_vector(points[index], points[index + 1])
            .map_err(|()| StrokeExpandError::NonFinitePoint)?.unwrap();
        emit_outline_join(&mut contour, points[index], before, after, 1.0, options)?;
    }
    contour.point(offset_endpoint(points[last], last_unit, radius, 1.0, extension))?;
    if options.cap == LineCap::Round {
        let start = atan2(last_unit.y, last_unit.x) + FRAC_PI_2;
        let segments = arc_segments(radius, options).map_err(|(needed, maximum)|
            StrokeExpandError::ArcSegmentLimit { needed, maximum })?;
        contour.arc(points[last], radius, start, -PI, segments)?;
    } else {
        contour.point(offset_endpoint(points[last], last_unit, radius, -1.0, extension))?;
    }
    for index in (1..last).rev() {
        let before = unit_vector(points[index + 1], points[index])
            .map_err(|()| StrokeExpandError::NonFinitePoint)?.unwrap();
        let after = unit_vector(points[index], points[index - 1])
            .map_err(|()| StrokeExpandError::NonFinitePoint)?.unwrap();
        emit_outline_join(&mut contour, points[index], before, after, 1.0, options)?;
    }
    contour.point(offset_endpoint(points[0], first_unit, radius, -1.0, -extension))?;
    if options.cap == LineCap::Round {
        let start = atan2(-first_unit.y, -first_unit.x) + FRAC_PI_2;
        let segments = arc_segments(radius, options).map_err(|(needed, maximum)|
            StrokeExpandError::ArcSegmentLimit { needed, maximum })?;
        contour.arc(points[0], radius, start, -PI, segments)?;
    }
    contour.close()
}

fn offset_endpoint(point: Point, unit: Point, radius: f32, side: f32,
    extension: f32) -> Point {
    (point.x - unit.y * radius * side + unit.x * extension,
     point.y + unit.x * radius * side + unit.y * extension).into()
}

fn emit_outline_join<S: EdgeSink>(contour: &mut EdgeContour<'_, S>, point: Point,
    before: Point, after: Point, side: f32, options: StrokeOptions) ->
    Result<(), StrokeExpandError<S::Error>> {
    let cross = before.x * after.y - before.y * after.x;
    let dot = before.x * after.x + before.y * after.y;
    debug_assert!(cross != 0.0 || dot >= 0.0);
    let radius = options.half_width();
    let before_offset = offset_endpoint(point, before, radius, side, 0.0);
    let after_offset = offset_endpoint(point, after, radius, side, 0.0);
    if cross == 0.0 {
        return contour.point(after_offset);
    }
    let delta: Point = (after_offset.x - before_offset.x,
                        after_offset.y - before_offset.y).into();
    let distance = (delta.x * after.y - delta.y * after.x) / cross;
    let intersection: Point = (before_offset.x + before.x * distance,
                               before_offset.y + before.y * distance).into();
    let outer = cross * side < 0.0;
    if !outer {
        return contour.point(intersection);
    }
    if options.join == LineJoin::Round {
        contour.point(before_offset)?;
        let start = atan2(before_offset.y - point.y, before_offset.x - point.x);
        let end = atan2(after_offset.y - point.y, after_offset.x - point.x);
        let mut sweep = end - start;
        if sweep > 0.0 { sweep -= PI * 2.0; }
        let base_segments = arc_segments(radius, options).map_err(|(needed, maximum)|
            StrokeExpandError::ArcSegmentLimit { needed, maximum })?;
        let segments = ceil(base_segments as f32 * sweep.abs() / PI)
            .max(1.0) as usize;
        return contour.arc(point, radius, start, sweep, segments);
    }
    if options.join == LineJoin::Miter {
        let (dx, dy) = (intersection.x - point.x, intersection.y - point.y);
        let limit = radius * options.miter_limit();
        if dx * dx + dy * dy <= limit * limit {
            return contour.point(intersection);
        }
    }
    contour.point(before_offset)?;
    contour.point(after_offset)
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
    if index + 1 < points.len() { (points[index], points[index + 1])
    } else { (points[points.len() - 1], points[0]) }
}

fn unit_vector(from: Point, to: Point) -> Result<Option<Point>, ()> {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = sqrt(dx * dx + dy * dy);
    if !length.is_finite() { return Err(()); }
    Ok((length != 0.0).then(|| (dx / length, dy / length).into()))
}

fn arc_segments(radius: f32, options: StrokeOptions) -> Result<usize, (usize, u16)> {
    let tolerance = options.tolerance().min(radius);
    let maximum_angle = 2.0 * acos((1.0 - tolerance / radius).clamp(-1.0, 1.0));
    let needed = ceil(PI / maximum_angle).max(2.0) as usize;
    if  needed > options.max_arc_segments() as usize {
        Err((needed, options.max_arc_segments()))
    } else { Ok(needed) }
}

fn emit_segment_body<S: EdgeSink>(from: Point, to: Point, unit: Point, radius: f32,
    sink: &mut S) -> Result<(), StrokeExpandError<S::Error>> {
    let normal: Point = (-unit.y * radius, unit.x * radius).into();
    emit_polygon(&[(from.x + normal.x, from.y + normal.y).into(),
                   (from.x - normal.x, from.y - normal.y).into(),
                     (to.x - normal.x,   to.y - normal.y).into(),
                     (to.x + normal.x,   to.y + normal.y).into()], sink)
}

fn emit_cap<S: EdgeSink>(point: Point, unit: Point, start: bool, options: StrokeOptions,
    sink: &mut S) -> Result<(), StrokeExpandError<S::Error>> {
    let (radius, direction) = (options.half_width(), if start { -1.0 } else { 1.0 });
    match options.cap {
        LineCap::Butt => Ok(()),
        LineCap::Square => {
            let end: Point = (point.x + unit.x * radius * direction,
                              point.y + unit.y * radius * direction).into();
            let cap_unit: Point = (unit.x * direction, unit.y * direction).into();
            emit_segment_body(point, end, cap_unit, radius, sink)
        }
        LineCap::Round => {
            let segments = arc_segments(radius, options).map_err(|(needed, maximum)|
                StrokeExpandError::ArcSegmentLimit { needed, maximum })?;
            let angle = atan2(unit.y, unit.x);
            let (start_angle, sweep) = if start {
                (angle - FRAC_PI_2, -PI)
            } else { (angle - FRAC_PI_2, PI) };
            let mut contour = EdgeContour::new(sink);
            contour.point(point)?;
            contour.point((point.x + radius * cos(start_angle),
                           point.y + radius * sin(start_angle)).into())?;
            contour.arc(point, radius, start_angle, sweep, segments)?;
            contour.close()
        }
    }
}

fn emit_join<S: EdgeSink>(point: Point, before: Point, after: Point,
    options: StrokeOptions, sink: &mut S) -> Result<(), StrokeExpandError<S::Error>> {
    let cross = before.x * after.y - before.y * after.x;
    let   dot = before.x * after.x + before.y * after.y;
    if cross == 0.0 {
        return if dot < 0.0 && options.join == LineJoin::Round {
            stroke_point(point, options.with_cap(LineCap::Round), sink)
        } else { Ok(()) };
    }
    let (radius, side) = (options.half_width(), if cross > 0.0 { -1.0 } else { 1.0 });
    let before_outer: Point =
        (point.x - before.y * radius * side, point.y + before.x * radius * side).into();
    let after_outer: Point =
        (point.x -  after.y * radius * side, point.y +  after.x * radius * side).into();
    match options.join {
        LineJoin::Bevel => emit_polygon(&[point, before_outer, after_outer], sink),
        LineJoin::Round => {
            let base_segments = arc_segments(radius, options).map_err(|(needed, maximum)|
                StrokeExpandError::ArcSegmentLimit { needed, maximum })?;
            let start = atan2(before_outer.y - point.y, before_outer.x - point.x);
            let   end = atan2( after_outer.y - point.y,  after_outer.x - point.x);
            let mut sweep = end - start;
            if cross > 0.0 && sweep < 0.0 { sweep += PI * 2.0; }
            if cross < 0.0 && sweep > 0.0 { sweep -= PI * 2.0; }
            let segments = ceil(base_segments as f32 * sweep.abs() / PI)
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
            self.point((center.x + radius * cos(angle),
                        center.y + radius * sin(angle)).into())?;
        }   Ok(())
    }

    fn close(self) -> Result<(), StrokeExpandError<S::Error>> {
        if let (Some(previous), Some(first)) = (self.previous, self.first)
            && let Some(edge) = Edge::from_line(previous, first) {
            self.sink.edge(edge).map_err(StrokeExpandError::Sink)?;
        }   Ok(())
    }
}
