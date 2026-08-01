//! Geometry shared by floating-point and fixed-point render backends.
//!
//! Containers are generic, while rasterization algorithms initially operate on
//! [`f32`]. A renderer can consume [`Path::segments`] or any independently
//! stored slice of [`PathSegment`], which leaves room for static/fixed-capacity
//! path storage on systems without a heap.

//  Point/Line/(Bezier)Curve, Shapes (Triangle/Rectangle/Polygon/Ellipse/Circle/Arc)
//  Fill(solid/linear/radial/conic/texture)/Stroke(width/cap/join/dash)

use alloc::vec::Vec;
use core::cmp::Ordering;

/// Default coordinate type for shared geometry; this alias does not enable the f32 backend.
pub type Scalar = f32;

pub trait ScalarConstants { const ZERO: Self; const ONE: Self; }
impl ScalarConstants for f32 { const ZERO: Self = 0.0; const ONE: Self = 1.0; }

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Point<T = Scalar> { pub x: T, pub y: T, }

impl<T> Point<T> { pub const fn new(x: T, y: T) -> Self { Self { x, y } } }
impl<T> From<(T, T)> for Point<T> { fn from((x, y): (T, T)) -> Self { Self::new(x, y) } }

/// Axis-aligned rectangle with ordered, finite-or-orderable boundaries.
///
/// ```
/// use ugl_rs::common::geometry::Rect;
///
/// let rect = Rect::from_ltrb(1.0, 2.0, 3.0, 4.0).unwrap();
/// assert_eq!((rect.min(), rect.max()), ((1.0, 2.0).into(), (3.0, 4.0).into()));
/// assert!(Rect::from_ltrb(3.0, 2.0, 1.0, 4.0).is_none());
/// assert!(Rect::from_ltrb(1.0, f32::NAN, 3.0, 4.0).is_none());
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rect<T = Scalar> { min: Point<T>, max: Point<T> }

impl<T> Rect<T> where T: Copy + PartialOrd {
    pub fn from_ltrb(left: T, top: T, right: T, bottom: T) -> Option<Self> {
        if left <= right && top <= bottom {
            Some(Self { min: (left, top).into(), max: (right, bottom).into() })
        } else { None }
    }

    pub fn min(&self) -> Point<T> { self.min }
    pub fn max(&self) -> Point<T> { self.max }
    pub fn left(&self) -> T { self.min.x }
    pub fn  top(&self) -> T { self.min.y }
    pub fn  right(&self) -> T { self.max.x }
    pub fn bottom(&self) -> T { self.max.y }
}

/// A 2D affine transform using column-vector convention.
///
/// `x' = a*x + c*y + e`, `y' = b*x + d*y + f`.
///
/// ```
/// use ugl_rs::common::geometry::Affine;
///
/// let transform = Affine::new(2.0, 0.5, -1.0, 3.0, 4.0, -2.0);
/// assert_eq!(transform.transform_point((3.0, 2.0).into()), (8.0, 5.5).into());
/// assert_eq!(transform.transform_vector((3.0, 2.0).into()), (4.0, 7.5).into());
/// let restored = transform.inverse().unwrap()
///     .transform_point(transform.transform_point((3.0, 2.0).into()));
/// assert!((restored.x - 3.0).abs() < 1e-6 && (restored.y - 2.0).abs() < 1e-6);
/// assert!(Affine::new(1.0, 2.0, 2.0, 4.0, 0.0, 0.0).inverse().is_none());
/// ```
#[derive(Clone, Copy, Debug, PartialEq)] pub struct Affine<T = Scalar> {
    pub a: T, pub b: T, pub c: T, pub d: T, pub e: T, pub f: T,
}

impl<T> Affine<T> {
    pub const fn new(a: T, b: T, c: T, d: T, e: T, f: T) -> Self {
        Self { a, b, c, d, e, f }
    }
}

impl<T> Affine<T> where T: Copy + ScalarConstants {
    pub fn identity() -> Self {
        Self::new(T::ONE, T::ZERO, T::ZERO, T::ONE, T::ZERO, T::ZERO)
    }

    pub fn translate(x: T, y: T) -> Self {
        Self::new(T::ONE, T::ZERO, T::ZERO, T::ONE, x, y)
    }
}

impl Affine<f32> {
    pub fn transform_point(&self, point: Point) -> Point {
        (self.a * point.x + self.c * point.y + self.e,
         self.b * point.x + self.d * point.y + self.f).into()
    }

    pub fn transform_vector(&self, vector: Point) -> Point {
        (self.a * vector.x + self.c * vector.y,
         self.b * vector.x + self.d * vector.y).into()
    }

    /// Returns the inverse transform, or `None` for non-finite or singular matrices.
    pub fn inverse(self) -> Option<Self> {
        if ![self.a, self.b, self.c, self.d, self.e, self.f]
            .into_iter().all(f32::is_finite) { return None; }
        let determinant = self.a * self.d - self.b * self.c;
        if determinant == 0.0 || !determinant.is_finite() { return None; }
        let inverse = Self {
            a:  self.d / determinant,  b: -self.b  / determinant,
            c: -self.c / determinant,  d:  self.a  / determinant,
            e: (self.c * self.f - self.d * self.e) / determinant,
            f: (self.b * self.e - self.a * self.f) / determinant,
        };
        [inverse.a, inverse.b, inverse.c, inverse.d, inverse.e, inverse.f]
            .into_iter().all(f32::is_finite).then_some(inverse)
    }
}

impl<T> Default for Affine<T> where T: Copy + ScalarConstants {
    fn default() -> Self { Self::identity() }
}

#[derive(Clone, Copy, Debug, PartialEq)] pub enum PathSegment<T = Scalar> {
    MoveTo(Point<T>), LineTo(Point<T>), Close,
    QuadTo  { ctrl:  Point<T>, to: Point<T> },
    CubicTo { ctrl1: Point<T>, ctrl2: Point<T>, to: Point<T> },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path<T = Scalar> { segments: Vec<PathSegment<T>>, }

impl<T> Path<T> {
    /// Validates and takes ownership of a segment sequence.
    ///
    /// The source can be assembled in fixed-capacity storage before conversion:
    ///
    /// ```
    /// use ugl_rs::common::geometry::{Path, PathSegment};
    ///
    /// let segments = [
    ///     PathSegment::MoveTo((0_i32, 0_i32).into()),
    ///     PathSegment::LineTo((256, 0).into()),
    ///     PathSegment::Close,
    /// ];
    /// let path = Path::from_segments(segments.to_vec()).unwrap();
    /// assert_eq!(path.segments(), &segments);
    /// ```
    pub fn  from_segments( segments: Vec<PathSegment<T>>) -> Result<Self, PathError> {
        validate_segments(&segments)?;
        Ok(Self { segments })
    }

    //pub fn transformed(&self, transform: Affine<T>) -> Self

    pub fn segments(&self) -> &[PathSegment<T>] { &self.segments }
    pub fn into_segments(self) -> Vec<PathSegment<T>> { self.segments }
    pub fn is_empty(&self) -> bool { self.segments.is_empty() }
    pub fn len(&self) -> usize { self.segments.len() }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathError { MissingMoveTo, NonFiniteCoordinate, }

impl core::fmt::Display for PathError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingMoveTo =>
                formatter.write_str("a drawing command requires an active subpath"),
            Self::NonFiniteCoordinate =>
                formatter.write_str("floating-point path coordinates must be finite"),
        }
    }
}

/// Builds paths while ensuring every drawing command has an active subpath.
///
/// A drawing command on an empty builder starts at its destination, and
/// repeated [`close`](Self::close) calls are idempotent:
///
/// ```
/// use ugl_rs::common::geometry::{PathBuilder, PathSegment};
///
/// let mut path = PathBuilder::<f32>::new();
/// path.close().line_to((1.0, 2.0));
/// assert_eq!(path.build().segments(),
///     &[PathSegment::MoveTo((1.0, 2.0).into())]);
///
/// let mut path = PathBuilder::<f32>::new();
/// path.move_to((0.0, 0.0)).line_to((1.0, 0.0)).close().close();
/// assert_eq!(path.build().segments(), &[
///     PathSegment::MoveTo((0.0, 0.0).into()),
///     PathSegment::LineTo((1.0, 0.0).into()),
///     PathSegment::Close,
/// ]);
///
/// let mut path = PathBuilder::<f32>::new();
/// path.quad_to((1.0, 1.0), (2.0, 2.0));
/// assert_eq!(path.build().segments(),
///     &[PathSegment::MoveTo((2.0, 2.0).into())]);
///
/// let mut path = PathBuilder::<f32>::new();
/// path.cubic_to((1.0, 1.0), (2.0, 2.0), (3.0, 3.0));
/// assert_eq!(path.build().segments(),
///     &[PathSegment::MoveTo((3.0, 3.0).into())]);
/// ```
#[derive(Clone, Debug, Default)] pub struct PathBuilder<T = Scalar> {
    segments: Vec<PathSegment<T>>, has_current_subpath: bool,
}

impl<T> PathBuilder<T> {
    pub fn new() -> Self { Self { segments: Vec::new(), has_current_subpath: false } }

    pub fn with_capacity(capacity: usize) -> Self {
        Self { segments: Vec::with_capacity(capacity), has_current_subpath: false }
    }

    pub fn move_to(&mut self, point: impl Into<Point<T>>) -> &mut Self {
        self.segments.push(PathSegment::MoveTo(point.into()));
        self.has_current_subpath = true;    self
    }

    /// Adds a line, or starts a subpath at `point` when the path is empty.
    pub fn line_to(&mut self, point: impl Into<Point<T>>) -> &mut Self {
        let point = point.into();
        if !self.has_current_subpath { return self.move_to(point); }
        self.segments.push(PathSegment::LineTo(point));     self
    }

    pub fn quad_to(&mut self, ctrl: impl Into<Point<T>>,
                                to: impl Into<Point<T>>) -> &mut Self {
        let (ctrl, to) = (ctrl.into(), to.into());
        if !self.has_current_subpath { return self.move_to(to); }
        self.segments.push(PathSegment::QuadTo { ctrl, to });   self
    }

    pub fn cubic_to(&mut self, ctrl1: impl Into<Point<T>>, ctrl2: impl Into<Point<T>>,
        to: impl Into<Point<T>>) -> &mut Self {
        let (ctrl1, ctrl2, to) = (ctrl1.into(), ctrl2.into(), to.into());
        if !self.has_current_subpath { return self.move_to(to); }
        self.segments.push(PathSegment::CubicTo { ctrl1, ctrl2, to, });     self
    }

    /// Closes the active subpath; an empty path is unchanged.
    pub fn close(&mut self) -> &mut Self {
        if !self.has_current_subpath { return self; }
        if !matches!(self.segments.last(), Some(PathSegment::Close)) {
            self.segments.push(PathSegment::Close);
        }   self
    }

    pub fn build(self) -> Path<T> { Path { segments: self.segments } }
}

impl Path<f32> {
    pub fn validate_finite(&self) -> Result<(), PathError> {
        let point_is_finite = |point: Point| point.x.is_finite() && point.y.is_finite();
        for segment in self.segments() {
            let finite = match segment {
                PathSegment::MoveTo(p) | PathSegment::LineTo(p) => point_is_finite(*p),
                PathSegment::QuadTo { ctrl, to } =>
                    point_is_finite(*ctrl) && point_is_finite(*to),
                PathSegment::CubicTo { ctrl1, ctrl2, to } =>
                    point_is_finite(*ctrl1) && point_is_finite(*ctrl2) && point_is_finite(*to),
                PathSegment::Close => true,
            };
            if !finite { return Err(PathError::NonFiniteCoordinate); }
        }   Ok(())
    }
}

impl PathBuilder<f32> {
    pub fn build_checked(self) -> Result<Path, PathError> {
        let path = self.build(); path.validate_finite()?; Ok(path)
    }
}

/// Receives flattened path lines while preserving subpath boundaries.
pub trait LineSink<T = Scalar> { type Error;
    fn begin_subpath(&mut self, _: Point<T>) -> Result<(), Self::Error> { Ok(()) }
    fn line(&mut self, from: Point<T>, to: Point<T>) -> Result<(), Self::Error>;
    fn close_subpath(&mut self) -> Result<(), Self::Error> { Ok(()) }
    fn end_subpath(&mut self) -> Result<(), Self::Error> { Ok(()) }
}

impl<T, E, F> LineSink<T> for F where F: FnMut(Point<T>, Point<T>) -> Result<(), E> {
    type Error = E;
    fn line(&mut self, from: Point<T>, to: Point<T>) -> Result<(), Self::Error> {
        self(from, to)
    }
}

/// A non-horizontal edge normalized to increasing device-space `y`.
///
/// `winding` preserves the source direction: `1` for downward and `-1` for
/// upward in the device coordinate system. Horizontal lines do not produce an
/// edge because they contribute no winding crossing.
#[derive(Clone, Copy, Debug, Default, PartialEq)] pub struct Edge<T = Scalar> {
    pub upper: Point<T>, pub lower: Point<T>, pub winding: i8,
}

impl<T> Edge<T> where T: Copy + PartialOrd {
    pub(crate) fn from_line(from: Point<T>, to: Point<T>) -> Option<Self> {
        match from.y.partial_cmp(&to.y)? {
            Ordering::Less => Some(Self { upper: from, lower: to, winding: 1 }),
            Ordering::Greater => Some(Self { upper: to, lower: from, winding: -1 }),
            Ordering::Equal => None,
        }
    }
}

pub trait EdgeSink<T = Scalar> { type Error;
    fn edge(&mut self, edge: Edge<T>) -> Result<(), Self::Error>;
}

impl<T, E, F> EdgeSink<T> for F where F: FnMut(Edge<T>) -> Result<(), E> {
    type Error = E;
    fn edge(&mut self, edge: Edge<T>) -> Result<(), Self::Error> { self(edge) }
}

pub(crate) struct FillEdgeBuilder<'a, S, T = Scalar> {
    sink: &'a mut S, start: Option<Point<T>>, current: Option<Point<T>>,
}

impl<'a, S, T> FillEdgeBuilder<'a, S, T> {
    pub(crate) fn new(sink: &'a mut S) -> Self {
        Self { sink, start: None, current: None }
    }
}

impl<S, T> LineSink<T> for FillEdgeBuilder<'_, S, T>
    where S: EdgeSink<T>, T: Copy + PartialOrd {
    type Error = S::Error;

    fn begin_subpath(&mut self, at: Point<T>) -> Result<(), Self::Error> {
        self.start = Some(at); self.current = Some(at); Ok(())
    }

    fn line(&mut self, from: Point<T>, to: Point<T>) -> Result<(), Self::Error> {
        self.emit(from, to)?; self.current = Some(to); Ok(())
    }

    fn end_subpath(&mut self) -> Result<(), Self::Error> {
        if let (Some(from), Some(to)) = (self.current, self.start) {
            self.emit(from, to)?;
        }
        self.start = None; self.current = None; Ok(())
    }
}

impl<S, T> FillEdgeBuilder<'_, S, T> where S: EdgeSink<T>, T: Copy + PartialOrd {
    fn emit(&mut self, from: Point<T>, to: Point<T>) -> Result<(), S::Error> {
        if let Some(edge) = Edge::from_line(from, to) { self.sink.edge(edge) } else { Ok(()) }
    }
}

fn validate_segments<T>(segments: &[PathSegment<T>]) -> Result<(), PathError> {
    let mut has_current_subpath = false;
    for segment in segments {
        match segment {
            PathSegment::MoveTo(_) => has_current_subpath = true,
            PathSegment::LineTo(_)      | PathSegment::QuadTo { .. } |
            PathSegment::CubicTo { .. } | PathSegment::Close if !has_current_subpath => {
                return Err(PathError::MissingMoveTo);
            }   _ => {}
        }
    }   Ok(())
}
