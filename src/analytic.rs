//! Allocation-free analytic scan conversion for linear directed edges.
//!
//! Each pixel row is split at edge endpoints, edge crossings, and integer-x
//! crossings. Inside each resulting slab, pixel overlap varies linearly in y,
//! so trapezoidal integration is exact apart from `f32` arithmetic.
//!
//! Active edges persist in caller-owned storage across slabs and rows. Empty
//! vertical ranges are skipped, while active edges are ordered only at event
//! boundaries. Only adjacent pairs need crossing checks because the first
//! future ordering change must occur between neighbors.

use crate::{edge::Edge,
    raster::{checked_width, emit_coverage_runs, CoverageSink, FillRule, RasterError}
};

#[derive(Clone, Copy, Debug, Default)]
pub struct AnalyticIntersection { x0: f32, x1: f32, slope: f32, y_end: f32, winding: i8 }

pub struct AnalyticWorkspace<'a> {
    pub intersections: &'a mut [AnalyticIntersection],
    pub  row_coverage: &'a mut [f32],
}

pub fn rasterize_edges_analytic<S>(edges: &[Edge], width: u32, height: u32,
    fill_rule: FillRule, workspace: &mut AnalyticWorkspace<'_>, sink: &mut S) ->
    Result<(), RasterError<S::Error>> where S: CoverageSink {
    if edges.iter().any(|edge| !edge.is_valid()) { return Err(RasterError::InvalidEdge); }
    let width = checked_width(width).ok_or(RasterError::DimensionsOverflow)?;
    if workspace.intersections.len() < edges.len() || workspace.row_coverage.len() < width {
        return Err(RasterError::WorkspaceTooSmall {
            intersections: edges.len(), row_coverage: width,
        });
    }
    let Some((mut y, last_row)) = occupied_rows(edges, height) else { return Ok(()); };
    let (mut active_count, mut first_slab) = (0, true);
    while y < last_row {
        active_count = retain_active(workspace.intersections, active_count, y as _);
        if active_count == 0 {
            let Some(next) = next_occupied_row(edges, y, last_row) else { break; };
            y = next;
        }
        let row = &mut workspace.row_coverage[..width];
        row.fill(0.0);
        active_count = integrate_row(edges, y as _, fill_rule,
            workspace.intersections, active_count, first_slab, row);
        emit_coverage_runs(row, y, sink)?;
        first_slab = false;     y += 1;
    }   Ok(())
}

fn integrate_row(edges: &[Edge], row_y: f32, fill_rule: FillRule,
    active: &mut [AnalyticIntersection], mut active_count: usize, first_slab: bool,
    row: &mut [f32]) -> usize {
    let (row_end, mut y0) = (row_y + 1.0, row_y);
    let mut include_spanning = first_slab;
    while y0 < row_end {
        active_count = retain_active(active, active_count, y0);
        active_count = activate_edges(edges, y0, include_spanning, active, active_count);
        include_spanning = false;
        if active_count == 0 {
            let next = edges.iter().map(|edge| edge.upper.y)
                .filter(|&start| start > y0).fold(row_end, f32::min);
            if next >= row_end { break; }
            y0 = next;  continue;
        }
        let  y1 = prepare_active_slab(edges, y0, row_end, &mut active[..active_count]);
        if   y1 <= y0 { break; }
        integrate_spans(&active[..active_count], y1 - y0, fill_rule, row);
        for edge in &mut active[..active_count] { edge.x0 = edge.x1; }
        y0 = y1;
    }   active_count
}

fn occupied_rows(edges: &[Edge], height: u32) -> Option<(u32, u32)> {
    let first = edges.iter().map(|edge| libm::floorf(edge.upper.y))
        .fold(f32::INFINITY, f32::min).clamp(0.0, height as _) as u32;
    let last = edges.iter().map(|edge| libm::ceilf(edge.lower.y))
        .fold(f32::NEG_INFINITY, f32::max).clamp(0.0, height as _) as u32;
    (first < last).then_some((first, last))
}

fn next_occupied_row(edges: &[Edge], current: u32, limit: u32) -> Option<u32> {
    let current_y = current as f32;
    edges.iter().filter(|edge| edge.lower.y > current_y)
        .map(|edge| libm::floorf(edge.upper.y).max(current_y) as u32)
        .filter(|&row| row < limit).min()
}

fn retain_active(active: &mut [AnalyticIntersection], count: usize, y: f32) -> usize {
    let mut retained = 0;
    for index in 0..count {
        if active[index].y_end > y {
            active[retained] = active[index];
            retained += 1;
        }
    }       retained
}

fn activate_edges(edges: &[Edge], y: f32, include_spanning: bool,
    active: &mut [AnalyticIntersection], mut count: usize) -> usize {
    for edge in edges {
        if edge.upper.y == y || include_spanning && edge.upper.y < y && edge.lower.y > y {
            let (slope, x0) = (edge.slope(), edge.x_at(y));
            active[count] = AnalyticIntersection {
                x0, x1: x0, slope, y_end: edge.lower.y, winding: edge.winding,
            };
            count += 1;
        }
    }       count
}

fn prepare_active_slab(edges: &[Edge], y0: f32, limit: f32,
    active: &mut [AnalyticIntersection]) -> f32 {
    let mut next = active.iter().map(|edge| edge.y_end)
        .filter(|&end| end > y0).fold(limit, f32::min);
    for start in edges.iter().map(|edge| edge.upper.y) {
        if start > y0 && start < next { next = start; }
    }
    for edge in &*active {
        if edge.slope != 0.0 {
            let step = if edge.slope > 0.0 { 1.0 } else { -1.0 };
            let mut boundary = if edge.slope > 0.0 { libm::floorf(edge.x0) + 1.0 }
                else { libm::ceilf(edge.x0) - 1.0 };
            let mut y = y0 + (boundary - edge.x0) / edge.slope;
            if  y <= y0 {
                boundary += step;
                y =  y0 + (boundary - edge.x0) / edge.slope;
            }
            if  y >  y0 && y < next && y < edge.y_end { next = y; }
        }
    }
    active.sort_unstable_by(|a, b|
        a.x0.total_cmp(&b.x0).then_with(|| a.slope.total_cmp(&b.slope)));
    for pair in active.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if a.slope == b.slope { continue; }
        let y = y0 + (b.x0 - a.x0) / (a.slope - b.slope);
        if  y > y0 && y < next { next = y; }
    }
    let height = next - y0;
    for intersection in &mut *active {
        intersection.x1 = intersection.x0 + intersection.slope * height;
    }
    active.sort_unstable_by(|a, b| (a.x0 + a.x1).total_cmp(&(b.x0 + b.x1)));
    next
}

fn integrate_spans(intersections: &[AnalyticIntersection], height: f32,
    fill_rule: FillRule, row: &mut [f32]) {
    let (mut winding, mut left) = (0_i32, None);
    for right in intersections {
        if let Some(left) = left && fill_rule.contains(winding) {
            integrate_span(left, right, height, row);
        }
        winding += right.winding as i32;
        left = Some(right);
    }
}

fn integrate_span(left: &AnalyticIntersection,
                 right: &AnalyticIntersection, height: f32, row: &mut [f32]) {
    let start = libm::floorf(left.x0.min( left.x1)).clamp(0.0, row.len() as _) as _;
    let end   = libm::ceilf(right.x0.max(right.x1)).clamp(0.0, row.len() as _) as _;
    for (x, coverage) in row.iter_mut().enumerate().take(end).skip(start) {
        let x = x as _;
        let overlap0 = (right.x0.min(x + 1.0) - left.x0.max(x)).clamp(0.0, 1.0);
        let overlap1 = (right.x1.min(x + 1.0) - left.x1.max(x)).clamp(0.0, 1.0);
        *coverage += (overlap0 + overlap1) * 0.5 * height;
    }
}

#[cfg(test)] mod tests { use super::*;
    use alloc::{vec, vec::Vec};
    use core::convert::Infallible;
    use crate::{flatten::FlattenOptions, geometry::{Affine, PathBuilder},
        raster::{rasterize_edges, FillRule, Intersection, RasterOptions, RasterWorkspace},
        edge::{build_fill_edges, Edge},
    };

    fn edges(builder: PathBuilder) -> Vec<Edge> {
        let mut edges = Vec::new();
        build_fill_edges(&builder.build(), Affine::identity(), FlattenOptions::default(),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }).unwrap();
        edges
    }

    fn render_analytic(edges: &[Edge], width: u32, height: u32,
        fill_rule: FillRule) -> Vec<u8> {
        let mut pixels = vec![0; width as usize * height as usize];
        let mut intersections = vec![AnalyticIntersection::default(); edges.len()];
        let mut row = vec![0.0; width as usize];
        rasterize_edges_analytic(edges, width, height, fill_rule, &mut AnalyticWorkspace {
                intersections: &mut intersections, row_coverage: &mut row,
            }, &mut |x, y, coverage| {
                pixels[(y * width + x) as usize] = coverage;
                Ok::<_, Infallible>(())
            },
        ).unwrap();     pixels
    }

    fn render_sampled(edges: &[Edge], width: u32, height: u32,
        fill_rule: FillRule) -> Vec<u8> {
        let mut pixels = vec![0; width as usize * height as usize];
        let mut intersections = vec![Intersection::default(); edges.len()];
        let mut row  = vec![0.0; width as usize];
        rasterize_edges(edges, width, height, fill_rule,
            RasterOptions { vertical_samples: 8192 }, &mut RasterWorkspace {
                intersections: &mut intersections, row_coverage: &mut row,
            }, &mut |x, y, coverage| {
                pixels[(y * width + x) as usize] = coverage;
                Ok::<_, Infallible>(())
            },
        ).unwrap();     pixels
    }

    #[test] fn aligned_rectangle_has_exact_coverage() {
        let mut builder = PathBuilder::new();
        builder.move_to((1.0, 1.0)).line_to((3.0, 1.0))
               .line_to((3.0, 3.0)).line_to((1.0, 3.0));
        assert_eq!(render_analytic(&edges(builder), 4, 4, FillRule::NonZero),
            [0, 0, 0, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 0, 0, 0]);
    }

    #[test] fn invalid_public_edges_are_rejected_before_rasterization() {
        let edges = [Edge { upper: (0.0, 1.0).into(), lower: (0.0, 0.0).into(), winding: 1 }];
        let (mut intersections, mut row) =
            ([AnalyticIntersection::default(); 1], [0.0; 1]);
        let result = rasterize_edges_analytic(&edges, 1, 1, FillRule::NonZero,
            &mut AnalyticWorkspace {
                intersections: &mut intersections, row_coverage: &mut row,
            }, &mut |_, _, _| Ok::<_, Infallible>(()));
        assert_eq!(result, Err(RasterError::InvalidEdge));
    }

    #[test] fn diagonal_half_pixel_is_integrated_analytically() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((1.0, 0.0)).line_to((0.0, 1.0));
        assert_eq!(render_analytic(&edges(builder), 1, 1, FillRule::NonZero), [128]);
    }

    #[test] fn fractional_rectangle_has_exact_horizontal_area() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.25, 0.0)).line_to((1.75, 0.0))
               .line_to((1.75, 1.0)).line_to((0.25, 1.0));
        assert_eq!(render_analytic(&edges(builder), 2, 1, FillRule::NonZero), [191, 191]);
    }

    #[test] fn crossing_edges_are_split_inside_the_row() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((2.0, 2.0))
               .line_to((0.0, 2.0)).line_to((2.0, 0.0));
        assert_eq!(render_analytic(&edges(builder), 2, 2, FillRule::EvenOdd), [128; 4]);
    }

    #[test] fn nested_contours_distinguish_non_zero_and_even_odd() {
        let mut builder = PathBuilder::new();
        for (x0, y0, x1, y1) in [(0.0, 0.0, 3.0, 3.0), (1.0, 1.0, 2.0, 2.0)] {
            builder.move_to((x0, y0)).line_to((x1, y0))
                   .line_to((x1, y1)).line_to((x0, y1));
        }
        let edges = edges(builder);
        assert_eq!(render_analytic(&edges, 3, 3, FillRule::NonZero)[4], 255);
        assert_eq!(render_analytic(&edges, 3, 3, FillRule::EvenOdd)[4], 0);
    }

    #[test] fn workspace_requirements_and_sink_errors_are_explicit() {
        let edges = [
            Edge { upper: (0.0, 0.0).into(), lower: (0.0, 1.0).into(), winding: -1 },
            Edge { upper: (1.0, 0.0).into(), lower: (1.0, 1.0).into(), winding: 1 },
        ];
        let (mut intersections, mut row) = ([], [0.0]);
        let result = rasterize_edges_analytic(&edges, 1, 1, FillRule::NonZero,
            &mut AnalyticWorkspace {
                intersections: &mut intersections, row_coverage: &mut row,
            }, &mut |_, _, _| Ok::<_, Infallible>(()),
        );
        assert_eq!(result,
            Err(RasterError::WorkspaceTooSmall { intersections: 2, row_coverage: 1 }));

        let mut intersections = [AnalyticIntersection::default(); 2];
        let result = rasterize_edges_analytic(&edges, 1, 1, FillRule::NonZero,
            &mut AnalyticWorkspace {
                intersections: &mut intersections, row_coverage: &mut row,
            }, &mut |_, _, _| Err("stop"),
        );
        assert_eq!(result, Err(RasterError::Sink("stop")));
    }

    #[test] fn empty_or_fully_clipped_targets_emit_no_spans() {
        let edges = [
            Edge { upper: (2.0, 0.0).into(), lower: (2.0, 1.0).into(), winding: -1 },
            Edge { upper: (3.0, 0.0).into(), lower: (3.0, 1.0).into(), winding: 1 },
        ];
        let mut intersections = [AnalyticIntersection::default(); 2];
        let mut row = [0.0];
        let mut calls = 0;
        for (width, height) in [(0, 1), (1, 0), (1, 1)] {
            rasterize_edges_analytic(&edges, width, height, FillRule::NonZero,
                &mut AnalyticWorkspace {
                    intersections: &mut intersections, row_coverage: &mut row,
                }, &mut |_, _, _| { calls += 1; Ok::<_, Infallible>(()) },
            ).unwrap();
        }
        assert_eq!(calls, 0);
    }

    #[test] fn analytic_polygons_match_high_sample_reference() {
        let mut state = 0x7a31_4f29_u32;
        for case in 0..32 {
            let mut builder = PathBuilder::new();
            let mut point = || {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 * (10.0 / 16_777_216.0) - 1.0
            };
            let points: Vec<_> = (0..3 + case % 4).map(|_| (point(), point())).collect();
            builder.move_to(points[0]).line_to(points[1]).line_to(points[2]);
            for &point in &points[3..] { builder.line_to(point); }
            let edges = edges(builder);
            for fill_rule in [FillRule::NonZero, FillRule::EvenOdd] {
                let (analytic, sampled) = (
                    render_analytic(&edges, 8, 8, fill_rule),
                    render_sampled(&edges, 8, 8, fill_rule),
                );
                for (pixel, (&actual, &reference)) in
                    analytic.iter().zip(&sampled).enumerate() {
                    assert!(actual.abs_diff(reference) <= 1,
                        "case {case}, pixel {pixel}, {fill_rule:?}, points {points:?}: \
                         analytic={actual}, sampled={reference}");
                }
            }
        }
    }
}
