//! Shared stroke storage and floating-point reference expansion.

use super::geometry::{LineSink, Point, Scalar};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LineCap { #[default] Butt, Round, Square, }

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LineJoin { #[default] Miter, Round, Bevel, }

/// Compact descriptor for one flattened stroke subpath.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StrokeContour { start: u32, len: u32, closed: bool }

impl StrokeContour {
    pub fn is_closed(&self) -> bool { self.closed }
    pub fn len(&self) -> usize { self.len as _ }
    pub fn is_empty(&self) -> bool { self.len == 0 }
}

/// Caller-owned storage used while flattening a path for stroke expansion.
pub struct StrokePathWorkspace<'a, T = Scalar> {
    pub contours: &'a mut [StrokeContour],
    pub   points: &'a mut [Point<T>],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum StrokeWorkspaceError {
      PointCapacity { needed_at_least: usize },
    ContourCapacity { needed_at_least: usize },
    IndexOverflow,
}

/// Borrowed flattened path backed by a [`StrokePathWorkspace`].
pub struct FlattenedStrokePath<'a, T = Scalar> {
    contours: &'a [StrokeContour],
      points: &'a [Point<T>],
}

impl<'a, T> FlattenedStrokePath<'a, T> {
    pub fn point_count(&self) -> usize { self.points.len() }
    pub fn contour_count(&self) -> usize { self.contours.len() }

    pub fn contours(&self) -> impl ExactSizeIterator<Item = (&'a [Point<T>], bool)> + 'a {
        self.contours.iter().map(|contour| {
            let start: usize = contour.start as _;
            (&self.points[start..start + contour.len()], contour.is_closed())
        })
    }
}

pub(crate) struct StrokePathSink<'a, T = Scalar> {
    points: &'a mut [Point<T>],
    contours: &'a mut [StrokeContour],
    point_len: usize,
    contour_len: usize,
    current_start: Option<usize>,
    current_closed: bool,
}

pub(crate) fn flatten_stroke_path_with<'a, T: Copy, E>(
    workspace: &'a mut StrokePathWorkspace<'_, T>,
    flatten: impl FnOnce(&mut StrokePathSink<'_, T>) -> Result<(), E>) ->
    Result<FlattenedStrokePath<'a, T>, E> {
    let (point_len, contour_len) = {
        let mut sink = StrokePathSink {
            points: workspace.points, contours: workspace.contours,
            point_len: 0, contour_len: 0, current_start: None, current_closed: false,
        };
        flatten(&mut sink)?;
        (sink.point_len, sink.contour_len)
    };
    Ok(FlattenedStrokePath {
          points: &workspace.points[..point_len],
        contours: &workspace.contours[..contour_len],
    })
}

impl<T: Copy> StrokePathSink<'_, T> {
    fn push_point(&mut self, point: Point<T>) -> Result<(), StrokeWorkspaceError> {
        let needed = self.point_len.checked_add(1).ok_or(StrokeWorkspaceError::IndexOverflow)?;
        let slot = self.points.get_mut(self.point_len)
            .ok_or(StrokeWorkspaceError::PointCapacity { needed_at_least: needed })?;
           *slot = point;   self.point_len = needed;    Ok(())
    }
}

impl<T: Copy> LineSink<T> for StrokePathSink<'_, T> {
    type Error = StrokeWorkspaceError;

    fn begin_subpath(&mut self, at: Point<T>) -> Result<(), Self::Error> {
        self.current_start = Some(self.point_len);
        self.current_closed = false;
        self.push_point(at)
    }

    fn line(&mut self, _: Point<T>, to: Point<T>) -> Result<(), Self::Error> {
        self.push_point(to)
    }

    fn close_subpath(&mut self) -> Result<(), Self::Error> {
        self.current_closed = true;     Ok(())
    }

    fn end_subpath(&mut self) -> Result<(), Self::Error> {
        let Some(start) = self.current_start.take() else { return Ok(()) };
        let len = self.point_len - start;
        let needed = self.contour_len.checked_add(1)
            .ok_or(StrokeWorkspaceError::IndexOverflow)?;
        let descriptor = StrokeContour {
            start: u32::try_from(start).map_err(|_| StrokeWorkspaceError::IndexOverflow)?,
              len: u32::try_from(len)  .map_err(|_| StrokeWorkspaceError::IndexOverflow)?,
            closed: self.current_closed,
        };
        let slot = self.contours.get_mut(self.contour_len)
            .ok_or(StrokeWorkspaceError::ContourCapacity { needed_at_least: needed })?;
        *slot = descriptor;     self.contour_len = needed;  Ok(())
    }
}

#[cfg(test)] mod tests { use super::*;
    fn flatten<'a>(workspace: &'a mut StrokePathWorkspace<'_, f32>) ->
        Result<FlattenedStrokePath<'a>, StrokeWorkspaceError> {
        flatten_stroke_path_with(workspace, |sink| {
            sink.begin_subpath((1.0, 2.0).into())?;
            sink.line((1.0, 2.0).into(), (3.0, 4.0).into())?;
            sink.close_subpath()?; sink.end_subpath()?;
            sink.begin_subpath((5.0, 6.0).into())?;
            sink.line((5.0, 6.0).into(), (7.0, 8.0).into())?;
            sink.end_subpath()
        })
    }

    #[test] fn stroke_path_workspace_preserves_subpaths_and_explicit_close() {
        let (mut points, mut contours) =
            ([Point::default(); 4], [StrokeContour::default(); 2]);
        let mut workspace = StrokePathWorkspace { points: &mut points, contours: &mut contours };
        let flattened = flatten(&mut workspace).unwrap();
        let contours: alloc::vec::Vec<_> = flattened.contours().collect();
        assert_eq!(contours, [
            (&[(1.0, 2.0).into(), (3.0, 4.0).into()][..], true),
            (&[(5.0, 6.0).into(), (7.0, 8.0).into()][..], false),
        ]);
    }

    #[test] fn stroke_path_workspace_reports_exact_capacity_class() {
        let (mut points, mut contours) =
            ([Point::default(); 1], [StrokeContour::default(); 2]);
        let mut workspace = StrokePathWorkspace { points: &mut points, contours: &mut contours };
        assert!(matches!(flatten(&mut workspace),
            Err(StrokeWorkspaceError::PointCapacity { needed_at_least: 2 })));
        let (mut points, mut contours) = ([Point::default(); 4], []);
        let mut workspace = StrokePathWorkspace { points: &mut points, contours: &mut contours };
        assert!(matches!(flatten(&mut workspace),
            Err(StrokeWorkspaceError::ContourCapacity { needed_at_least: 1 })));
    }
}
