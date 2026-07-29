//! Directed fill edges produced from flattened paths.

use crate::flatten::{flatten_path, FlattenError, FlattenOptions, LineSink};
use crate::geometry::{Affine, Path, Point, Scalar};

/// A non-horizontal edge normalized to increasing device-space `y`.
///
/// `winding` preserves the source direction: `1` for downward and `-1` for
/// upward in the device coordinate system. Horizontal lines do not produce an
/// edge because they contribute no winding crossing.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, PartialEq)] pub struct Edge<T = Scalar> {
    pub upper: Point<T>, pub lower: Point<T>, pub winding: i8,
}

impl Edge {
    pub(crate) fn slope(&self) -> f32 {
        (self.lower.x - self.upper.x) / (self.lower.y - self.upper.y)
    }

    pub(crate) fn x_at(&self, y: f32) -> f32 {
        self.upper.x + self.slope() * (y - self.upper.y)
    }
}

pub trait EdgeSink {    type Error;
    fn edge(&mut self, edge: Edge) -> Result<(), Self::Error>;
}

impl<E, F> EdgeSink for F where F: FnMut(Edge) -> Result<(), E> {
    fn edge(&mut self, edge: Edge) -> Result<(), Self::Error> { self(edge) }
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
        if from.y == to.y { return Ok(()); }
        let edge = if from.y < to.y {
            Edge { upper: from, lower: to, winding: 1 }
        } else {
            Edge { upper: to, lower: from, winding: -1 }
        };  self.sink.edge(edge)
    }
}

#[cfg(test)] mod tests {
    use alloc::vec::Vec;
    use core::convert::Infallible;
    use super::{build_fill_edges, Edge};
    use crate::flatten::{FlattenError, FlattenOptions};
    use crate::geometry::{Affine, PathBuilder, Point};

    fn collect(path: &crate::geometry::Path) -> Result<Vec<Edge>, FlattenError<Infallible>> {
        let mut edges = Vec::new();
        build_fill_edges(path, Affine::identity(), FlattenOptions::default(),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) })?;
        Ok(edges)
    }

    #[test] fn open_rectangle_is_implicitly_closed_and_horizontal_edges_are_omitted() {
        let mut builder = PathBuilder::new();
        builder.move_to((1.0, 2.0))      .line_to((5.0, 2.0)).unwrap()
            .line_to((5.0, 7.0)).unwrap().line_to((1.0, 7.0)).unwrap();
        assert_eq!(collect(&builder.build()).unwrap(), [
            Edge {
                upper: Point::new(5.0, 2.0), lower: Point::new(5.0, 7.0), winding: 1,
            },
            Edge {
                upper: Point::new(1.0, 2.0), lower: Point::new(1.0, 7.0), winding: -1,
            },
        ]);
    }

    #[test] fn explicit_close_does_not_duplicate_the_closing_edge() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((1.0, 1.0)).unwrap()
            .line_to((2.0, 0.0)).unwrap().close().unwrap();
        assert_eq!(collect(&builder.build()).unwrap().len(), 2);
    }

    #[test] fn move_to_closes_each_previous_subpath() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((1.0, 1.0)).unwrap();
        builder.move_to((2.0, 0.0)).line_to((3.0, 1.0)).unwrap();
        let edges = collect(&builder.build()).unwrap();
        assert_eq!(edges.len(), 4);
        assert_eq!(edges.iter().map(|edge| edge.winding as i32).sum::<i32>(), 0);
    }

    #[test] fn edge_sink_capacity_error_propagates() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((1.0, 1.0)).unwrap();
        let result = build_fill_edges(&builder.build(), Affine::identity(),
            FlattenOptions::default(), &mut |_| Err("full"));
        assert_eq!(result, Err(FlattenError::Sink("full")));
    }
}
