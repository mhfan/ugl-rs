//! Floating-point edge calculations.

use crate::edge::Edge;

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
