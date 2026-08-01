//! Floating-point vector rendering backend.

use crate::common::geometry::Edge;

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

#[cfg(test)] mod tests {
    use crate::common::geometry::{PathBuilder, PathError};
    #[test] fn checked_path_rejects_non_finite_coordinates() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((f32::INFINITY, 1.0));
        assert_eq!(builder.build_checked().unwrap_err(), PathError::NonFiniteCoordinate);
    }
}
