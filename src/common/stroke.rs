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

#[cfg(all(test, feature = "f32"))]
use crate::float::stroke::{StrokeError, StrokeExpandError, StrokeOptions,
    flatten_stroke_path, stroke_line, stroke_point, stroke_polyline};
#[cfg(all(test, feature = "f32"))] mod tests { use super::*;
    use alloc::vec::Vec;
    use core::convert::Infallible;
    use crate::{common::geometry::{Affine, Edge, PathBuilder},
        float::flatten::{FlattenError, FlattenOptions}};

    fn collect_line(from: impl Into<Point>,
                      to: impl Into<Point>, cap: LineCap) -> Vec<Edge> {
        let mut edges = Vec::new();
        stroke_line(from.into(), to.into(), StrokeOptions::new(2.0).unwrap().with_cap(cap),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }).unwrap();
        edges
    }

    #[test] fn stroke_path_workspace_preserves_subpaths_and_explicit_close() {
        let mut builder = PathBuilder::new();
        builder.move_to((1.0, 2.0)).line_to((3.0, 4.0)).close()
               .move_to((5.0, 6.0)).line_to((7.0, 8.0));
        let mut points = [Point::default(); 5];
        let mut contours = [StrokeContour::default(); 2];
        let mut workspace = StrokePathWorkspace {
            points: &mut points, contours: &mut contours,
        };
        let flattened = flatten_stroke_path(&builder.build(), Affine::identity(),
            FlattenOptions::default(), &mut workspace).unwrap();
        let contours: Vec<_> = flattened.contours().collect();
        assert_eq!(contours, [
            (&[(1.0, 2.0).into(), (3.0, 4.0).into(), (1.0, 2.0).into()][..], true),
            (&[(5.0, 6.0).into(), (7.0, 8.0).into()][..], false),
        ]);
    }

    #[test] fn stroke_path_workspace_reports_exact_capacity_class() {
        let mut builder = PathBuilder::new();
        builder.move_to((1.0, 2.0)).line_to((3.0, 4.0));
        let path = builder.build();
        let mut points = [Point::default(); 1];
        let mut contours = [StrokeContour::default(); 1];
        let mut workspace = StrokePathWorkspace {
            points: &mut points, contours: &mut contours,
        };
        assert_eq!(flatten_stroke_path(&path, Affine::identity(),
            FlattenOptions::default(), &mut workspace).err(),
            Some(FlattenError::Sink(StrokeWorkspaceError::PointCapacity {
                needed_at_least: 2,
            })));

        let (mut points, mut contours) = ([Point::default(); 2], []);
        let mut workspace = StrokePathWorkspace {
            points: &mut points, contours: &mut contours,
        };
        assert_eq!(flatten_stroke_path(&path, Affine::identity(),
            FlattenOptions::default(), &mut workspace).err(),
            Some(FlattenError::Sink(StrokeWorkspaceError::ContourCapacity {
                needed_at_least: 1,
            })));
    }

    #[test] fn stroke_options_reject_invalid_geometric_states() {
        assert_eq!(StrokeOptions::new(0.0), Err(StrokeError::NonPositiveWidth));
        assert_eq!(StrokeOptions::new(f32::INFINITY), Err(StrokeError::NonFiniteWidth));
        assert_eq!(StrokeOptions::new(2.0).unwrap().with_miter_limit(0.5),
                   Err(StrokeError::MiterLimitTooSmall));
        assert_eq!(StrokeOptions::new(2.0).unwrap().with_miter_limit(f32::NAN),
                   Err(StrokeError::NonFiniteMiterLimit));
        assert_eq!(StrokeOptions::new(2.0).unwrap().with_tolerance(0.0),
                   Err(StrokeError::NonPositiveTolerance));
        assert_eq!(StrokeOptions::new(2.0).unwrap().with_max_arc_segments(0),
                   Err(StrokeError::ArcSegmentLimitZero));
    }

    #[test] fn line_caps_expand_to_expected_bounds_without_allocation() {
        let bounds = |edges: &[Edge]| edges.iter().flat_map(|edge|
            [edge.upper.x, edge.lower.x]).fold((f32::INFINITY, f32::NEG_INFINITY),
                |(minimum, maximum), x| (minimum.min(x), maximum.max(x)));
        assert_eq!(bounds(&collect_line((2.0, 3.0),
            (6.0, 3.0), LineCap::Butt)),   (2.0, 6.0));
        assert_eq!(bounds(&collect_line((2.0, 3.0),
            (6.0, 3.0), LineCap::Square)), (1.0, 7.0));
        let (minimum, maximum) =
            bounds(&collect_line((2.0, 3.0), (6.0, 3.0), LineCap::Round));
        assert!(minimum > 1.0 && minimum - 1.0 <= StrokeOptions::default().tolerance());
        assert!(maximum < 7.0 && 7.0 - maximum <= StrokeOptions::default().tolerance());
    }

    #[test] fn point_only_contours_follow_cap_semantics_and_arc_limits() {
        let mut edges = Vec::new();
        stroke_point((4.0, 5.0).into(), StrokeOptions::new(2.0).unwrap(),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }).unwrap();
        assert!(edges.is_empty());
        stroke_point((4.0, 5.0).into(),
            StrokeOptions::new(2.0).unwrap().with_cap(LineCap::Square),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }).unwrap();
        assert_eq!(edges.len(), 2);

        let options = StrokeOptions::new(100.0).unwrap().with_cap(LineCap::Round)
            .with_tolerance(1e-4).unwrap().with_max_arc_segments(2).unwrap();
        assert!(matches!(stroke_point((0.0, 0.0).into(), options,
            &mut |_: Edge| Ok::<_, Infallible>(())),
            Err(StrokeExpandError::ArcSegmentLimit { maximum: 2, .. })));
    }

    #[test] fn invalid_geometry_and_sink_errors_are_explicit() {
        assert_eq!(stroke_line((f32::NAN, 0.0).into(), (1.0, 0.0).into(),
            StrokeOptions::default(), &mut |_| Ok::<_, &'static str>(())),
            Err(StrokeExpandError::NonFinitePoint));
        assert_eq!(stroke_line((0.0, 0.0).into(), (1.0, 1.0).into(),
            StrokeOptions::default(), &mut |_| Err("full")),
            Err(StrokeExpandError::Sink("full")));
    }

    #[test] fn polyline_joins_support_bevel_round_miter_and_fallback() {
        let points = [(2.0, 4.0).into(), (4.0, 4.0).into(), (4.0, 6.0).into()];
        let collect = |join, miter_limit| {
            let mut edges = Vec::new();
            let options = StrokeOptions::new(2.0).unwrap().with_join(join)
                .with_miter_limit(miter_limit).unwrap();
            stroke_polyline(&points, false, options, &mut |edge| {
                edges.push(edge); Ok::<_, Infallible>(())
            }).unwrap();
            edges
        };
        let has_corner = |edges: &[Edge]| edges.iter().any(|edge|
            [edge.upper, edge.lower].contains(&(5.0, 3.0).into()));
        let bevel = collect(LineJoin::Bevel, 4.0);
        let round = collect(LineJoin::Round, 4.0);
        let miter = collect(LineJoin::Miter, 4.0);
        let fallback = collect(LineJoin::Miter, 1.0);
        assert!(!has_corner(&bevel) && !has_corner(&round));
        assert!(has_corner(&miter) && !has_corner(&fallback));
        assert!(round.len() > bevel.len());
    }

    #[test] fn polylines_ignore_repeated_points_and_closed_contours_have_no_caps() {
        let collect = |points: &[Point], closed| {
            let mut edges = Vec::new();
            stroke_polyline(points, closed,
                StrokeOptions::new(2.0).unwrap().with_cap(LineCap::Square),
                &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }).unwrap();
            edges
        };
        let x_bounds = |edges: &[Edge]| edges.iter().flat_map(|edge|
            [edge.upper.x, edge.lower.x]).fold((f32::INFINITY, f32::NEG_INFINITY),
            |(minimum, maximum), x| (minimum.min(x), maximum.max(x)));
        let plain    = collect(&[(2.0, 3.0).into(), (6.0, 3.0).into()], false);
        let repeated = collect(&[(2.0, 3.0).into(), (2.0, 3.0).into(),
                                 (6.0, 3.0).into(), (6.0, 3.0).into()], false);
        assert_eq!(plain.len(), 2);
        assert_eq!(x_bounds(&plain), x_bounds(&repeated));

        let closed = collect(&[(2.0, 3.0).into(), (6.0, 3.0).into()], true);
        assert_eq!(x_bounds(&plain),  (1.0, 7.0));
        assert_eq!(x_bounds(&closed), (2.0, 6.0));

        let options = StrokeOptions::new(100.0).unwrap().with_cap(LineCap::Round)
            .with_join(LineJoin::Bevel).with_tolerance(1e-4).unwrap()
            .with_max_arc_segments(2).unwrap();
        let mut edges = Vec::new();
        stroke_polyline(&[(0.0, 0.0).into(), (1.0, 0.0).into(), (1.0, 1.0).into()],
            true, options, &mut |edge| {
                edges.push(edge); Ok::<_, Infallible>(())
            }).unwrap();
        assert!(!edges.is_empty());

        edges.clear();
        stroke_polyline(&[(1.0, 1.0).into(), (1.0, 1.0).into()], true, options,
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }).unwrap();
        assert!(edges.is_empty());
    }

    #[test] fn randomized_finite_polylines_emit_only_valid_edges() {
        let (mut seed, mut edges) = (0x5EED_1234_u32, Vec::new());
        let random = |seed: &mut u32| {
            *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((*seed >> 8) as f32 / 0x00FF_FFFF as f32) * 32.0 - 16.0
        };
        for case in 0..512 {
            let len = case * 7 % 9;
            let mut points = Vec::with_capacity(len);
            for index in 0..len {
                let point = if index != 0 && (case + index) % 5 == 0 {
                    points[index - 1]
                } else { (random(&mut seed), random(&mut seed)).into() };
                points.push(point);
            }
            let cap = [LineCap::Butt, LineCap::Round, LineCap::Square][case % 3];
            let join = [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel][case / 3 % 3];
            let options = StrokeOptions::new(0.125 + (case % 16) as f32 * 0.25).unwrap()
                .with_cap(cap).with_join(join);
            edges.clear();
            stroke_polyline(&points, case & 1 != 0, options, &mut |edge| {
                edges.push(edge); Ok::<_, Infallible>(())
            }).unwrap();
            assert!(edges.iter().all(Edge::is_valid), "case {case}: {points:?}");
        }
    }

    #[test] fn polyline_preflight_rejects_arc_budget_and_overflow_before_writing() {
        let mut edges = Vec::new();
        let options = StrokeOptions::new(100.0).unwrap().with_join(LineJoin::Round)
            .with_tolerance(1e-4).unwrap().with_max_arc_segments(2).unwrap();
        assert!(matches!(stroke_polyline(&[(0.0, 0.0).into(),
                        (1.0, 0.0).into(), (1.0, 1.0).into()], false, options,
                &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }),
            Err(StrokeExpandError::ArcSegmentLimit { .. })));
        assert!(edges.is_empty());

        assert_eq!(stroke_polyline(&[(0.0, 0.0).into(), (1.0, 0.0).into(),
                (f32::MAX, f32::MAX).into(), (-f32::MAX, -f32::MAX).into()],
                false, StrokeOptions::default(),
                &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }),
        Err(StrokeExpandError::NonFinitePoint));
        assert!(edges.is_empty());
    }
}
