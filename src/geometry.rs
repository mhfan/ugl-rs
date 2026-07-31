//! Geometry shared by floating-point and future fixed-point render backends.
//!
//! Containers are generic, while rasterization algorithms initially operate on
//! [`f32`]. A renderer can consume [`Path::segments`] or any independently
//! stored slice of [`PathSegment`], which leaves room for static/fixed-capacity
//! path storage on systems without a heap.

//  Point/Line/(Bezier)Curve, Shapes (Triangle/Rectangle/Polygon/Ellipse/Circle/Arc)
//  Fill(solid/linear/radial/conic/texture)/Stroke(width/cap/join/dash)

use alloc::vec::Vec;

/// Coordinate type used by the reference renderer.
pub type Scalar = f32;

/// Q24.8 device coordinate used by the fixed-point reference backend.
///
/// Raster products and areas must use widened intermediates rather than this
/// 32-bit storage type.
#[cfg(feature = "fixed")] pub type FixedScalar = fixed::types::I24F8;
/// Raw Q24.8 coordinate magnitude supported by the bounded fixed render path.
#[cfg(feature = "fixed")] pub const FIXED_DEVICE_RAW_LIMIT: i32 = 1 << 29;
#[cfg(feature = "fixed")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum FixedTransformError { Overflow }

pub trait ScalarConstants { const ZERO: Self; const ONE: Self; }
impl ScalarConstants for f32 { const ZERO: Self = 0.0; const ONE: Self = 1.0; }

#[cfg(feature = "fixed")] impl ScalarConstants for FixedScalar {
    const ZERO: Self = Self::ZERO;
    const  ONE: Self = Self::ONE;
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Point<T = Scalar> { pub x: T, pub y: T, }

impl<T> Point<T> { pub const fn new(x: T, y: T) -> Self { Self { x, y } } }
impl<T> From<(T, T)> for Point<T> { fn from((x, y): (T, T)) -> Self { Self::new(x, y) } }

/// Axis-aligned rectangle with ordered, finite-or-orderable boundaries.
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
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

#[cfg(feature = "serde")] impl<'de, T> serde::Deserialize<'de> for Rect<T>
    where T: Copy + PartialOrd + serde::Deserialize<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where D: serde::Deserializer<'de> {
        #[derive(serde::Deserialize)] struct Fields<T> { min: Point<T>, max: Point<T> }
        let Fields { min, max } = Fields::deserialize(deserializer)?;
        Self::from_ltrb(min.x, min.y, max.x, max.y).ok_or_else(||
            serde::de::Error::custom("rectangle boundaries must be ordered"))
    }
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
        if  determinant == 0.0 || !determinant.is_finite() { return None; }
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

#[cfg(feature = "fixed")] impl Affine<FixedScalar> {
    /// Transforms a Q24.8 point with widened multiply-add and checked conversion.
    ///
    /// The result is rounded to the nearest Q24.8 value, with exact half units
    /// rounded away from zero. Renderer-specific device limits are checked by
    /// the consuming backend.
    pub fn try_transform_point(&self, point: Point<FixedScalar>) ->
        Result<Point<FixedScalar>, FixedTransformError> {
        let transform = |first: FixedScalar, x: FixedScalar,
            second: FixedScalar, y: FixedScalar, translation: FixedScalar| {
            const FRACTION_BITS: u32 = 8;
            const SCALE: i128 = 1 << FRACTION_BITS;
            let value = first.to_bits() as i128 * x.to_bits() as i128
                + second.to_bits() as i128 * y.to_bits() as i128
                + ((translation.to_bits() as i128) << FRACTION_BITS);
            let rounded = if value < 0 {
                (value - SCALE / 2) / SCALE
            } else { (value + SCALE / 2) / SCALE };
            i32::try_from(rounded).map(FixedScalar::from_bits)
                .map_err(|_| FixedTransformError::Overflow)
        };
        Ok((transform(self.a, point.x, self.c, point.y, self.e)?,
            transform(self.b, point.x, self.d, point.y, self.f)?).into())
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
        let path = self.build(); path.validate_finite()?; Ok(path)
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
        assert_eq!(transform.transform_point((3.0, 2.0).into()), (8.0, 5.5).into());
        assert_eq!(transform.transform_vector((3.0, 2.0).into()), (4.0, 7.5).into());
        let restored = transform.inverse().unwrap()
            .transform_point(transform.transform_point((3.0, 2.0).into()));
        assert!((restored.x - 3.0).abs() < 1e-6 && (restored.y - 2.0).abs() < 1e-6);
        assert!(Affine::new(1.0, 2.0, 2.0, 4.0, 0.0, 0.0).inverse().is_none());
    }

    #[test] fn rectangles_reject_unordered_and_non_finite_boundaries() {
        assert_eq!(Rect::from_ltrb(1.0, 2.0, 3.0, 4.0).map(|rect|
            (rect.min(), rect.max())), Some(((1.0, 2.0).into(), (3.0, 4.0).into())));
        assert_eq!(Rect::from_ltrb(3.0, 2.0, 1.0, 4.0), None);
        assert_eq!(Rect::from_ltrb(1.0, f32::NAN, 3.0, 4.0), None);
    }

    #[test] fn path_builder_starts_missing_subpaths_and_closes_idempotently() {
        let mut builder = PathBuilder::<f32>::new();
        builder.close().line_to((1.0, 2.0));
        assert_eq!(builder.build().segments(),
            &[PathSegment::MoveTo((1.0, 2.0).into())]);

        let mut builder = PathBuilder::<f32>::new();
        builder.move_to((0.0, 0.0)).line_to((1.0, 0.0));
        builder.close().close();
        assert_eq!(builder.build().segments(), &[
            PathSegment::MoveTo((0.0, 0.0).into()),
            PathSegment::LineTo((1.0, 0.0).into()),
            PathSegment::Close,
        ]);

        let mut quad = PathBuilder::<f32>::new();
        quad.quad_to((1.0, 1.0), (2.0, 2.0));
        assert_eq!(quad.build().segments(), &[PathSegment::MoveTo((2.0, 2.0).into())]);

        let mut cubic = PathBuilder::<f32>::new();
        cubic.cubic_to((1.0, 1.0), (2.0, 2.0), (3.0, 3.0));
        assert_eq!(cubic.build().segments(), &[PathSegment::MoveTo((3.0, 3.0).into())]);
    }

    #[test] fn path_can_borrow_static_or_fixed_capacity_segments() {
        let segments = [
            PathSegment::MoveTo((0_i32, 0_i32).into()),
            PathSegment::LineTo((256, 0).into()),
            PathSegment::Close,
        ];
        assert_eq!(segments.len(), 3);
        assert_eq!(Path::from_segments(segments.to_vec()).unwrap().len(), 3);
    }

    #[test] fn checked_reference_path_rejects_non_finite_coordinates() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((f32::INFINITY, 1.0));
        assert_eq!(builder.build_checked().unwrap_err(), PathError::NonFiniteCoordinate);
    }

    #[cfg(feature = "fixed")]
    #[test] fn fixed_geometry_reuses_generic_point_path_and_affine_types() {
        let (one, half) = (FixedScalar::from_num(1), FixedScalar::from_num(0.5));
        let transform = Affine::<FixedScalar>::translate(half, one);
        assert_eq!(transform.try_transform_point((one, half).into()).unwrap(),
            (FixedScalar::from_num(1.5), FixedScalar::from_num(1.5)).into());

        let mut builder = PathBuilder::<FixedScalar>::new();
        builder.move_to((FixedScalar::ZERO, FixedScalar::ZERO))
            .line_to((one, half));
        assert_eq!(builder.build().len(), 2);
    }

    #[cfg(feature = "fixed")]
    #[test] fn fixed_affine_widens_rounds_symmetrically_and_checks_output() {
        let raw = FixedScalar::from_bits;
        let half_scale = Affine::new(raw(128), raw(0), raw(0), raw(128), raw(0), raw(0));
        assert_eq!(half_scale.try_transform_point((raw(1), raw(-1)).into()).unwrap(),
            (raw(1), raw(-1)).into());

        let maximum = FixedScalar::MAX;
        let overflow = Affine::new(maximum, FixedScalar::ZERO, FixedScalar::ZERO,
            maximum, maximum, maximum);
        assert_eq!(overflow.try_transform_point((maximum, maximum).into()),
            Err(FixedTransformError::Overflow));
    }
}
