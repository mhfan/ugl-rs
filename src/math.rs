//! Numeric building blocks shared by rendering backends.
//!
//! Geometry no longer exposes a third-party matrix type. The optional re-export
//! remains for clients experimenting with fixed-point coordinates.

//  number: Q-number/Fixed/float/double, performance/fast vs quality/accurate
//  linear algebra, affine transformation, trigonometry

//  https://github.com/dimforge/nalgebra/blob/main/nalgebra-glm/src/aliases.rs
//pub use nalgebra_glm::TMat;

//  https://en.wikipedia.org/wiki/Q_(number_format)
//  https://johnmcfarlane.github.io/cnl/
//  https://gitlab.com/tspiteri/fixed
#[cfg(feature = "fixed")] pub use fixed::traits::Fixed;

