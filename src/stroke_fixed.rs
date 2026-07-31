//! No-FPU stroke expansion for Q24.8 polylines.

use crate::{edge::{Edge, EdgeSink}, geometry::{FIXED_DEVICE_RAW_LIMIT, FixedScalar, Point},
    math::integer_sqrt_u64, stroke::{LineCap, LineJoin}};

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum FixedStrokeError {
    NonPositiveWidth, WidthOutOfRange, MiterLimitTooSmall,
}

/// Validated Q24.8 stroke parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub struct FixedStrokeOptions {
    width: i32, miter_limit: i32, cap: LineCap, join: LineJoin,
}

impl FixedStrokeOptions {
    pub fn new(width: FixedScalar) -> Result<Self, FixedStrokeError> {
        let width = width.to_bits();
        if width <= 0 { return Err(FixedStrokeError::NonPositiveWidth); }
        if width > FIXED_DEVICE_RAW_LIMIT { return Err(FixedStrokeError::WidthOutOfRange); }
        Ok(Self { width, ..Self::default() })
    }

    pub fn with_cap(mut self, cap: LineCap) -> Self { self.cap = cap; self }
    pub fn with_join(mut self, join: LineJoin) -> Self { self.join = join; self }

    pub fn with_miter_limit(mut self, limit: FixedScalar) ->
        Result<Self, FixedStrokeError> {
        if limit < FixedScalar::ONE { return Err(FixedStrokeError::MiterLimitTooSmall); }
        self.miter_limit = limit.to_bits();   Ok(self)
    }

    pub fn width(&self) -> FixedScalar { FixedScalar::from_bits(self.width) }
    pub fn miter_limit(&self) -> FixedScalar { FixedScalar::from_bits(self.miter_limit) }
    pub fn cap(&self) -> LineCap { self.cap }
    pub fn join(&self) -> LineJoin { self.join }
}

impl Default for FixedStrokeOptions {
    fn default() -> Self { Self {
        width: FixedScalar::ONE.to_bits(), miter_limit: FixedScalar::ONE.to_bits() * 4,
        cap: LineCap::Butt, join: LineJoin::Miter,
    } }
}

#[derive(Clone, Copy, Debug, PartialEq)] pub enum FixedStrokeExpandError<E> {
    CoordinateOutOfRange, UnsupportedRound, Sink(E),
}

#[derive(Clone, Copy)] struct Direction { dx: i64, dy: i64, length: u64 }

pub fn stroke_line_fixed<S: EdgeSink<FixedScalar>>(from: Point<FixedScalar>,
    to: Point<FixedScalar>, options: FixedStrokeOptions, sink: &mut S) ->
    Result<(), FixedStrokeExpandError<S::Error>> {
    stroke_polyline_fixed(&[from, to], false, options, sink)
}

/// Expands a fixed open or closed polyline into consistently wound fill edges.
///
/// Butt/square caps and bevel/miter joins are supported without floating point.
/// Round geometry returns `UnsupportedRound` before writing to `sink`.
pub fn stroke_polyline_fixed<S: EdgeSink<FixedScalar>>(points: &[Point<FixedScalar>],
    closed: bool, options: FixedStrokeOptions, sink: &mut S) ->
    Result<(), FixedStrokeExpandError<S::Error>> {
    if options.cap == LineCap::Round || options.join == LineJoin::Round {
        return Err(FixedStrokeExpandError::UnsupportedRound);
    }
    if points.iter().any(|point| !point_in_range(*point)) {
        return Err(FixedStrokeExpandError::CoordinateOutOfRange);
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
            return if closed || options.cap == LineCap::Butt {
                Ok(())
            } else { emit_square_point(point, options.width, sink) };
        };
    if closed {
        emit_join(first_point, last_direction, first_direction, options, sink)
    } else {
        emit_cap(first_point, first_direction, true, options, sink)?;
        emit_cap(last_point, last_direction, false, options, sink)
    }
}

fn point_in_range(point: Point<FixedScalar>) -> bool {
    [point.x.to_bits(), point.y.to_bits()].iter()
        .all(|value| value.unsigned_abs() <= FIXED_DEVICE_RAW_LIMIT as u32)
}

fn segment_at(points: &[Point<FixedScalar>], index: usize) ->
    (Point<FixedScalar>, Point<FixedScalar>) {
    if index + 1 < points.len() { (points[index], points[index + 1])
    } else { (points[points.len() - 1], points[0]) }
}

fn direction(from: Point<FixedScalar>, to: Point<FixedScalar>) -> Option<Direction> {
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

fn offset<E>(point: Point<FixedScalar>, dx: i64, dy: i64) ->
    Result<Point<FixedScalar>, FixedStrokeExpandError<E>> {
    let (x, y) = (point.x.to_bits() as i64 + dx, point.y.to_bits() as i64 + dy);
    if [x, y].iter().any(|value| value.unsigned_abs() > FIXED_DEVICE_RAW_LIMIT as u64) {
        return Err(FixedStrokeExpandError::CoordinateOutOfRange);
    }
    Ok((FixedScalar::from_bits(x as _), FixedScalar::from_bits(y as _)).into())
}

fn emit_segment_body<S: EdgeSink<FixedScalar>>(from: Point<FixedScalar>,
    to: Point<FixedScalar>, direction: Direction, width: i32, sink: &mut S) ->
    Result<(), FixedStrokeExpandError<S::Error>> {
    let (nx, ny) = normal(direction, width);
    emit_polygon(&[
        offset(from,  nx,  ny)?,
        offset(from, -nx, -ny)?,
        offset(to,   -nx, -ny)?,
        offset(to,    nx,  ny)?,
    ], sink)
}

fn emit_cap<S: EdgeSink<FixedScalar>>(point: Point<FixedScalar>, direction: Direction,
    start: bool, options: FixedStrokeOptions, sink: &mut S) ->
    Result<(), FixedStrokeExpandError<S::Error>> {
    if options.cap == LineCap::Butt { return Ok(()) }
    let sign = if start { -1 } else { 1 };
    let denominator = direction.length as i128 * 2;
    let (dx, dy) = (
        round_ratio(direction.dx as i128 * options.width as i128, denominator) * sign,
        round_ratio(direction.dy as i128 * options.width as i128, denominator) * sign,
    );
    let end = offset(point, dx, dy)?;
    emit_segment_body(point, end, direction, options.width, sink)
}

fn emit_square_point<S: EdgeSink<FixedScalar>>(point: Point<FixedScalar>, width: i32,
    sink: &mut S) -> Result<(), FixedStrokeExpandError<S::Error>> {
    let (low, high) = (-(width as i64) / 2, (width as i64 + 1) / 2);
    emit_polygon(&[
        offset(point, low, low)?,
        offset(point, high, low)?,
        offset(point, high, high)?,
        offset(point, low, high)?,
    ], sink)
}

fn emit_join<S: EdgeSink<FixedScalar>>(point: Point<FixedScalar>,
    before: Direction, after: Direction, options: FixedStrokeOptions, sink: &mut S) ->
    Result<(), FixedStrokeExpandError<S::Error>> {
    let cross = before.dx as i128 * after.dy as i128 -
                before.dy as i128 * after.dx as i128;
    if cross == 0 { return Ok(()) }
    let side = if cross > 0 { -1 } else { 1 };
    let (before_x, before_y) = normal(before, options.width);
    let (after_x, after_y) = normal(after, options.width);
    let before_outer = offset(point, before_x * side, before_y * side)?;
    let after_outer = offset(point, after_x * side, after_y * side)?;
    if options.join == LineJoin::Bevel {
        return emit_polygon(&[point, before_outer, after_outer], sink);
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
    let scale = 2_i128 * FixedScalar::ONE.to_bits() as i128;
    let limit = options.width as i128 * options.miter_limit as i128;
    if (dx * dx + dy * dy) * scale * scale <= limit * limit {
        emit_polygon(&[point, before_outer, miter, after_outer], sink)
    } else { emit_polygon(&[point, before_outer, after_outer], sink) }
}

fn emit_polygon<S: EdgeSink<FixedScalar>>(points: &[Point<FixedScalar>], sink: &mut S) ->
    Result<(), FixedStrokeExpandError<S::Error>> {
    for pair in points.windows(2) {
        if let Some(edge) = Edge::from_line(pair[0], pair[1]) {
            sink.edge(edge).map_err(FixedStrokeExpandError::Sink)?;
        }
    }
    if let (Some(&first), Some(&last)) = (points.first(), points.last())
        && let Some(edge) = Edge::from_line(last, first) {
        sink.edge(edge).map_err(FixedStrokeExpandError::Sink)?;
    }
    Ok(())
}

#[cfg(test)] mod tests { use super::*;
    use alloc::vec::Vec;
    use core::convert::Infallible;

    fn fixed(value: f32) -> FixedScalar { FixedScalar::from_num(value) }

    fn collect(points: &[(f32, f32)], closed: bool, options: FixedStrokeOptions) ->
        Vec<Edge<FixedScalar>> {
        let points: Vec<_> = points.iter().map(|&(x, y)| (fixed(x), fixed(y)).into()).collect();
        let mut edges = Vec::new();
        stroke_polyline_fixed(&points, closed, options, &mut |edge| {
            edges.push(edge); Ok::<_, Infallible>(())
        }).unwrap();
        edges
    }

    fn x_bounds(edges: &[Edge<FixedScalar>]) -> (FixedScalar, FixedScalar) {
        edges.iter().flat_map(|edge| [edge.upper.x, edge.lower.x])
            .fold((FixedScalar::MAX, FixedScalar::MIN),
                |(minimum, maximum), x| (minimum.min(x), maximum.max(x)))
    }

    #[test] fn fixed_line_caps_have_exact_device_bounds() {
        let options = FixedStrokeOptions::new(fixed(2.0)).unwrap();
        let butt = collect(&[(2.0, 3.0), (6.0, 3.0)], false, options);
        let square = collect(&[(2.0, 3.0), (6.0, 3.0)], false,
            options.with_cap(LineCap::Square));
        assert_eq!(x_bounds(&butt), (fixed(2.0), fixed(6.0)));
        assert_eq!(x_bounds(&square), (fixed(1.0), fixed(7.0)));
    }

    #[test] fn fixed_diagonal_offsets_track_the_f32_reference() {
        let options = FixedStrokeOptions::new(fixed(2.0)).unwrap();
        let actual = collect(&[(2.0, 2.0), (6.0, 6.0)], false, options);
        let mut expected = Vec::new();
        crate::stroke::stroke_line((2.0, 2.0).into(), (6.0, 6.0).into(),
            crate::stroke::StrokeOptions::new(2.0).unwrap(), &mut |edge| {
                expected.push(edge); Ok::<_, Infallible>(())
            }).unwrap();
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(&expected) {
            for (actual, expected) in [
                (actual.upper.x.to_num::<f32>(), expected.upper.x),
                (actual.upper.y.to_num::<f32>(), expected.upper.y),
                (actual.lower.x.to_num::<f32>(), expected.lower.x),
                (actual.lower.y.to_num::<f32>(), expected.lower.y),
            ] {
                assert!((actual - expected).abs() <= 1.0 / 128.0,
                    "actual={actual}, expected={expected}");
            }
        }
    }

    #[test] fn fixed_bevel_and_miter_joins_follow_limit_and_degenerate_rules() {
        let base = FixedStrokeOptions::new(fixed(2.0)).unwrap();
        let points = [(2.0, 4.0), (4.0, 4.0), (4.0, 6.0)];
        let bevel = collect(&points, false, base.with_join(LineJoin::Bevel));
        let miter = collect(&points, false, base);
        let fallback = collect(&points, false,
            base.with_miter_limit(FixedScalar::ONE).unwrap());
        let corner = (fixed(5.0), fixed(3.0)).into();
        let has_corner = |edges: &[Edge<FixedScalar>]| edges.iter()
            .any(|edge| [edge.upper, edge.lower].contains(&corner));
        assert!(!has_corner(&bevel));
        assert!(has_corner(&miter));
        assert!(!has_corner(&fallback));

        let plain = collect(&[(1.0, 2.0), (5.0, 2.0)], false, base);
        let repeated = collect(
            &[(1.0, 2.0), (1.0, 2.0), (5.0, 2.0), (5.0, 2.0)], false, base);
        assert_eq!(plain, repeated);
        assert!(!collect(&[(1.0, 2.0), (5.0, 2.0)], true, base).is_empty());
    }

    #[test] fn fixed_stroke_rejects_unsupported_and_out_of_range_before_writing() {
        assert_eq!(FixedStrokeOptions::new(FixedScalar::ZERO),
            Err(FixedStrokeError::NonPositiveWidth));
        let mut edges = Vec::new();
        let round = FixedStrokeOptions::default().with_cap(LineCap::Round);
        assert_eq!(stroke_line_fixed((fixed(0.0), fixed(0.0)).into(),
            (fixed(1.0), fixed(0.0)).into(), round, &mut |edge| {
                edges.push(edge); Ok::<_, Infallible>(())
            }), Err(FixedStrokeExpandError::UnsupportedRound));
        assert!(edges.is_empty());

        let outside = FixedScalar::from_bits(FIXED_DEVICE_RAW_LIMIT + 1);
        assert_eq!(stroke_line_fixed((outside, fixed(0.0)).into(),
            (fixed(1.0), fixed(0.0)).into(), FixedStrokeOptions::default(),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }),
            Err(FixedStrokeExpandError::CoordinateOutOfRange));
        assert!(edges.is_empty());
    }
}
