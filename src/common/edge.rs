//! Directed fill edges produced from flattened paths.

use core::cmp::Ordering;
use crate::geometry::{Point, Scalar};

pub trait LineSink<T = Scalar> { type Error;
    fn begin_subpath(&mut self, _: Point<T>) -> Result<(), Self::Error> { Ok(()) }

    fn line(&mut self, from: Point<T>, to: Point<T>) -> Result<(), Self::Error>;

    /// Reports an explicit path close after its closing line has been emitted.
    fn close_subpath(&mut self) -> Result<(), Self::Error> { Ok(()) }

    fn end_subpath(&mut self) -> Result<(), Self::Error> { Ok(()) }
}

impl<T, E, F> LineSink<T> for F where F: FnMut(Point<T>, Point<T>) -> Result<(), E> {
    type Error = E;

    fn line(&mut self, from: Point<T>, to: Point<T>) -> Result<(), Self::Error> {
        self(from, to)
    }
}

/// A non-horizontal edge normalized to increasing device-space `y`.
///
/// `winding` preserves the source direction: `1` for downward and `-1` for
/// upward in the device coordinate system. Horizontal lines do not produce an
/// edge because they contribute no winding crossing.
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

pub trait EdgeSink<T = Scalar> {    type Error;
    fn edge(&mut self, edge: Edge<T>) -> Result<(), Self::Error>;
}

impl<T, E, F> EdgeSink<T> for F where F: FnMut(Edge<T>) -> Result<(), E> {
    fn edge(&mut self, edge: Edge<T>) -> Result<(), Self::Error> { self(edge) }
    type Error = E;
}

pub(crate) struct FillEdgeBuilder<'a, S, T = Scalar> {
    sink: &'a mut S, start: Option<Point<T>>, current: Option<Point<T>>,
}

impl<'a, S, T> FillEdgeBuilder<'a, S, T> {
    pub(crate) fn new(sink: &'a mut S) -> Self {
        Self { sink, start: None, current: None }
    }
}

impl<S, T> LineSink<T> for FillEdgeBuilder<'_, S, T>
    where S: EdgeSink<T>, T: Copy + PartialOrd {
    type Error = S::Error;

    fn begin_subpath(&mut self, at: Point<T>) -> Result<(), Self::Error> {
        self.start   = Some(at);
        self.current = Some(at);    Ok(())
    }

    fn line(&mut self, from: Point<T>, to: Point<T>) -> Result<(), Self::Error> {
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

impl<S, T> FillEdgeBuilder<'_, S, T> where S: EdgeSink<T>, T: Copy + PartialOrd {
    fn emit(&mut self, from: Point<T>, to: Point<T>) -> Result<(), S::Error> {
        if let Some(edge) = Edge::from_line(from, to) { self.sink.edge(edge) } else { Ok(()) }
    }
}

#[cfg(all(test, feature = "f32"))] mod tests { use super::*;
    use crate::{float::flatten::{build_fill_edges, FlattenError, FlattenOptions},
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

}
