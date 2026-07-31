//! Allocation-free curve flattening for the `f32` reference backend.

use crate::geometry::{Affine, Path, PathError, PathSegment, Point, Scalar};

const STACK_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlattenOptions {
    /// Maximum geometric deviation in device pixels.
    pub tolerance: f32,
    /// Maximum number of midpoint subdivisions for one source curve.
    pub max_depth: u8,
}

impl Default for FlattenOptions {
    fn default() -> Self { Self { tolerance: 0.25, max_depth: 16 } }
}

pub trait LineSink<T = Scalar> { type Error;
    fn begin_subpath(&mut self, _: Point<T>) -> Result<(), Self::Error> { Ok(()) }

    fn line(&mut self, from: Point<T>, to: Point<T>) -> Result<(), Self::Error>;

    /// Reports an explicit path close after its closing line has been emitted.
    fn close_subpath(&mut self) -> Result<(), Self::Error> { Ok(()) }

    fn end_subpath(&mut self) -> Result<(), Self::Error> { Ok(()) }
}

impl<T, E, F> LineSink<T> for F where F: FnMut(Point<T>, Point<T>) -> Result<(), E> {
    type Error = E;

    fn line(&mut self, from: Point<T>, to: Point<T>) -> Result<(), Self::Error> {
        self(from, to)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlattenError<E> {
    InvalidTolerance,
    InvalidDepth,
    NonFiniteCoordinate,
    DepthLimit,
    InvalidPath(PathError),
    Sink(E),
}

pub fn flatten_path<S>(path: &Path, transform: Affine, options: FlattenOptions,
    sink: &mut S) -> Result<(), FlattenError<S::Error>> where S: LineSink {
    validate_options(options)?;
    path.validate_finite().map_err(FlattenError::InvalidPath)?;
    let (mut current, mut subpath_start) = (None, None);

    for segment in path.segments() {
        match *segment {
            PathSegment::MoveTo(to) => {
                if current.is_some() {
                    sink.end_subpath().map_err(FlattenError::Sink)?;
                }
                let to = transformed(transform, to)?;
                sink.begin_subpath(to).map_err(FlattenError::Sink)?;
                subpath_start = Some(to);
                current = Some(to);
            }
            PathSegment::LineTo(to) => {
                let (from, to) = (
                    current.ok_or(FlattenError::InvalidPath(PathError::MissingMoveTo))?,
                    transformed(transform, to)?,
                );
                emit_line(from, to, sink)?;
                current = Some(to);
            }
            PathSegment::QuadTo { ctrl, to } => {
                let curve = Quad {
                    p0: current.ok_or(FlattenError::InvalidPath(PathError::MissingMoveTo))?,
                    p1: transformed(transform, ctrl)?,
                    p2: transformed(transform, to)?,
                };
                flatten_quad(curve, options, sink)?;
                current = Some(curve.p2);
            }
            PathSegment::CubicTo { ctrl1, ctrl2, to } => {
                let curve = Cubic {
                    p0: current.ok_or(FlattenError::InvalidPath(PathError::MissingMoveTo))?,
                    p1: transformed(transform, ctrl1)?,
                    p2: transformed(transform, ctrl2)?,
                    p3: transformed(transform, to)?,
                };
                flatten_cubic(curve, options, sink)?;
                current = Some(curve.p3);
            }
            PathSegment::Close => {
                let (from, to) = (
                    current.ok_or(FlattenError::InvalidPath(PathError::MissingMoveTo))?,
                    subpath_start.ok_or(FlattenError::InvalidPath(PathError::MissingMoveTo))?,
                );
                emit_line(from, to, sink)?;
                sink.close_subpath().map_err(FlattenError::Sink)?;
                current = Some(to);
            }
        }
    }
    if current.is_some() { sink.end_subpath().map_err(FlattenError::Sink)?; }
    Ok(())
}

fn validate_options<E>(options: FlattenOptions) -> Result<(), FlattenError<E>> {
    if !options.tolerance.is_finite() || options.tolerance <= 0.0 {
        return Err(FlattenError::InvalidTolerance);
    }
    if options.max_depth as usize >= STACK_CAPACITY {
        return Err(FlattenError::InvalidDepth);
    }   Ok(())
}

fn transformed<E>(transform: Affine, point: Point) -> Result<Point, FlattenError<E>> {
    let point = transform.transform_point(point);
    if  point.x.is_finite() && point.y.is_finite() { Ok(point) } else {
        Err(FlattenError::NonFiniteCoordinate)
    }
}

fn emit_line<S>(from: Point, to: Point, sink: &mut S) ->
    Result<(), FlattenError<S::Error>> where S: LineSink {
    if from != to { sink.line(from, to).map_err(FlattenError::Sink)?; }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)] struct Quad { p0: Point, p1: Point, p2: Point }

#[derive(Clone, Copy, Debug, Default)]
struct Cubic { p0: Point, p1: Point, p2: Point, p3: Point }

fn flatten_quad<S>(curve: Quad, options: FlattenOptions, sink: &mut S) ->
    Result<(), FlattenError<S::Error>> where S: LineSink {
    let (mut stack, mut len) = ([(Quad::default(), 0_u8); STACK_CAPACITY], 1);
    stack[0] = (curve, 0);

    while len != 0 {
        len -= 1;
        let (curve, depth) = stack[len];
        if quad_is_flat(curve, options.tolerance) {
            emit_line(curve.p0, curve.p2, sink)?;
        } else {
            if depth == options.max_depth { return Err(FlattenError::DepthLimit); }
            let (left, right) = split_quad(curve);
            stack[len] = (right, depth + 1);
            stack[len + 1] = (left, depth + 1);
            len += 2;
        }
    }   Ok(())
}

fn flatten_cubic<S>(curve: Cubic, options: FlattenOptions, sink: &mut S) ->
    Result<(), FlattenError<S::Error>> where S: LineSink {
    let (mut stack, mut len) = ([(Cubic::default(), 0_u8); STACK_CAPACITY], 1);
    stack[0] = (curve, 0);

    while len != 0 {    len -= 1;
        let (curve, depth) = stack[len];
        if cubic_is_flat(curve, options.tolerance) {
            emit_line(curve.p0, curve.p3, sink)?;
        } else {
            if depth == options.max_depth { return Err(FlattenError::DepthLimit); }
            let (left, right) = split_cubic(curve);
            stack[len] = (right, depth + 1);
            stack[len + 1] = (left, depth + 1);
            len += 2;
        }
    }   Ok(())
}

fn quad_is_flat(curve: Quad, tolerance: f32) -> bool {
    control_is_flat(curve.p0, curve.p2, curve.p1, tolerance)
}

fn cubic_is_flat(curve: Cubic, tolerance: f32) -> bool {
    control_is_flat(curve.p0, curve.p3, curve.p1, tolerance) &&
        control_is_flat(curve.p0, curve.p3, curve.p2, tolerance)
}

fn control_is_flat(from: Point, to: Point, control: Point, tolerance: f32) -> bool {
    let scale = [from.x, from.y, to.x, to.y, control.x, control.y, tolerance]
        .iter().fold(0.0_f32, |scale, value| scale.max(value.abs()));
    let normalize = |value: f32| value / scale;
    let (from, to, control, tolerance) = (
        Point::new(normalize(from.x), normalize(from.y)),
        Point::new(normalize(to.x), normalize(to.y)),
        Point::new(normalize(control.x), normalize(control.y)),
        normalize(tolerance),
    );
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let chord_len_sq = dx * dx + dy * dy;
    if chord_len_sq == 0.0 {
        let (cx, cy) = (control.x - from.x, control.y - from.y);
        cx * cx + cy * cy <= tolerance * tolerance
    } else {
        let (cx, cy) = (control.x - from.x, control.y - from.y);
        let projection = cx * dx + cy * dy;
        let cross = cx * dy - cy * dx;
        0.0 <= projection && projection <= chord_len_sq &&
            cross * cross <= tolerance * tolerance * chord_len_sq
    }
}

fn split_quad(curve: Quad) -> (Quad, Quad) {
    let p01 = midpoint(curve.p0, curve.p1);
    let p12 = midpoint(curve.p1, curve.p2);
    let center = midpoint(p01, p12);
    (Quad { p0: curve.p0, p1: p01, p2: center },
     Quad { p0: center, p1: p12, p2: curve.p2 },)
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

fn midpoint(a: Point, b: Point) -> Point {
    (a.x * 0.5 + b.x * 0.5, a.y * 0.5 + b.y * 0.5).into()
}

#[cfg(test)] mod tests { use super::*;
    use crate::geometry::PathBuilder;
    use core::convert::Infallible;
    use alloc::vec::Vec;

    fn collect(path: &Path, transform: Affine,
        options: FlattenOptions) -> Result<Vec<(Point, Point)>, FlattenError<Infallible>> {
        let mut lines = Vec::new();
        flatten_path(path, transform, options, &mut |from, to| {
            lines.push((from, to));
            Ok::<_, Infallible>(())
        })?;
        Ok(lines)
    }

    #[test] fn straight_curves_emit_one_directed_line() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0))
            .quad_to((1.0, 1.0), (2.0, 2.0))
            .cubic_to((3.0, 3.0), (4.0, 4.0), (5.0, 5.0));
        let lines = collect(&builder.build(), Affine::identity(),
            FlattenOptions::default()).unwrap();
        assert_eq!(lines, [((0.0, 0.0).into(), (2.0, 2.0).into()),
                           ((2.0, 2.0).into(), (5.0, 5.0).into())]);
    }

    #[test] fn curved_segments_preserve_order_and_exact_endpoints() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0))
            .cubic_to((0.0, 10.0), (10.0, 10.0), (10.0, 0.0));
        let lines = collect(&builder.build(), Affine::identity(),
            FlattenOptions { tolerance: 0.1, max_depth: 16 }).unwrap();
        assert!(lines.len() > 1);
        assert_eq!(lines.first().unwrap().0, (0.0, 0.0).into());
        assert_eq!(lines.last().unwrap().1, (10.0, 0.0).into());
        assert!(lines.windows(2).all(|pair| pair[0].1 == pair[1].0));
    }

    #[test] fn collinear_curve_that_reverses_direction_is_not_collapsed() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).quad_to((2.0, 0.0), (1.0, 0.0));
        let lines = collect(&builder.build(), Affine::identity(),
            FlattenOptions { tolerance: 0.1, max_depth: 16 }).unwrap();
        assert!(lines.len() > 1);
        assert!(lines.iter().any(|(from, to)| to.x < from.x));
    }

    #[test] fn tolerance_is_evaluated_after_transform() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).quad_to((0.5, 0.1), (1.0, 0.0));
        let path = builder.build();
        let options = FlattenOptions { tolerance: 0.2, max_depth: 16 };
        assert_eq!(collect(&path, Affine::identity(), options).unwrap().len(), 1);
        assert!(collect(&path, Affine::new(10.0, 0.0, 0.0, 10.0, 0.0, 0.0), options)
            .unwrap().len() > 1);
    }

    #[test] fn finite_extreme_coordinates_do_not_overflow_midpoint_or_flatness() {
        let large = 3.0e38;
        let center =   midpoint((large, large).into(), (large, large).into());
        assert!(center.x.is_finite() && center.y.is_finite());
        assert!(control_is_flat((large, large).into(), (large, large).into(),
                                (large, large).into(), 1.0));
    }

    #[test] fn invalid_options_depth_and_sink_fail_explicitly() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).quad_to((0.0, 1.0), (1.0, 1.0));
        let path = builder.build();
        assert_eq!(collect(&path, Affine::identity(),
                FlattenOptions { tolerance: 0.0, max_depth: 16 }),
            Err(FlattenError::InvalidTolerance),
        );
        assert_eq!(collect(&path, Affine::identity(),
                FlattenOptions { tolerance: 0.01, max_depth: 0 }),
            Err(FlattenError::DepthLimit),
        );

        let mut remaining = 0;
        let result = flatten_path(
            &path, Affine::identity(), FlattenOptions::default(),
            &mut |_, _| {
                if remaining == 0 { Err("full") } else { remaining -= 1; Ok(()) }
            },
        );
        assert_eq!(result, Err(FlattenError::Sink("full")));
    }
}
