//! No-FPU stroke expansion for Q24.8 polylines.

use crate::{common::{geometry::{Affine, Edge, EdgeSink, Path, Point},
        stroke::{FlattenedStrokePath, LineCap, LineJoin, StrokePathWorkspace,
            StrokeWorkspaceError, flatten_stroke_path_with}},
    fixed::{DEVICE_RAW_LIMIT, Scalar,
        flatten::{self, Error as FlattenError, Options as FlattenOptions},
        math::{Angle, cordic_turn, cordic_unit_vector, integer_sqrt_u64}}};

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum Error {
    NonPositiveWidth, WidthOutOfRange, MiterLimitTooSmall, RoundSegmentLimitZero,
}

/// Validated Q24.8 stroke parameters.
///
/// ```
/// use ugl_rs::{common::stroke::{LineCap, LineJoin},
///     fixed::{Scalar, stroke::{Error, Options}}};
///
/// let options = Options::new(Scalar::from_num(6)).unwrap()
///     .with_cap(LineCap::Round).with_join(LineJoin::Bevel)
///     .with_miter_limit(Scalar::from_num(8)).unwrap()
///     .with_round_segments(16).unwrap();
/// assert_eq!(options.width(), Scalar::from_num(6));
/// assert_eq!((options.cap(), options.join()), (LineCap::Round, LineJoin::Bevel));
/// assert_eq!(Options::new(Scalar::ZERO), Err(Error::NonPositiveWidth));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub struct Options {
    width: i32, miter_limit: i32, round_segments: u16,
    cap: LineCap, join: LineJoin,
}

impl Options {
    pub fn new(width: Scalar) -> Result<Self, Error> {
        let width = width.to_bits();
        if width <= 0 { return Err(Error::NonPositiveWidth); }
        if width > DEVICE_RAW_LIMIT { return Err(Error::WidthOutOfRange); }
        Ok(Self { width, ..Self::default() })
    }

    pub fn with_cap(mut self, cap: LineCap) -> Self { self.cap = cap; self }
    pub fn with_join(mut self, join: LineJoin) -> Self { self.join = join; self }

    pub fn with_miter_limit(mut self, limit: Scalar) ->
        Result<Self, Error> {
        if limit < Scalar::ONE { return Err(Error::MiterLimitTooSmall); }
        self.miter_limit = limit.to_bits();   Ok(self)
    }

    /// Sets the number of segments used for a half circle.
    pub fn with_round_segments(mut self, segments: u16) -> Result<Self, Error> {
        if segments == 0 { return Err(Error::RoundSegmentLimitZero); }
        self.round_segments = segments;   Ok(self)
    }

    pub fn width(&self) -> Scalar { Scalar::from_bits(self.width) }
    pub fn miter_limit(&self) -> Scalar { Scalar::from_bits(self.miter_limit) }
    pub fn cap(&self) -> LineCap { self.cap }
    pub fn join(&self) -> LineJoin { self.join }
    pub fn round_segments(&self) -> u16 { self.round_segments }
}

impl Default for Options {
    fn default() -> Self { Self {
        width: Scalar::ONE.to_bits(), miter_limit: Scalar::ONE.to_bits() * 4,
        round_segments: 8, cap: LineCap::Butt, join: LineJoin::Miter,
    } }
}

#[derive(Clone, Copy, Debug, PartialEq)] pub enum ExpandError<E> {
    CoordinateOutOfRange, Sink(E),
}

#[derive(Clone, Copy)] struct Direction { dx: i64, dy: i64, length: u64 }

pub fn flatten_path<'a>(path: &Path<Scalar>,
    transform: Affine<Scalar>, options: FlattenOptions,
    workspace: &'a mut StrokePathWorkspace<'_, Scalar>) ->
    Result<FlattenedStrokePath<'a, Scalar>,
        FlattenError<StrokeWorkspaceError>> {
    flatten_stroke_path_with(workspace,
        |sink| flatten::flatten_path(path, transform, options, sink))
}

pub fn stroke_line<S: EdgeSink<Scalar>>(from: Point<Scalar>,
    to: Point<Scalar>, options: Options, sink: &mut S) ->
    Result<(), ExpandError<S::Error>> {
    stroke_polyline(&[from, to], false, options, sink)
}

/// Expands a fixed open or closed polyline into consistently wound fill edges.
///
/// All cap and join styles are supported without floating point.
pub fn stroke_polyline<S: EdgeSink<Scalar>>(points: &[Point<Scalar>],
    closed: bool, options: Options, sink: &mut S) ->
    Result<(), ExpandError<S::Error>> {
    if points.iter().any(|point| !point_in_range(*point)) {
        return Err(ExpandError::CoordinateOutOfRange);
    }
    if !closed && points.len() >= 2 && points.windows(2).all(|pair|
        pair[0] != pair[1]) && points.windows(3).all(|triple| {
            let before = direction(triple[0], triple[1]).unwrap();
            let after = direction(triple[1], triple[2]).unwrap();
            let cross = before.dx as i128 * after.dy as i128 -
                        before.dy as i128 * after.dx as i128;
            let dot = before.dx as i128 * after.dx as i128 +
                      before.dy as i128 * after.dy as i128;
            cross != 0 || dot >= 0
        }) {
        return stroke_open_outline(points, options, sink);
    }
    let Some(&point) = points.first() else { return Ok(()) };
    let slots = points.len().saturating_sub(1) + usize::from(closed && points.len() > 1);
    let (mut first, mut previous) = (None, None);
    for index in 0..slots {
        let (from, to) = segment_at(points, index);
        let Some(direction) = direction(from, to) else { continue };
        emit_segment_body(from, to, direction, options.width, sink)?;
        if let Some((at, before)) = previous {
            emit_join(at, before, direction, options, sink)?;
        } else { first = Some((from, direction)); }
        previous = Some((to, direction));
    }
    let (Some((first_point, first_direction)), Some((last_point, last_direction))) =
        (first, previous) else {
            if closed { return Ok(()) }
            return match options.cap {
                LineCap::Butt => Ok(()),
                LineCap::Square => emit_square_point(point, options.width, sink),
                LineCap::Round => emit_round_point(point, options, sink),
            };
        };
    if closed {
        emit_join(first_point, last_direction, first_direction, options, sink)
    } else {
        emit_cap(first_point, first_direction, true, options, sink)?;
        emit_cap(last_point, last_direction, false, options, sink)
    }
}

/// Emits a regular open stroke as one boundary instead of overlapping segment,
/// join, and cap polygons. Besides reducing edge storage, this keeps the fixed
/// rasterizer's active set proportional to the visible outline.
fn stroke_open_outline<S: EdgeSink<Scalar>>(points: &[Point<Scalar>],
    options: Options, sink: &mut S) -> Result<(), ExpandError<S::Error>> {
    let first_direction = direction(points[0], points[1]).unwrap();
    let last = points.len() - 1;
    let last_direction = direction(points[last - 1], points[last]).unwrap();
    let extension = options.cap == LineCap::Square;
    let (start_extension, end_extension) =
        if extension { (-1, 1) } else { (0, 0) };
    let mut contour = EdgeContour::new(sink);

    contour.point(outline_endpoint(points[0], first_direction, options.width,
        1, start_extension)?)?;
    for index in 1..last {
        outline_join(&mut contour, points[index],
            direction(points[index - 1], points[index]).unwrap(),
            direction(points[index], points[index + 1]).unwrap(),
            1, options)?;
    }
    contour.point(outline_endpoint(points[last], last_direction, options.width,
        1, end_extension)?)?;
    if options.cap == LineCap::Round {
        contour_arc(&mut contour, points[last], options.width,
            cordic_turn(-last_direction.dy, last_direction.dx),
            -(Angle::HALF_TURN.to_bits() as i64), options.round_segments as _)?;
    } else {
        contour.point(outline_endpoint(points[last], last_direction, options.width,
            -1, end_extension)?)?;
    }
    for index in (1..last).rev() {
        let before = reverse(direction(points[index], points[index + 1]).unwrap());
        let after = reverse(direction(points[index - 1], points[index]).unwrap());
        outline_join(&mut contour, points[index], before, after, 1, options)?;
    }
    contour.point(outline_endpoint(points[0], first_direction, options.width,
        -1, start_extension)?)?;
    if options.cap == LineCap::Round {
        contour_arc(&mut contour, points[0], options.width,
            cordic_turn(first_direction.dy, -first_direction.dx),
            -(Angle::HALF_TURN.to_bits() as i64), options.round_segments as _)?;
    }
    contour.close()
}

fn reverse(direction: Direction) -> Direction {
    Direction { dx: -direction.dx, dy: -direction.dy, ..direction }
}

fn outline_endpoint<E>(point: Point<Scalar>, direction: Direction, width: i32,
    side: i64, extension: i64) -> Result<Point<Scalar>, ExpandError<E>> {
    let (nx, ny) = normal(direction, width);
    let denominator = direction.length as i128 * 2;
    let (tx, ty) = (
        round_ratio(direction.dx as i128 * width as i128, denominator),
        round_ratio(direction.dy as i128 * width as i128, denominator),
    );
    offset(point, nx * side + tx * extension, ny * side + ty * extension)
}

fn outline_join<S: EdgeSink<Scalar>>(contour: &mut EdgeContour<'_, S>,
    point: Point<Scalar>, before: Direction, after: Direction, side: i64,
    options: Options) -> Result<(), ExpandError<S::Error>> {
    let cross = before.dx as i128 * after.dy as i128 -
                before.dy as i128 * after.dx as i128;
    let (before_x, before_y) = normal(before, options.width);
    let (after_x, after_y) = normal(after, options.width);
    let before_offset = offset(point, before_x * side, before_y * side)?;
    let after_offset = offset(point, after_x * side, after_y * side)?;
    if cross == 0 { return contour.point(after_offset); }
    let (delta_x, delta_y) = (
        after_offset.x.to_bits() as i128 - before_offset.x.to_bits() as i128,
        after_offset.y.to_bits() as i128 - before_offset.y.to_bits() as i128,
    );
    let distance = delta_x * after.dy as i128 - delta_y * after.dx as i128;
    let intersection = offset(before_offset,
        round_ratio(before.dx as i128 * distance, cross),
        round_ratio(before.dy as i128 * distance, cross))?;
    if cross * side as i128 >= 0 { return contour.point(intersection); }
    if options.join == LineJoin::Round {
        contour.point(before_offset)?;
        let start = cordic_turn(
            before_offset.x.to_bits() as i64 - point.x.to_bits() as i64,
            before_offset.y.to_bits() as i64 - point.y.to_bits() as i64);
        let end = cordic_turn(
            after_offset.x.to_bits() as i64 - point.x.to_bits() as i64,
            after_offset.y.to_bits() as i64 - point.y.to_bits() as i64);
        // The compact contour visits both sides in their forward direction,
        // so every exposed round join advances clockwise to the next offset.
        let sweep = -(start.wrapping_sub(end) as i64);
        let segments = (options.round_segments as u64 * sweep.unsigned_abs())
            .div_ceil(Angle::HALF_TURN.to_bits() as u64).max(1) as usize;
        return contour_arc(contour, point, options.width, start, sweep, segments);
    }
    if options.join == LineJoin::Miter {
        let (dx, dy) = (
            intersection.x.to_bits() as i128 - point.x.to_bits() as i128,
            intersection.y.to_bits() as i128 - point.y.to_bits() as i128,
        );
        let scale = 2_i128 * Scalar::ONE.to_bits() as i128;
        let limit = options.width as i128 * options.miter_limit as i128;
        if (dx * dx + dy * dy) * scale * scale <= limit * limit {
            return contour.point(intersection);
        }
    }
    contour.point(before_offset)?;
    contour.point(after_offset)
}

fn contour_arc<S: EdgeSink<Scalar>>(contour: &mut EdgeContour<'_, S>,
    center: Point<Scalar>, width: i32, start: u32, sweep: i64, segments: usize) ->
    Result<(), ExpandError<S::Error>> {
    for index in 1..=segments {
        let angle = start.wrapping_add((sweep as i128 * index as i128 /
            segments as i128) as _);
        contour.point(circle_point(center, width, Angle::from_bits(angle))?)?;
    }
    Ok(())
}

fn point_in_range(point: Point<Scalar>) -> bool {
    [point.x.to_bits(), point.y.to_bits()].iter()
        .all(|value| value.unsigned_abs() <= DEVICE_RAW_LIMIT as u32)
}

fn segment_at(points: &[Point<Scalar>], index: usize) ->
    (Point<Scalar>, Point<Scalar>) {
    if index + 1 < points.len() { (points[index], points[index + 1])
    } else { (points[points.len() - 1], points[0]) }
}

fn direction(from: Point<Scalar>, to: Point<Scalar>) -> Option<Direction> {
    let (dx, dy) = (to.x.to_bits() as i64 - from.x.to_bits() as i64,
                    to.y.to_bits() as i64 - from.y.to_bits() as i64);
    let squared = (dx * dx + dy * dy) as u64;
    if squared == 0 { return None; }
    let floor = integer_sqrt_u64(squared);
    let length = if squared - floor * floor > floor { floor + 1 } else { floor };
    Some(Direction { dx, dy, length })
}

fn normal(direction: Direction, width: i32) -> (i64, i64) {
    let denominator = direction.length as i128 * 2;
    (round_ratio(-direction.dy as i128 * width as i128, denominator),
     round_ratio( direction.dx as i128 * width as i128, denominator))
}

fn round_ratio(numerator: i128, denominator: i128) -> i64 {
    if denominator < 0 { return round_ratio(-numerator, -denominator); }
    let magnitude = (numerator.unsigned_abs() + denominator as u128 / 2) /
                    denominator as u128;
    if numerator < 0 { -(magnitude as i64) } else { magnitude as _ }
}

fn offset<E>(point: Point<Scalar>, dx: i64, dy: i64) ->
    Result<Point<Scalar>, ExpandError<E>> {
    let (x, y) = (point.x.to_bits() as i64 + dx, point.y.to_bits() as i64 + dy);
    if [x, y].iter().any(|value| value.unsigned_abs() > DEVICE_RAW_LIMIT as u64) {
        return Err(ExpandError::CoordinateOutOfRange);
    }
    Ok((Scalar::from_bits(x as _), Scalar::from_bits(y as _)).into())
}

fn emit_segment_body<S: EdgeSink<Scalar>>(from: Point<Scalar>,
    to: Point<Scalar>, direction: Direction, width: i32, sink: &mut S) ->
    Result<(), ExpandError<S::Error>> {
    let (nx, ny) = normal(direction, width);
    emit_polygon(&[
        offset(from,  nx,  ny)?,
        offset(from, -nx, -ny)?,
        offset(to,   -nx, -ny)?,
        offset(to,    nx,  ny)?,
    ], sink)
}

fn emit_cap<S: EdgeSink<Scalar>>(point: Point<Scalar>, direction: Direction,
    start: bool, options: Options, sink: &mut S) ->
    Result<(), ExpandError<S::Error>> {
    if options.cap == LineCap::Butt { return Ok(()) }
    if options.cap == LineCap::Round {
        let tangent = cordic_turn(direction.dx, direction.dy);
        let start_angle = tangent.wrapping_sub(Angle::QUARTER_TURN.to_bits());
        let sweep = if start {
            -(Angle::HALF_TURN.to_bits() as i64)
        } else { Angle::HALF_TURN.to_bits() as i64 };
        return emit_round_wedge(point, options.width, start_angle, sweep,
            options.round_segments as _, sink);
    }
    let sign = if start { -1 } else { 1 };
    let denominator = direction.length as i128 * 2;
    let (dx, dy) = (
        round_ratio(direction.dx as i128 * options.width as i128, denominator) * sign,
        round_ratio(direction.dy as i128 * options.width as i128, denominator) * sign,
    );
    let end = offset(point, dx, dy)?;
    emit_segment_body(point, end, direction, options.width, sink)
}

fn emit_square_point<S: EdgeSink<Scalar>>(point: Point<Scalar>, width: i32,
    sink: &mut S) -> Result<(), ExpandError<S::Error>> {
    let (low, high) = (-(width as i64) / 2, (width as i64 + 1) / 2);
    emit_polygon(&[
        offset(point, low, low)?,
        offset(point, high, low)?,
        offset(point, high, high)?,
        offset(point, low, high)?,
    ], sink)
}

fn emit_round_point<S: EdgeSink<Scalar>>(point: Point<Scalar>,
    options: Options, sink: &mut S) ->
    Result<(), ExpandError<S::Error>> {
    let segments = options.round_segments as usize * 2;
    let mut contour = EdgeContour::new(sink);
    contour.point(circle_point(point, options.width, Angle::ZERO)?)?;
    for index in 1..segments {
        let angle = ((index as u64) << 32) / segments as u64;
        contour.point(circle_point(point, options.width,
            Angle::from_bits(angle as _))?)?;
    }
    contour.close()
}

fn emit_join<S: EdgeSink<Scalar>>(point: Point<Scalar>,
    before: Direction, after: Direction, options: Options, sink: &mut S) ->
    Result<(), ExpandError<S::Error>> {
    let cross = before.dx as i128 * after.dy as i128 -
                before.dy as i128 * after.dx as i128;
    if cross == 0 { return Ok(()) }
    let side = if cross > 0 { -1 } else { 1 };
    let (before_x, before_y) = normal(before, options.width);
    let (after_x, after_y) = normal(after, options.width);
    let before_outer = offset(point, before_x * side, before_y * side)?;
    let after_outer = offset(point, after_x * side, after_y * side)?;
    match options.join {
        LineJoin::Bevel =>
            return emit_polygon(&[point, before_outer, after_outer], sink),
        LineJoin::Round => {
            let start = cordic_turn(
                before_outer.x.to_bits() as i64 - point.x.to_bits() as i64,
                before_outer.y.to_bits() as i64 - point.y.to_bits() as i64);
            let end = cordic_turn(
                after_outer.x.to_bits() as i64 - point.x.to_bits() as i64,
                after_outer.y.to_bits() as i64 - point.y.to_bits() as i64);
            let sweep = if cross > 0 {
                end.wrapping_sub(start) as i64
            } else { -(start.wrapping_sub(end) as i64) };
            let segments = (options.round_segments as u64 * sweep.unsigned_abs())
                .div_ceil(Angle::HALF_TURN.to_bits() as u64).max(1) as usize;
            return emit_round_wedge(
                point, options.width, start, sweep, segments, sink);
        }
        LineJoin::Miter => {}
    }
    let (delta_x, delta_y) = (
        after_outer.x.to_bits() as i128 - before_outer.x.to_bits() as i128,
        after_outer.y.to_bits() as i128 - before_outer.y.to_bits() as i128,
    );
    let distance = delta_x * after.dy as i128 - delta_y * after.dx as i128;
    let miter = offset(before_outer,
        round_ratio(before.dx as i128 * distance, cross),
        round_ratio(before.dy as i128 * distance, cross))?;
    let (dx, dy) = (miter.x.to_bits() as i128 - point.x.to_bits() as i128,
                    miter.y.to_bits() as i128 - point.y.to_bits() as i128);
    let scale = 2_i128 * Scalar::ONE.to_bits() as i128;
    let limit = options.width as i128 * options.miter_limit as i128;
    if (dx * dx + dy * dy) * scale * scale <= limit * limit {
        emit_polygon(&[point, before_outer, miter, after_outer], sink)
    } else { emit_polygon(&[point, before_outer, after_outer], sink) }
}

fn circle_point<E>(center: Point<Scalar>, width: i32, angle: Angle) ->
    Result<Point<Scalar>, ExpandError<E>> {
    let (cosine, sine) = cordic_unit_vector(angle);
    let denominator = 2_i128 << 30;
    offset(center,
        round_ratio(cosine as i128 * width as i128, denominator),
        round_ratio(  sine as i128 * width as i128, denominator))
}

fn emit_round_wedge<S: EdgeSink<Scalar>>(center: Point<Scalar>, width: i32,
    start: u32, sweep: i64, segments: usize, sink: &mut S) ->
    Result<(), ExpandError<S::Error>> {
    let mut contour = EdgeContour::new(sink);
    contour.point(center)?;
    contour.point(circle_point(center, width, Angle::from_bits(start))?)?;
    for index in 1..=segments {
        let offset = sweep as i128 * index as i128 / segments as i128;
        contour.point(circle_point(center, width,
            Angle::from_bits(start.wrapping_add(offset as _)))?)?;
    }
    contour.close()
}

fn emit_polygon<S: EdgeSink<Scalar>>(points: &[Point<Scalar>], sink: &mut S) ->
    Result<(), ExpandError<S::Error>> {
    for pair in points.windows(2) {
        if let Some(edge) = Edge::from_line(pair[0], pair[1]) {
            sink.edge(edge).map_err(ExpandError::Sink)?;
        }
    }
    if let (Some(&first), Some(&last)) = (points.first(), points.last())
        && let Some(edge) = Edge::from_line(last, first) {
        sink.edge(edge).map_err(ExpandError::Sink)?;
    }
    Ok(())
}

struct EdgeContour<'a, S> {
    sink: &'a mut S, first: Option<Point<Scalar>>, previous: Option<Point<Scalar>>,
}

impl<'a, S> EdgeContour<'a, S> {
    fn new(sink: &'a mut S) -> Self { Self { sink, first: None, previous: None } }
}

impl<S: EdgeSink<Scalar>> EdgeContour<'_, S> {
    fn point(&mut self, point: Point<Scalar>) ->
        Result<(), ExpandError<S::Error>> {
        if let Some(previous) = self.previous {
            if let Some(edge) = Edge::from_line(previous, point) {
                self.sink.edge(edge).map_err(ExpandError::Sink)?;
            }
        } else { self.first = Some(point); }
        self.previous = Some(point);   Ok(())
    }

    fn close(self) -> Result<(), ExpandError<S::Error>> {
        if let (Some(previous), Some(first)) = (self.previous, self.first)
            && let Some(edge) = Edge::from_line(previous, first) {
            self.sink.edge(edge).map_err(ExpandError::Sink)?;
        }
        Ok(())
    }
}

#[cfg(test)] mod tests { use super::*;
    use alloc::vec::Vec;
    use core::convert::Infallible;
    #[cfg(feature = "f32")] use crate::float::stroke;

    fn fixed(value: f32) -> Scalar { Scalar::from_num(value) }

    fn collect(points: &[(f32, f32)], closed: bool, options: Options) ->
        Vec<Edge<Scalar>> {
        let points: Vec<_> = points.iter().map(|&(x, y)| (fixed(x), fixed(y)).into()).collect();
        let mut edges = Vec::new();
        stroke_polyline(&points, closed, options, &mut |edge| {
            edges.push(edge); Ok::<_, Infallible>(())
        }).unwrap();
        edges
    }

    fn x_bounds(edges: &[Edge<Scalar>]) -> (Scalar, Scalar) {
        edges.iter().flat_map(|edge| [edge.upper.x, edge.lower.x])
            .fold((Scalar::MAX, Scalar::MIN),
                |(minimum, maximum), x| (minimum.min(x), maximum.max(x)))
    }

    #[test] fn line_caps_have_exact_device_bounds() {
        let options = Options::new(fixed(2.0)).unwrap();
        let butt = collect(&[(2.0, 3.0), (6.0, 3.0)], false, options);
        let square = collect(&[(2.0, 3.0), (6.0, 3.0)], false,
            options.with_cap(LineCap::Square));
        assert_eq!(x_bounds(&butt), (fixed(2.0), fixed(6.0)));
        assert_eq!(x_bounds(&square), (fixed(1.0), fixed(7.0)));
    }

    #[cfg(feature = "f32")]
    #[test] fn diagonal_offsets_track_the_f32_reference() {
        let options = Options::new(fixed(2.0)).unwrap();
        let actual = collect(&[(2.0, 2.0), (6.0, 6.0)], false, options);
        let mut expected = Vec::new();
        stroke::stroke_line((2.0, 2.0).into(), (6.0, 6.0).into(),
            stroke::StrokeOptions::new(2.0).unwrap(), &mut |edge| {
                expected.push(edge); Ok::<_, Infallible>(())
            }).unwrap();
        let actual_bounds = actual.iter().flat_map(|edge| [edge.upper, edge.lower]).fold(
            (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY),
            |(min_x, min_y, max_x, max_y), point| (min_x.min(point.x.to_num()),
                min_y.min(point.y.to_num()), max_x.max(point.x.to_num()),
                max_y.max(point.y.to_num())));
        let expected_bounds = expected.iter().flat_map(|edge| [edge.upper, edge.lower]).fold(
            (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY),
            |(min_x, min_y, max_x, max_y), point| (min_x.min(point.x),
                min_y.min(point.y), max_x.max(point.x), max_y.max(point.y)));
        for (actual, expected) in [actual_bounds.0, actual_bounds.1, actual_bounds.2,
            actual_bounds.3].into_iter().zip([expected_bounds.0, expected_bounds.1,
                expected_bounds.2, expected_bounds.3]) {
            assert!((actual - expected).abs() <= 1.0 / 128.0,
                "actual={actual}, expected={expected}");
        }
    }

    #[test] fn bevel_and_miter_joins_follow_limit_and_degenerate_rules() {
        let base = Options::new(fixed(2.0)).unwrap();
        let points = [(2.0, 4.0), (4.0, 4.0), (4.0, 6.0)];
        let bevel = collect(&points, false, base.with_join(LineJoin::Bevel));
        let miter = collect(&points, false, base);
        let fallback = collect(&points, false,
            base.with_miter_limit(Scalar::ONE).unwrap());
        let corner = (fixed(5.0), fixed(3.0)).into();
        let has_corner = |edges: &[Edge<Scalar>]| edges.iter()
            .any(|edge| [edge.upper, edge.lower].contains(&corner));
        assert!(!has_corner(&bevel));
        assert!(has_corner(&miter));
        assert!(!has_corner(&fallback));

        let plain = collect(&[(1.0, 2.0), (5.0, 2.0)], false, base);
        let repeated = collect(
            &[(1.0, 2.0), (1.0, 2.0), (5.0, 2.0), (5.0, 2.0)], false, base);
        assert_eq!(x_bounds(&plain), x_bounds(&repeated));
        assert_eq!(plain.iter().flat_map(|edge| [edge.upper.y, edge.lower.y]).min(),
            repeated.iter().flat_map(|edge| [edge.upper.y, edge.lower.y]).min());
        assert_eq!(plain.iter().flat_map(|edge| [edge.upper.y, edge.lower.y]).max(),
            repeated.iter().flat_map(|edge| [edge.upper.y, edge.lower.y]).max());
        assert!(!collect(&[(1.0, 2.0), (5.0, 2.0)], true, base).is_empty());
    }

    #[test] fn round_caps_and_joins_are_bounded_and_configurable() {
        let base = Options::new(fixed(2.0)).unwrap()
            .with_round_segments(8).unwrap();
        let round = collect(&[(2.0, 3.0), (6.0, 3.0)], false,
            base.with_cap(LineCap::Round));
        assert_eq!(x_bounds(&round), (fixed(1.0), fixed(7.0)));
        let point = collect(&[(4.0, 5.0)], false, base.with_cap(LineCap::Round));
        assert_eq!(x_bounds(&point), (fixed(3.0), fixed(5.0)));

        let points = [(2.0, 4.0), (4.0, 4.0), (4.0, 6.0)];
        let bevel = collect(&points, false, base.with_join(LineJoin::Bevel));
        let round = collect(&points, false, base.with_join(LineJoin::Round));
        assert!(round.len() > bevel.len());
        assert_eq!(base.with_round_segments(0),
            Err(Error::RoundSegmentLimitZero));
    }

    #[test] fn stroke_rejects_out_of_range_before_writing() {
        assert_eq!(Options::new(Scalar::ZERO),
            Err(Error::NonPositiveWidth));
        let mut edges = Vec::new();
        let outside = Scalar::from_bits(DEVICE_RAW_LIMIT + 1);
        assert_eq!(stroke_line((outside, fixed(0.0)).into(),
            (fixed(1.0), fixed(0.0)).into(), Options::default(),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }),
            Err(ExpandError::CoordinateOutOfRange));
        assert!(edges.is_empty());
    }
}
