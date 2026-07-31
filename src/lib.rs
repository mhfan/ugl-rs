
#![no_std]

extern crate alloc;

pub mod color;      // rgba/rgb, intensity & quantization
pub mod blend;      // color blending & alpha compositing, gamma correction

pub mod sampler;    // can be thought of 2D shaders
pub mod shader;     // reserved for a future optional 3D layer

pub mod geometry;   // shape, curve, free path

pub mod raster;
pub mod stroke;
pub mod dash;
pub mod flatten;
pub mod analytic;
pub mod canvas;
pub mod canvas_linear;
pub mod context;
pub mod edge;

#[cfg(feature = "fixed")] pub mod fixed;

#[cfg(all(test, feature = "fixed"))] mod test_support {
    use crate::{edge::Edge, geometry::Point};

    pub(crate) fn polygon_edges<T: Copy + PartialOrd>(
        points: &[Point<T>]) -> alloc::vec::Vec<Edge<T>> {
        let mut edges = alloc::vec::Vec::new();
        for index in 0..points.len() {
            if let Some(edge) = Edge::from_line(
                points[index], points[(index + 1) % points.len()]) {
                edges.push(edge);
            }
        }
        edges
    }

    pub(crate) fn assert_coverage_near(
        actual: &[u8], expected: &[u8], tolerance: u8,
        context: impl core::fmt::Display) {
        assert_eq!(actual.len(), expected.len(),
            "{context}: coverage dimensions differ");
        for (pixel, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
            assert!(actual.abs_diff(expected) <= tolerance,
                "{context}, pixel {pixel}: actual={actual}, expected={expected}");
        }
    }
}
