//! Floating-point vector rendering backend.

use crate::common::geometry::{Edge, Path, PathBuilder, PathError, PathSegment, Point};

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
