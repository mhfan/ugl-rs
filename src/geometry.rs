//! Geometry shared by floating-point and future fixed-point render backends.
//!
//! Containers are generic, while rasterization algorithms initially operate on
//! [`f32`]. A renderer can consume [`Path::segments`] or any independently
//! stored slice of [`PathSegment`], which leaves room for static/fixed-capacity
//! path storage on systems without a heap.

//  Point/Line/(Bezier)Curve, Shapes (Triangle/Rectangle/Polygon/Ellipse/Circle/Arc)
//  Fill(solid/linear/radial/conic/texture)/Stroke(width/cap/join/dash)

use alloc::vec::Vec;
use core::ops::{Add, Mul};

/// Coordinate type used by the reference renderer.
pub type Scalar = f32;

/// Q24.8 device coordinate used by the fixed-point reference backend.
///
/// Raster products and areas must use widened intermediates rather than this
/// 32-bit storage type.
#[cfg(feature = "fixed")] pub type FixedScalar = fixed::types::I24F8;

pub trait ScalarConstants { const ZERO: Self; const ONE: Self; }
impl ScalarConstants for f32 { const ZERO: Self = 0.0; const ONE: Self = 1.0; }

#[cfg(feature = "fixed")] impl ScalarConstants for FixedScalar {
    const ZERO: Self = Self::ZERO;
    const  ONE: Self = Self::ONE;
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point<T = Scalar> { pub x: T, pub y: T, }

impl<T> Point<T> { pub const fn new(x: T, y: T) -> Self { Self { x, y } } }

impl<T> From<(T, T)> for Point<T> {
    fn from((x, y): (T, T)) -> Self { Self::new(x, y) }
}

/// A 2D affine transform using column-vector convention.
///
/// `x' = a*x + c*y + e`, `y' = b*x + d*y + f`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)] pub struct Affine<T = Scalar> {
    pub a: T, pub b: T, pub c: T, pub d: T, pub e: T, pub f: T,
}

impl<T> Affine<T> {
    pub const fn new(a: T, b: T, c: T, d: T, e: T, f: T) -> Self {
        Self { a, b, c, d, e, f }
    }

    pub fn transform_point(&self, point: Point<T>) -> Point<T>
        where T: Copy + Add<Output = T> + Mul<Output = T> {
        Point::new(
            self.a * point.x + self.c * point.y + self.e,
            self.b * point.x + self.d * point.y + self.f,
        )
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

impl<T> Default for Affine<T> where T: Copy + ScalarConstants {
    fn default() -> Self { Self::identity() }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)] pub enum PathSegment<T = Scalar> {
    MoveTo(Point<T>), LineTo(Point<T>), Close,
    QuadTo  { ctrl:  Point<T>, to: Point<T> },
    CubicTo { ctrl1: Point<T>, ctrl2: Point<T>, to: Point<T> },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path<T = Scalar> { segments: Vec<PathSegment<T>>, }

impl<T> Path<T> {
    pub fn  from_segments( segments: Vec<PathSegment<T>>) -> Result<Self, PathError> {
        validate_segments(&segments)?;
        Ok(Self { segments })
    }

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

#[derive(Clone, Debug, Default)] pub struct PathBuilder<T = Scalar> {
    segments: Vec<PathSegment<T>>,
    has_current_subpath: bool,
}

impl<T> PathBuilder<T> {
    pub fn new() -> Self { Self { segments: Vec::new(), has_current_subpath: false } }

    pub fn with_capacity(capacity: usize) -> Self {
        Self { segments: Vec::with_capacity(capacity), has_current_subpath: false }
    }

    pub fn move_to(&mut self, point: impl Into<Point<T>>) -> &mut Self {
        self.segments.push(PathSegment::MoveTo(point.into()));
        self.has_current_subpath = true;
        self
    }

    pub fn line_to(&mut self, point: impl Into<Point<T>>) -> Result<&mut Self, PathError> {
        self.require_subpath()?;
        self.segments.push(PathSegment::LineTo(point.into()));
        Ok(self)
    }

    pub fn quad_to(&mut self, ctrl: impl Into<Point<T>>, to: impl Into<Point<T>>) ->
        Result<&mut Self, PathError> {
        self.require_subpath()?;
        self.segments.push(PathSegment::QuadTo { ctrl: ctrl.into(), to: to.into() });
        Ok(self)
    }

    pub fn cubic_to(&mut self, ctrl1: impl Into<Point<T>>, ctrl2: impl Into<Point<T>>,
        to: impl Into<Point<T>>) -> Result<&mut Self, PathError> {
        self.require_subpath()?;
        self.segments.push(PathSegment::CubicTo {
            ctrl1: ctrl1.into(), ctrl2: ctrl2.into(), to: to.into(),
        }); Ok(self)
    }

    pub fn close(&mut self) -> Result<&mut Self, PathError> {
        self.require_subpath()?;
        if !matches!(self.segments.last(), Some(PathSegment::Close)) {
            self.segments.push(PathSegment::Close);
        }   Ok(self)
    }

    pub fn build(self) -> Path<T> { Path { segments: self.segments } }

    fn require_subpath(&self) -> Result<(), PathError> {
        self.has_current_subpath.then_some(()).ok_or(PathError::MissingMoveTo)
    }
}

impl Path<f32> {
    pub fn validate_finite(&self) -> Result<(), PathError> {
        let point_is_finite = |point: Point<f32>| point.x.is_finite() && point.y.is_finite();
        for segment in   &self.segments {
            let finite = match segment {
                PathSegment::MoveTo(p) | PathSegment::LineTo(p) => point_is_finite(*p),
                PathSegment::QuadTo { ctrl, to } =>
                    point_is_finite(*ctrl)  && point_is_finite(*to),
                PathSegment::CubicTo { ctrl1, ctrl2, to } =>
                    point_is_finite(*ctrl1) &&
                    point_is_finite(*ctrl2) && point_is_finite(*to),
                PathSegment::Close => true,
            };
            if !finite { return Err(PathError::NonFiniteCoordinate); }
        }   Ok(())
    }
}

impl PathBuilder<f32> {
    pub fn build_checked(self) -> Result<Path<f32>, PathError> {
        let path = self.build();
        path.validate_finite()?;
        Ok(path)
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

#[cfg(test)] mod tests { use super::*;
    #[test] fn affine_uses_documented_column_vector_convention() {
        let transform = Affine::new(2.0, 0.5, -1.0, 3.0, 4.0, -2.0);
        assert_eq!(transform.transform_point(Point::new(3.0, 2.0)), Point::new(8.0, 5.5));
    }

    #[test] fn path_builder_enforces_subpath_start_and_idempotent_close() {
        let mut builder = PathBuilder::<f32>::new();
        assert_eq!(builder.line_to((1.0, 2.0)).unwrap_err(), PathError::MissingMoveTo);
        builder.move_to((0.0, 0.0)).line_to((1.0, 0.0)).unwrap();
        builder.close().unwrap().close().unwrap();
        assert_eq!(builder.build().segments(), &[
            PathSegment::MoveTo(Point::new(0.0, 0.0)),
            PathSegment::LineTo(Point::new(1.0, 0.0)),
            PathSegment::Close,
        ]);
    }

    #[test] fn path_can_borrow_static_or_fixed_capacity_segments() {
        let segments = [
            PathSegment::MoveTo(Point::new(0_i32, 0_i32)),
            PathSegment::LineTo(Point::new(256, 0)),
            PathSegment::Close,
        ];
        assert_eq!(segments.len(), 3);
        assert_eq!(Path::from_segments(segments.to_vec()).unwrap().len(), 3);
    }

    #[test] fn checked_reference_path_rejects_non_finite_coordinates() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((f32::INFINITY, 1.0)).unwrap();
        assert_eq!(builder.build_checked().unwrap_err(), PathError::NonFiniteCoordinate);
    }

    #[cfg(feature = "fixed")]
    #[test] fn fixed_geometry_reuses_generic_point_path_and_affine_types() {
        let (one, half) = (FixedScalar::from_num(1), FixedScalar::from_num(0.5));
        let transform = Affine::<FixedScalar>::translate(half, one);
        assert_eq!(transform.transform_point(Point::new(one, half)),
            Point::new(FixedScalar::from_num(1.5), FixedScalar::from_num(1.5)));

        let mut builder = PathBuilder::<FixedScalar>::new();
        builder.move_to((FixedScalar::ZERO, FixedScalar::ZERO))
            .line_to((one, half)).unwrap();
        assert_eq!(builder.build().len(), 2);
    }
}
