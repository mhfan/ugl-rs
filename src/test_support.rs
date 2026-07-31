#[cfg(feature = "fixed")]
use crate::{edge::Edge, geometry::Point};

#[cfg(feature = "fixed")]
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

#[cfg(feature = "fixed")]
pub(crate) fn assert_coverage_near(
    actual: &[u8], expected: &[u8], tolerance: u8, context: impl core::fmt::Display) {
    assert_eq!(actual.len(), expected.len(), "{context}: coverage dimensions differ");
    for (pixel, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
        assert!(actual.abs_diff(expected) <= tolerance,
            "{context}, pixel {pixel}: actual={actual}, expected={expected}");
    }
}
