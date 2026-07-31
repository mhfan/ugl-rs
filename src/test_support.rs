use crate::{edge::Edge, geometry::Point};

#[cfg(feature = "fixed")]
pub(crate) struct Random(u64);

pub(crate) struct RectangleCoverageCase {
    pub(crate) left_raw: i32,
    pub(crate) right_raw: i32,
    pub(crate) width: usize,
    pub(crate) expected: &'static [u8],
}

pub(crate) const RECTANGLE_COVERAGE_CASES: &[RectangleCoverageCase] = &[
    RectangleCoverageCase {
        left_raw: 256, right_raw: 768, width: 4, expected: &[0, 255, 255, 0],
    },
    RectangleCoverageCase {
        left_raw: 128, right_raw: 384, width: 2, expected: &[128, 128],
    },
];

#[cfg(feature = "fixed")]
impl Random {
    pub(crate) const fn new(seed: u64) -> Self { Self(seed) }

    pub(crate) fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 as _
    }
}

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

pub(crate) fn assert_line_chain<T: Copy + core::fmt::Debug + PartialEq>(
    lines: &[(Point<T>, Point<T>)], start: Point<T>, end: Point<T>) {
    assert!(lines.len() > 1);
    assert_eq!(lines.first().unwrap().0, start);
    assert_eq!(lines.last().unwrap().1, end);
    assert!(lines.windows(2).all(|pair| pair[0].1 == pair[1].0));
}

pub(crate) fn x_bounds<T: Copy + PartialOrd>(edges: &[Edge<T>]) -> Option<(T, T)> {
    let mut coordinates = edges.iter()
        .flat_map(|edge| [edge.upper.x, edge.lower.x]);
    let first = coordinates.next()?;
    Some(coordinates.fold((first, first), |(minimum, maximum), x| (
        if x < minimum { x } else { minimum },
        if x > maximum { x } else { maximum },
    )))
}
