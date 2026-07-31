//! Directed fill edges produced from flattened paths.

use core::cmp::Ordering;
use crate::{geometry::{Affine, Path, Point, Scalar},
    flatten::{flatten_path, FlattenError, FlattenOptions, LineSink}};

/// A non-horizontal edge normalized to increasing device-space `y`.
///
/// `winding` preserves the source direction: `1` for downward and `-1` for
/// upward in the device coordinate system. Horizontal lines do not produce an
/// edge because they contribute no winding crossing.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, PartialEq)] pub struct Edge<T = Scalar> {
    pub upper: Point<T>, pub lower: Point<T>, pub winding: i8,
}

impl<T> Edge<T> where T: Copy + PartialOrd {
    pub(crate) fn from_line(from: Point<T>, to: Point<T>) -> Option<Self> {
        match from.y.partial_cmp(&to.y)? {
            Ordering::Less => Some(Self { upper: from, lower: to, winding: 1 }),
            Ordering::Greater => Some(Self { upper: to, lower: from, winding: -1 }),
            Ordering::Equal => None,
        }
    }
}

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

pub trait EdgeSink<T = Scalar> {    type Error;
    fn edge(&mut self, edge: Edge<T>) -> Result<(), Self::Error>;
}

impl<T, E, F> EdgeSink<T> for F where F: FnMut(Edge<T>) -> Result<(), E> {
    fn edge(&mut self, edge: Edge<T>) -> Result<(), Self::Error> { self(edge) }
    type Error = E;
}

/// Flattens a path and emits edges suitable for filling.
///
/// Every subpath is implicitly closed. Explicitly closed subpaths produce the
/// same edges because zero-length closing lines are ignored.
pub fn build_fill_edges<S>(path: &Path, transform: Affine, options: FlattenOptions,
    sink: &mut S) -> Result<(), FlattenError<S::Error>> where S: EdgeSink {
    flatten_path(path, transform, options, &mut FillEdgeBuilder::new(sink))
}

struct FillEdgeBuilder<'a, S> {
    sink: &'a mut S, start: Option<Point>, current: Option<Point>,
}

impl<'a, S> FillEdgeBuilder<'a, S> {
    fn new(sink: &'a mut S) -> Self { Self { sink, start: None, current: None } }
}

impl<S> LineSink for FillEdgeBuilder<'_, S> where S: EdgeSink {
    type Error = S::Error;

    fn begin_subpath(&mut self, at: Point) -> Result<(), Self::Error> {
        self.start   = Some(at);
        self.current = Some(at);    Ok(())
    }

    fn line(&mut self, from: Point, to: Point) -> Result<(), Self::Error> {
        self.emit(from, to)?;
        self.current = Some(to);    Ok(())
    }

    fn end_subpath(&mut self) -> Result<(), Self::Error> {
        if let (Some(from), Some(to)) = (self.current, self.start) {
            self.emit(from, to)?;
        }
        self.start   = None;
        self.current = None;        Ok(())
    }
}

impl<S> FillEdgeBuilder<'_, S> where S: EdgeSink {
    fn emit(&mut self, from: Point, to: Point) -> Result<(), S::Error> {
        if let Some(edge) = Edge::from_line(from, to) { self.sink.edge(edge) } else { Ok(()) }
    }
}

#[cfg(test)] mod tests { use super::*;
    use crate::{flatten::{FlattenError, FlattenOptions},
        geometry::{Affine, PathBuilder, Path}};
    use core::convert::Infallible;
    use alloc::vec::Vec;

    fn collect(path: &Path) -> Result<Vec<Edge>, FlattenError<Infallible>> {
        let mut edges = Vec::new();
        build_fill_edges(path, Affine::identity(), FlattenOptions::default(),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) })?;
        Ok(edges)
    }

    #[test] fn open_rectangle_is_implicitly_closed_and_horizontal_edges_are_omitted() {
        let mut builder = PathBuilder::new();
        builder.move_to((1.0, 2.0)).line_to((5.0, 2.0))
               .line_to((5.0, 7.0)).line_to((1.0, 7.0));
        assert_eq!(collect(&builder.build()).unwrap(), [
            Edge {
                upper: (5.0, 2.0).into(), lower: (5.0, 7.0).into(), winding: 1,
            },
            Edge {
                upper: (1.0, 2.0).into(), lower: (1.0, 7.0).into(), winding: -1,
            },
        ]);
    }

    #[test] fn explicit_close_does_not_duplicate_the_closing_edge() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((1.0, 1.0)).line_to((2.0, 0.0)).close();
        assert_eq!(collect(&builder.build()).unwrap().len(), 2);
    }

    #[test] fn move_to_closes_each_previous_subpath() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((1.0, 1.0));
        builder.move_to((2.0, 0.0)).line_to((3.0, 1.0));
        let edges = collect(&builder.build()).unwrap();
        assert_eq!(edges.len(), 4);
        assert_eq!(edges.iter().map(|edge| edge.winding as i32).sum::<i32>(), 0);
    }

    #[test] fn edge_sink_capacity_error_propagates() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((1.0, 1.0));
        let result = build_fill_edges(&builder.build(), Affine::identity(),
            FlattenOptions::default(), &mut |_| Err("full"));
        assert_eq!(result, Err(FlattenError::Sink("full")));
    }

    #[cfg(feature = "fixed")]
    #[test] fn fixed_lines_share_edge_normalization_and_winding() {
        use crate::geometry::FixedScalar;

        let (zero, one) = (FixedScalar::ZERO, FixedScalar::ONE);
        assert_eq!(Edge::from_line((zero, one).into(), (one, zero).into()),
            Some(Edge { upper: (one, zero).into(),
                        lower: (zero, one).into(), winding: -1,
            }));
        assert_eq!(Edge::from_line((zero, one).into(), (one, one).into()), None);
    }
}
