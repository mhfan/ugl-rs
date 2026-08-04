//! Allocation-free Q24.8 curve flattening without floating point.

use crate::{common::geometry::{Affine, EdgeSink, FillEdgeBuilder, LineSink,
        Path, PathError, PathSegment, Point},
    fixed::{DEVICE_RAW_LIMIT, Scalar}};

const STACK_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub struct Options {
    pub tolerance: Scalar,
    pub max_depth: u8,
}

impl Default for Options {
    fn default() -> Self {
        Self { tolerance: Scalar::from_bits(64), max_depth: 16 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)] pub enum Error<E> {
    NonPositiveTolerance, InvalidDepth, CoordinateOutOfRange,
    DepthLimit, InvalidPath(PathError), Sink(E),
}

#[derive(Clone, Copy, Debug, Default)] struct Quad {
    p0: Point<Scalar>, p1: Point<Scalar>, p2: Point<Scalar>,
}

#[derive(Clone, Copy, Debug, Default)] struct Cubic {
    p0: Point<Scalar>, p1: Point<Scalar>,
    p2: Point<Scalar>, p3: Point<Scalar>,
}

/// Transforms and flattens a fixed path into caller-consumed device-space lines.
pub fn flatten_path<S: LineSink<Scalar>>(path: &Path<Scalar>,
    transform: Affine<Scalar>, options: Options, sink: &mut S) ->
    Result<(), Error<S::Error>> {
    validate_options(options)?;
    let (mut current, mut subpath_start) = (None, None);
    for segment in path.segments() {
        match *segment {
            PathSegment::MoveTo(to) => {
                let to = transform_point(to, transform)?;
                if current.is_some() {
                    sink.end_subpath().map_err(Error::Sink)?;
                }
                sink.begin_subpath(to).map_err(Error::Sink)?;
                subpath_start = Some(to);
                current = Some(to);
            }
            PathSegment::LineTo(to) => {
                let to = transform_point(to, transform)?;
                let from = current.ok_or(
                    Error::InvalidPath(PathError::MissingMoveTo))?;
                emit_line(from, to, sink)?;
                current = Some(to);
            }
            PathSegment::QuadTo { ctrl, to } => {
                let (ctrl, to) = (transform_point(ctrl, transform)?,
                    transform_point(to, transform)?);
                let curve = Quad { p0: current.ok_or(
                    Error::InvalidPath(PathError::MissingMoveTo))?, p1: ctrl, p2: to };
                flatten_quad(curve, options, sink)?;
                current = Some(to);
            }
            PathSegment::CubicTo { ctrl1, ctrl2, to } => {
                let (ctrl1, ctrl2, to) = (transform_point(ctrl1, transform)?,
                    transform_point(ctrl2, transform)?, transform_point(to, transform)?);
                let curve = Cubic { p0: current.ok_or(
                    Error::InvalidPath(PathError::MissingMoveTo))?,
                    p1: ctrl1, p2: ctrl2, p3: to };
                flatten_cubic(curve, options, sink)?;
                current = Some(to);
            }
            PathSegment::Close => {
                let (from, to) = (current.ok_or(
                    Error::InvalidPath(PathError::MissingMoveTo))?,
                    subpath_start.ok_or(
                        Error::InvalidPath(PathError::MissingMoveTo))?);
                emit_line(from, to, sink)?;
                sink.close_subpath().map_err(Error::Sink)?;
                current = Some(to);
            }
        }
    }
    if current.is_some() { sink.end_subpath().map_err(Error::Sink)?; }
    Ok(())
}

/// Flattens a device-space fixed path and emits normalized fill edges.
pub fn build_fill_edges<S>(path: &Path<Scalar>, transform: Affine<Scalar>,
    options: Options, sink: &mut S) -> Result<(), Error<S::Error>>
    where S: EdgeSink<Scalar> {
    flatten_path(path, transform, options, &mut FillEdgeBuilder::new(sink))
}

fn transform_point<E>(point: Point<Scalar>, transform: Affine<Scalar>) ->
    Result<Point<Scalar>, Error<E>> {
    let point = transform.try_transform_point(point)
        .map_err(|_| Error::CoordinateOutOfRange)?;
    validate_point(point)?;
    Ok(point)
}

fn validate_options<E>(options: Options) -> Result<(), Error<E>> {
    if options.tolerance <= Scalar::ZERO {
        return Err(Error::NonPositiveTolerance);
    }
    if options.max_depth as usize >= STACK_CAPACITY {
        return Err(Error::InvalidDepth);
    }
    Ok(())
}

fn validate_point<E>(point: Point<Scalar>) -> Result<(), Error<E>> {
    if [point.x.to_bits(), point.y.to_bits()].iter()
        .any(|value| value.unsigned_abs() > DEVICE_RAW_LIMIT as u32) {
        Err(Error::CoordinateOutOfRange)
    } else { Ok(()) }
}

fn emit_line<S: LineSink<Scalar>>(from: Point<Scalar>, to: Point<Scalar>,
    sink: &mut S) -> Result<(), Error<S::Error>> {
    if from != to { sink.line(from, to).map_err(Error::Sink)?; }
    Ok(())
}

fn flatten_quad<S: LineSink<Scalar>>(curve: Quad, options: Options,
    sink: &mut S) -> Result<(), Error<S::Error>> {
    let (mut stack, mut len) = ([(Quad::default(), 0_u8); STACK_CAPACITY], 1);
    stack[0] = (curve, 0);
    while len != 0 {
        len -= 1;
        let (curve, depth) = stack[len];
        if control_is_flat(curve.p0, curve.p2, curve.p1, options.tolerance) {
            emit_line(curve.p0, curve.p2, sink)?;
        } else {
            if depth == options.max_depth { return Err(Error::DepthLimit); }
            let (left, right) = split_quad(curve);
            stack[len] = (right, depth + 1);
            stack[len + 1] = (left, depth + 1);
            len += 2;
        }
    }
    Ok(())
}

fn flatten_cubic<S: LineSink<Scalar>>(curve: Cubic, options: Options,
    sink: &mut S) -> Result<(), Error<S::Error>> {
    let (mut stack, mut len) = ([(Cubic::default(), 0_u8); STACK_CAPACITY], 1);
    stack[0] = (curve, 0);
    while len != 0 {
        len -= 1;
        let (curve, depth) = stack[len];
        if control_is_flat(curve.p0, curve.p3, curve.p1, options.tolerance) &&
           control_is_flat(curve.p0, curve.p3, curve.p2, options.tolerance) {
            emit_line(curve.p0, curve.p3, sink)?;
        } else {
            if depth == options.max_depth { return Err(Error::DepthLimit); }
            let (left, right) = split_cubic(curve);
            stack[len] = (right, depth + 1);
            stack[len + 1] = (left, depth + 1);
            len += 2;
        }
    }
    Ok(())
}

fn control_is_flat(from: Point<Scalar>, to: Point<Scalar>,
    control: Point<Scalar>, tolerance: Scalar) -> bool {
    let (from_x, from_y, to_x, to_y, control_x, control_y) = (
        from.x.to_bits() as i64, from.y.to_bits() as i64,
        to.x.to_bits() as i64, to.y.to_bits() as i64,
        control.x.to_bits() as i64, control.y.to_bits() as i64,
    );
    let (dx, dy) = (to_x - from_x, to_y - from_y);
    let (cx, cy) = (control_x - from_x, control_y - from_y);
    let chord_squared = dx * dx + dy * dy;
    let tolerance_squared = tolerance.to_bits() as i64 * tolerance.to_bits() as i64;
    if chord_squared == 0 {
        return cx * cx + cy * cy <= tolerance_squared;
    }
    let projection = cx * dx + cy * dy;
    let cross = cx * dy - cy * dx;
    if projection < 0 || projection > chord_squared { return false; }
    if let (Some(distance), Some(limit)) =
        (cross.checked_mul(cross), tolerance_squared.checked_mul(chord_squared)) {
        distance <= limit
    } else {
        cross as i128 * cross as i128 <=
            tolerance_squared as i128 * chord_squared as i128
    }
}

fn split_quad(curve: Quad) -> (Quad, Quad) {
    let p01 = midpoint(curve.p0, curve.p1);
    let p12 = midpoint(curve.p1, curve.p2);
    let center = midpoint(p01, p12);
    (Quad { p0: curve.p0, p1: p01, p2: center },
     Quad { p0: center, p1: p12, p2: curve.p2 })
}

fn split_cubic(curve: Cubic) -> (Cubic, Cubic) {
    let p01 = midpoint(curve.p0, curve.p1);
    let p12 = midpoint(curve.p1, curve.p2);
    let p23 = midpoint(curve.p2, curve.p3);
    let p012 = midpoint(p01, p12);
    let p123 = midpoint(p12, p23);
    let center = midpoint(p012, p123);
    (Cubic { p0: curve.p0, p1: p01, p2: p012, p3: center },
     Cubic { p0: center, p1: p123, p2: p23, p3: curve.p3 })
}

fn midpoint(a: Point<Scalar>, b: Point<Scalar>) -> Point<Scalar> {
    let average = |a: Scalar, b: Scalar| {
        let sum = a.to_bits() + b.to_bits();
        Scalar::from_bits(if sum < 0 { (sum - 1) / 2 } else { (sum + 1) / 2 })
    };
    (average(a.x, b.x), average(a.y, b.y)).into()
}

#[cfg(test)] mod tests { use super::*;
    use alloc::vec::Vec;
    use core::convert::Infallible;
    use crate::common::geometry::{Edge, PathBuilder};

    type Line = (Point<Scalar>, Point<Scalar>);

    fn fixed(value: i32) -> Scalar { Scalar::from_num(value) }

    fn collect(path: &Path<Scalar>, options: Options) ->
        Result<Vec<Line>, Error<Infallible>> {
        let mut lines = Vec::new();
        flatten_path(path, Affine::identity(), options,
            &mut |from, to| { lines.push((from, to)); Ok::<_, Infallible>(()) })?;
        Ok(lines)
    }

    #[test] fn lines_share_edge_normalization_and_winding() {
        let (zero, one) = (Scalar::ZERO, Scalar::ONE);
        assert_eq!(Edge::from_line(
            (zero, one).into(), (one, zero).into()),
            Some(Edge {
                upper: (one, zero).into(), lower: (zero, one).into(), winding: -1,
            }));
        assert_eq!(Edge::from_line(
            (zero, one).into(), (one, one).into()), None);
    }

    #[test] fn fill_builder_closes_curved_subpaths() {
        let (zero, one, two) =
            (Scalar::ZERO, Scalar::ONE, Scalar::from_num(2));
        let mut builder = PathBuilder::new();
        builder.move_to((zero, zero)).quad_to((one, two), (two, zero));
        let mut edges = Vec::new();
        build_fill_edges(&builder.build(), Affine::identity(),
            Options::default(),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }).unwrap();
        assert!(!edges.is_empty());
        assert_eq!(edges.iter().map(|edge| edge.winding as i32).sum::<i32>(), 0);
    }

    #[test] fn straight_curves_emit_their_exact_endpoints_once() {
        let mut builder = PathBuilder::new();
        builder.move_to((fixed(0), fixed(0)))
            .quad_to((fixed(2), fixed(2)), (fixed(4), fixed(4)))
            .cubic_to((fixed(5), fixed(5)), (fixed(6), fixed(6)),
                (fixed(8), fixed(8)));
        let lines = collect(&builder.build(), Options::default()).unwrap();
        assert_eq!(lines, [
            ((fixed(0), fixed(0)).into(), (fixed(4), fixed(4)).into()),
            ((fixed(4), fixed(4)).into(), (fixed(8), fixed(8)).into()),
        ]);
    }

    #[test] fn curved_cubic_is_ordered_and_contiguous() {
        let mut builder = PathBuilder::new();
        let (start, end) = ((fixed(0), fixed(0)).into(), (fixed(10), fixed(0)).into());
        builder.move_to(start).cubic_to(
            (fixed(0), fixed(10)), (fixed(10), fixed(10)), end);
        let lines = collect(&builder.build(), Options::default()).unwrap();
        assert!(lines.len() > 1);
        assert_eq!(lines.first().unwrap().0, start);
        assert_eq!(lines.last().unwrap().1, end);
        assert!(lines.windows(2).all(|pair| pair[0].1 == pair[1].0));
    }

    #[test] fn transform_is_applied_before_device_space_flatness() {
        let mut builder = PathBuilder::new();
        builder.move_to((fixed(0), fixed(0))).quad_to(
            (fixed(1), fixed(1)), (fixed(2), fixed(0)));
        let path = builder.build();
        let collect_with = |transform| {
            let mut lines = Vec::new();
            flatten_path(&path, transform, Options::default(),
                &mut |from, to| {
                    lines.push((from, to)); Ok::<_, Infallible>(())
                }).unwrap();
            lines
        };
        let identity = collect_with(Affine::identity());
        let scale = fixed(4);
        let scaled = collect_with(Affine::new(scale, Scalar::ZERO,
            Scalar::ZERO, scale, fixed(3), fixed(-2)));
        assert!(scaled.len() > identity.len());
        assert_eq!(scaled.first().unwrap().0, (fixed(3), fixed(-2)).into());
        assert_eq!(scaled.last().unwrap().1, (fixed(11), fixed(-2)).into());
    }

    #[test] fn midpoint_rounds_half_units_away_from_zero() {
        let raw = |value| Scalar::from_bits(value);
        assert_eq!(midpoint((raw(0), raw(0)).into(), (raw(1), raw(1)).into()),
            (raw(1), raw(1)).into());
        assert_eq!(midpoint((raw(0), raw(0)).into(), (raw(-1), raw(-1)).into()),
            (raw(-1), raw(-1)).into());
    }

    #[test] fn rejects_invalid_options_and_device_coordinates() {
        let path = PathBuilder::<Scalar>::new().build();
        let mut sink = |_, _| Ok::<_, Infallible>(());
        assert_eq!(flatten_path(&path, Affine::identity(), Options {
            tolerance: Scalar::ZERO, max_depth: 16,
        }, &mut sink), Err(Error::NonPositiveTolerance));
        assert_eq!(flatten_path(&path, Affine::identity(), Options {
            tolerance: Scalar::ONE, max_depth: STACK_CAPACITY as _,
        }, &mut sink), Err(Error::InvalidDepth));

        let outside = Scalar::from_bits(DEVICE_RAW_LIMIT + 1);
        let mut builder = PathBuilder::new();
        builder.move_to((outside, Scalar::ZERO));
        assert_eq!(flatten_path(&builder.build(), Affine::identity(),
            Options::default(),
            &mut sink), Err(Error::CoordinateOutOfRange));

        let mut builder = PathBuilder::new();
        builder.move_to((Scalar::MAX, Scalar::MAX));
        let maximum = Scalar::MAX;
        let overflow = Affine::new(maximum, Scalar::ZERO, Scalar::ZERO,
            maximum, maximum, maximum);
        assert_eq!(flatten_path(&builder.build(), overflow,
            Options::default(), &mut sink),
            Err(Error::CoordinateOutOfRange));
    }

    #[test] fn depth_and_sink_failures_propagate() {
        let mut builder = PathBuilder::new();
        builder.move_to((fixed(0), fixed(0)))
            .quad_to((fixed(1), fixed(10)), (fixed(2), fixed(0)));
        let path = builder.build();
        assert_eq!(flatten_path(&path, Affine::identity(), Options {
            tolerance: Scalar::from_bits(1), max_depth: 0,
        }, &mut |_, _| Ok::<_, &'static str>(())),
            Err(Error::DepthLimit));
        assert_eq!(flatten_path(&path, Affine::identity(),
            Options::default(),
            &mut |_, _| Err("full")), Err(Error::Sink("full")));
    }
}
