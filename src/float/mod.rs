//! Floating-point vector rendering backend.

use crate::common::geometry::{Affine, Edge, Path, PathBuilder, PathError, PathSegment, Point};

impl Affine<f32> {
    /// Transforms points and vectors using the affine matrix's column-vector convention.
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

mod math;
pub mod blend;
pub mod dash;
pub mod flatten;
pub mod raster;
pub mod stroke;
pub(crate) use math::*;

impl Edge {
    pub(crate) fn is_valid(&self) -> bool {
        [self.upper.x, self.upper.y, self.lower.x, self.lower.y]
            .iter().all(|value| value.is_finite()) &&
            self.upper.y < self.lower.y && matches!(self.winding, -1 | 1)
    }

    pub(crate) fn slope(&self) -> f32 {
        (self.lower.x - self.upper.x) / (self.lower.y - self.upper.y)
    }

    pub(crate) fn x_at(&self, y: f32) -> f32 {
        self.upper.x + self.slope() * (y - self.upper.y)
    }
}

pub mod analytic;
pub mod canvas;
pub mod linear;
pub mod context;
pub mod sampler;

pub use context::Canvas;

#[cfg(test)] mod tests { use super::*;
    #[test] fn checked_path_rejects_non_finite_coordinates() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((f32::INFINITY, 1.0));
        assert_eq!(builder.build_checked().unwrap_err(), PathError::NonFiniteCoordinate);
    }
}
