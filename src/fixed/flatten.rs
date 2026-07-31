//! Allocation-free Q24.8 curve flattening without floating point.

use crate::{edge::{EdgeSink, FillEdgeBuilder}, flatten::LineSink,
    geometry::{Affine, FIXED_DEVICE_RAW_LIMIT, FixedScalar, Path, PathError,
        PathSegment, Point}};

const STACK_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub struct FixedFlattenOptions {
    pub tolerance: FixedScalar,
    pub max_depth: u8,
}

impl Default for FixedFlattenOptions {
    fn default() -> Self {
        Self { tolerance: FixedScalar::from_bits(64), max_depth: 16 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)] pub enum FixedFlattenError<E> {
    NonPositiveTolerance, InvalidDepth, CoordinateOutOfRange,
    DepthLimit, InvalidPath(PathError), Sink(E),
}

#[derive(Clone, Copy, Debug, Default)] struct Quad {
    p0: Point<FixedScalar>, p1: Point<FixedScalar>, p2: Point<FixedScalar>,
}

#[derive(Clone, Copy, Debug, Default)] struct Cubic {
    p0: Point<FixedScalar>, p1: Point<FixedScalar>,
    p2: Point<FixedScalar>, p3: Point<FixedScalar>,
}

/// Transforms and flattens a fixed path into caller-consumed device-space lines.
pub fn flatten_path_fixed<S: LineSink<FixedScalar>>(path: &Path<FixedScalar>,
    transform: Affine<FixedScalar>, options: FixedFlattenOptions, sink: &mut S) ->
    Result<(), FixedFlattenError<S::Error>> {
    validate_options(options)?;
    let (mut current, mut subpath_start) = (None, None);
    for segment in path.segments() {
        match *segment {
            PathSegment::MoveTo(to) => {
                let to = transform_point(to, transform)?;
                if current.is_some() {
                    sink.end_subpath().map_err(FixedFlattenError::Sink)?;
                }
                sink.begin_subpath(to).map_err(FixedFlattenError::Sink)?;
                subpath_start = Some(to);
                current = Some(to);
            }
            PathSegment::LineTo(to) => {
                let to = transform_point(to, transform)?;
                let from = current.ok_or(
                    FixedFlattenError::InvalidPath(PathError::MissingMoveTo))?;
                emit_line(from, to, sink)?;
                current = Some(to);
            }
            PathSegment::QuadTo { ctrl, to } => {
                let (ctrl, to) = (transform_point(ctrl, transform)?,
                    transform_point(to, transform)?);
                let curve = Quad { p0: current.ok_or(
                    FixedFlattenError::InvalidPath(PathError::MissingMoveTo))?, p1: ctrl, p2: to };
                flatten_quad(curve, options, sink)?;
                current = Some(to);
            }
            PathSegment::CubicTo { ctrl1, ctrl2, to } => {
                let (ctrl1, ctrl2, to) = (transform_point(ctrl1, transform)?,
                    transform_point(ctrl2, transform)?, transform_point(to, transform)?);
                let curve = Cubic { p0: current.ok_or(
                    FixedFlattenError::InvalidPath(PathError::MissingMoveTo))?,
                    p1: ctrl1, p2: ctrl2, p3: to };
                flatten_cubic(curve, options, sink)?;
                current = Some(to);
            }
            PathSegment::Close => {
                let (from, to) = (current.ok_or(
                    FixedFlattenError::InvalidPath(PathError::MissingMoveTo))?,
                    subpath_start.ok_or(
                        FixedFlattenError::InvalidPath(PathError::MissingMoveTo))?);
                emit_line(from, to, sink)?;
                sink.close_subpath().map_err(FixedFlattenError::Sink)?;
                current = Some(to);
            }
        }
    }
    if current.is_some() { sink.end_subpath().map_err(FixedFlattenError::Sink)?; }
    Ok(())
}

/// Flattens a device-space fixed path and emits normalized fill edges.
pub fn build_fill_edges_fixed<S>(path: &Path<FixedScalar>, transform: Affine<FixedScalar>,
    options: FixedFlattenOptions, sink: &mut S) -> Result<(), FixedFlattenError<S::Error>>
    where S: EdgeSink<FixedScalar> {
    flatten_path_fixed(path, transform, options, &mut FillEdgeBuilder::new(sink))
}

fn transform_point<E>(point: Point<FixedScalar>, transform: Affine<FixedScalar>) ->
    Result<Point<FixedScalar>, FixedFlattenError<E>> {
    let point = transform.try_transform_point(point)
        .map_err(|_| FixedFlattenError::CoordinateOutOfRange)?;
    validate_point(point)?;
    Ok(point)
}

fn validate_options<E>(options: FixedFlattenOptions) -> Result<(), FixedFlattenError<E>> {
    if options.tolerance <= FixedScalar::ZERO {
        return Err(FixedFlattenError::NonPositiveTolerance);
    }
    if options.max_depth as usize >= STACK_CAPACITY {
        return Err(FixedFlattenError::InvalidDepth);
    }
    Ok(())
}

fn validate_point<E>(point: Point<FixedScalar>) -> Result<(), FixedFlattenError<E>> {
    if [point.x.to_bits(), point.y.to_bits()].iter()
        .any(|value| value.unsigned_abs() > FIXED_DEVICE_RAW_LIMIT as u32) {
        Err(FixedFlattenError::CoordinateOutOfRange)
    } else { Ok(()) }
}

fn emit_line<S: LineSink<FixedScalar>>(from: Point<FixedScalar>, to: Point<FixedScalar>,
    sink: &mut S) -> Result<(), FixedFlattenError<S::Error>> {
    if from != to { sink.line(from, to).map_err(FixedFlattenError::Sink)?; }
    Ok(())
}

fn flatten_quad<S: LineSink<FixedScalar>>(curve: Quad, options: FixedFlattenOptions,
    sink: &mut S) -> Result<(), FixedFlattenError<S::Error>> {
    let (mut stack, mut len) = ([(Quad::default(), 0_u8); STACK_CAPACITY], 1);
    stack[0] = (curve, 0);
    while len != 0 {
        len -= 1;
        let (curve, depth) = stack[len];
        if control_is_flat(curve.p0, curve.p2, curve.p1, options.tolerance) {
            emit_line(curve.p0, curve.p2, sink)?;
        } else {
            if depth == options.max_depth { return Err(FixedFlattenError::DepthLimit); }
            let (left, right) = split_quad(curve);
            stack[len] = (right, depth + 1);
            stack[len + 1] = (left, depth + 1);
            len += 2;
        }
    }
    Ok(())
}

fn flatten_cubic<S: LineSink<FixedScalar>>(curve: Cubic, options: FixedFlattenOptions,
    sink: &mut S) -> Result<(), FixedFlattenError<S::Error>> {
    let (mut stack, mut len) = ([(Cubic::default(), 0_u8); STACK_CAPACITY], 1);
    stack[0] = (curve, 0);
    while len != 0 {
        len -= 1;
        let (curve, depth) = stack[len];
        if control_is_flat(curve.p0, curve.p3, curve.p1, options.tolerance) &&
           control_is_flat(curve.p0, curve.p3, curve.p2, options.tolerance) {
            emit_line(curve.p0, curve.p3, sink)?;
        } else {
            if depth == options.max_depth { return Err(FixedFlattenError::DepthLimit); }
            let (left, right) = split_cubic(curve);
            stack[len] = (right, depth + 1);
            stack[len + 1] = (left, depth + 1);
            len += 2;
        }
    }
    Ok(())
}

fn control_is_flat(from: Point<FixedScalar>, to: Point<FixedScalar>,
    control: Point<FixedScalar>, tolerance: FixedScalar) -> bool {
    let (from_x, from_y, to_x, to_y, control_x, control_y) = (
        from.x.to_bits() as i128, from.y.to_bits() as i128,
        to.x.to_bits() as i128, to.y.to_bits() as i128,
        control.x.to_bits() as i128, control.y.to_bits() as i128,
    );
    let (dx, dy) = (to_x - from_x, to_y - from_y);
    let (cx, cy) = (control_x - from_x, control_y - from_y);
    let chord_squared = dx * dx + dy * dy;
    let tolerance_squared = tolerance.to_bits() as i128 * tolerance.to_bits() as i128;
    if chord_squared == 0 {
        return cx * cx + cy * cy <= tolerance_squared;
    }
    let projection = cx * dx + cy * dy;
    let cross = cx * dy - cy * dx;
    0 <= projection && projection <= chord_squared &&
        cross * cross <= tolerance_squared * chord_squared
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

fn midpoint(a: Point<FixedScalar>, b: Point<FixedScalar>) -> Point<FixedScalar> {
    let average = |a: FixedScalar, b: FixedScalar| {
        let sum = a.to_bits() as i64 + b.to_bits() as i64;
        FixedScalar::from_bits(if sum < 0 { ((sum - 1) / 2) as _ } else { ((sum + 1) / 2) as _ })
    };
    (average(a.x, b.x), average(a.y, b.y)).into()
}

#[cfg(test)] mod tests { use super::*;
    use alloc::vec::Vec;
    use core::convert::Infallible;
    use crate::geometry::PathBuilder;

    type FixedLine = (Point<FixedScalar>, Point<FixedScalar>);

    fn fixed(value: i32) -> FixedScalar { FixedScalar::from_num(value) }

    fn collect(path: &Path<FixedScalar>, options: FixedFlattenOptions) ->
        Result<Vec<FixedLine>, FixedFlattenError<Infallible>> {
        let mut lines = Vec::new();
        flatten_path_fixed(path, Affine::identity(), options,
            &mut |from, to| { lines.push((from, to)); Ok::<_, Infallible>(()) })?;
        Ok(lines)
    }

    #[test] fn fixed_lines_share_edge_normalization_and_winding() {
        let (zero, one) = (FixedScalar::ZERO, FixedScalar::ONE);
        assert_eq!(crate::edge::Edge::from_line(
            (zero, one).into(), (one, zero).into()),
            Some(crate::edge::Edge {
                upper: (one, zero).into(), lower: (zero, one).into(), winding: -1,
            }));
        assert_eq!(crate::edge::Edge::from_line(
            (zero, one).into(), (one, one).into()), None);
    }

    #[test] fn fixed_fill_builder_closes_curved_subpaths() {
        let (zero, one, two) =
            (FixedScalar::ZERO, FixedScalar::ONE, FixedScalar::from_num(2));
        let mut builder = PathBuilder::new();
        builder.move_to((zero, zero)).quad_to((one, two), (two, zero));
        let mut edges = Vec::new();
        build_fill_edges_fixed(&builder.build(), Affine::identity(),
            FixedFlattenOptions::default(),
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
        let lines = collect(&builder.build(), FixedFlattenOptions::default()).unwrap();
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
        let lines = collect(&builder.build(), FixedFlattenOptions::default()).unwrap();
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
            flatten_path_fixed(&path, transform, FixedFlattenOptions::default(),
                &mut |from, to| {
                    lines.push((from, to)); Ok::<_, Infallible>(())
                }).unwrap();
            lines
        };
        let identity = collect_with(Affine::identity());
        let scale = fixed(4);
        let scaled = collect_with(Affine::new(scale, FixedScalar::ZERO,
            FixedScalar::ZERO, scale, fixed(3), fixed(-2)));
        assert!(scaled.len() > identity.len());
        assert_eq!(scaled.first().unwrap().0, (fixed(3), fixed(-2)).into());
        assert_eq!(scaled.last().unwrap().1, (fixed(11), fixed(-2)).into());
    }

    #[test] fn midpoint_rounds_half_units_away_from_zero() {
        let raw = |value| FixedScalar::from_bits(value);
        assert_eq!(midpoint((raw(0), raw(0)).into(), (raw(1), raw(1)).into()),
            (raw(1), raw(1)).into());
        assert_eq!(midpoint((raw(0), raw(0)).into(), (raw(-1), raw(-1)).into()),
            (raw(-1), raw(-1)).into());
    }

    #[test] fn rejects_invalid_options_and_device_coordinates() {
        let path = PathBuilder::<FixedScalar>::new().build();
        let mut sink = |_, _| Ok::<_, Infallible>(());
        assert_eq!(flatten_path_fixed(&path, Affine::identity(), FixedFlattenOptions {
            tolerance: FixedScalar::ZERO, max_depth: 16,
        }, &mut sink), Err(FixedFlattenError::NonPositiveTolerance));
        assert_eq!(flatten_path_fixed(&path, Affine::identity(), FixedFlattenOptions {
            tolerance: FixedScalar::ONE, max_depth: STACK_CAPACITY as _,
        }, &mut sink), Err(FixedFlattenError::InvalidDepth));

        let outside = FixedScalar::from_bits(FIXED_DEVICE_RAW_LIMIT + 1);
        let mut builder = PathBuilder::new();
        builder.move_to((outside, FixedScalar::ZERO));
        assert_eq!(flatten_path_fixed(&builder.build(), Affine::identity(),
            FixedFlattenOptions::default(),
            &mut sink), Err(FixedFlattenError::CoordinateOutOfRange));

        let mut builder = PathBuilder::new();
        builder.move_to((FixedScalar::MAX, FixedScalar::MAX));
        let maximum = FixedScalar::MAX;
        let overflow = Affine::new(maximum, FixedScalar::ZERO, FixedScalar::ZERO,
            maximum, maximum, maximum);
        assert_eq!(flatten_path_fixed(&builder.build(), overflow,
            FixedFlattenOptions::default(), &mut sink),
            Err(FixedFlattenError::CoordinateOutOfRange));
    }

    #[test] fn depth_and_sink_failures_propagate() {
        let mut builder = PathBuilder::new();
        builder.move_to((fixed(0), fixed(0)))
            .quad_to((fixed(1), fixed(10)), (fixed(2), fixed(0)));
        let path = builder.build();
        assert_eq!(flatten_path_fixed(&path, Affine::identity(), FixedFlattenOptions {
            tolerance: FixedScalar::from_bits(1), max_depth: 0,
        }, &mut |_, _| Ok::<_, &'static str>(())),
            Err(FixedFlattenError::DepthLimit));
        assert_eq!(flatten_path_fixed(&path, Affine::identity(),
            FixedFlattenOptions::default(),
            &mut |_, _| Err("full")), Err(FixedFlattenError::Sink("full")));
    }
}
