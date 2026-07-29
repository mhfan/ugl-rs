//! Allocation-free analytic scan conversion for linear directed edges.
//!
//! Each pixel row is split at edge endpoints, edge crossings, and integer-x
//! crossings. Inside each resulting slab, pixel overlap varies linearly in y,
//! so trapezoidal integration is exact apart from `f32` arithmetic.

use crate::{edge::Edge, raster::{emit_coverage_runs, CoverageSink, FillRule, RasterError}};

#[derive(Clone, Copy, Debug, Default)]
pub struct AnalyticIntersection { x: f32, x0: f32, x1: f32, winding: i8 }

pub struct AnalyticWorkspace<'a> {
    pub intersections: &'a mut [AnalyticIntersection],
    pub  row_coverage: &'a mut [f32],
}

pub fn rasterize_edges_analytic<S>(edges: &[Edge], width: u32, height: u32,
    fill_rule: FillRule, workspace: &mut AnalyticWorkspace<'_>, sink: &mut S) ->
    Result<(), RasterError<S::Error>> where S: CoverageSink {
    let width = width as _;
    if workspace.intersections.len() < edges.len() || workspace.row_coverage.len() < width {
        return Err(RasterError::WorkspaceTooSmall {
            intersections: edges.len(), row_coverage: width,
        });
    }
    for y in 0..height {
        let row = &mut workspace.row_coverage[..width];
        row.fill(0.0);
        integrate_row(edges, y as _, fill_rule,
            &mut workspace.intersections[..edges.len()], row);
        emit_coverage_runs(row, y, sink)?;
    }   Ok(())
}

fn integrate_row(edges: &[Edge], row_y: f32, fill_rule: FillRule,
    intersections: &mut [AnalyticIntersection], row: &mut [f32]) {
    let (row_end, mut y0) = (row_y + 1.0, row_y);
    while y0 < row_end {
        let y1 = next_event(edges, y0, row_end);
        if  y1 <= y0 { break; }
        let (middle, mut count) = ((y0 + y1) * 0.5, 0);
        for edge in edges {
            if edge.upper.y < y1 && edge.lower.y > y0 {
                intersections[count] = AnalyticIntersection {
                    x: edge.x_at(middle), x0: edge.x_at(y0),
                    winding: edge.winding, x1: edge.x_at(y1),
                };  count += 1;
            }
        }
        let intersections = &mut intersections[..count];
        intersections.sort_unstable_by(|a, b| a.x.total_cmp(&b.x));
        integrate_spans(intersections, y1 - y0, fill_rule, row);
        y0 = y1;
    }
}

fn next_event(edges: &[Edge], y0: f32, limit: f32) -> f32 {
    let mut next = limit;
    for edge in edges {
        for y in [edge.upper.y, edge.lower.y] { if y > y0 && y < next { next = y; } }
        let active_end = edge.lower.y.min(limit);
        if edge.upper.y < active_end && active_end > y0 {
            let slope = edge.slope();
            if slope != 0.0 {
                let x = edge.x_at(y0);
                let boundary = if slope > 0.0 { libm::floorf(x) + 1.0 }
                    else { libm::ceilf(x) - 1.0 };
                let y = edge.upper.y + (boundary - edge.upper.x) / slope;
                if y > y0 && y < next && y < active_end { next = y; }
            }
        }
    }
    for (index, a) in edges.iter().enumerate() {
        for b in &edges[index + 1..] {
            let start = y0.max(a.upper.y).max(b.upper.y);
            let end = next.min(a.lower.y).min(b.lower.y);
            if  start >= end { continue; }
            let (sa, sb) = (a.slope(), b.slope());
            if sa == sb { continue; }
            let y = (b.upper.x - sb * b.upper.y - a.upper.x + sa * a.upper.y) / (sa - sb);
            if  y > y0 && y < end { next = y; }
        }
    }   next
}

fn integrate_spans(intersections: &[AnalyticIntersection], height: f32,
    fill_rule: FillRule, row: &mut [f32]) {
    let (mut winding, mut left) = (0_i32, None);
    for right in intersections {
        if let Some(left) = left {
            if fill_rule.contains(winding) { integrate_span(left, right, height, row); }
        }
        winding += right.winding as i32;
        left = Some(right);
    }
}

fn integrate_span(left: &AnalyticIntersection, right: &AnalyticIntersection,
    height: f32, row: &mut [f32]) {
    for (x, coverage) in row.iter_mut().enumerate() {
        let x = x as _;
        let overlap0 = (right.x0.min(x + 1.0) - left.x0.max(x)).clamp(0.0, 1.0);
        let overlap1 = (right.x1.min(x + 1.0) - left.x1.max(x)).clamp(0.0, 1.0);
        *coverage += (overlap0 + overlap1) * 0.5 * height;
    }
}

#[cfg(test)] mod tests {
    use alloc::{vec, vec::Vec};
    use core::convert::Infallible;
    use super::{rasterize_edges_analytic, AnalyticIntersection, AnalyticWorkspace};
    use crate::{edge::{build_fill_edges, Edge}, flatten::FlattenOptions,
        geometry::{Affine, PathBuilder}, raster::FillRule};

    fn edges(builder: PathBuilder) -> Vec<Edge> {
        let mut edges = Vec::new();
        build_fill_edges(&builder.build(), Affine::identity(), FlattenOptions::default(),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }).unwrap();
        edges
    }

    fn render(edges: &[Edge], width: u32, height: u32, fill_rule: FillRule) -> Vec<u8> {
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

    #[test] fn aligned_rectangle_has_exact_coverage() {
        let mut builder = PathBuilder::new();
        builder.move_to((1.0, 1.0)).line_to((3.0, 1.0)).unwrap()
            .line_to((3.0, 3.0)).unwrap().line_to((1.0, 3.0)).unwrap();
        assert_eq!(render(&edges(builder), 4, 4, FillRule::NonZero),
            [0, 0, 0, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 0, 0, 0]);
    }

    #[test] fn diagonal_half_pixel_is_integrated_analytically() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((1.0, 0.0)).unwrap()
            .line_to((0.0, 1.0)).unwrap();
        assert_eq!(render(&edges(builder), 1, 1, FillRule::NonZero), [128]);
    }

    #[test] fn fractional_rectangle_has_exact_horizontal_area() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.25, 0.0)).line_to((1.75, 0.0)).unwrap()
            .line_to((1.75, 1.0)).unwrap().line_to((0.25, 1.0)).unwrap();
        assert_eq!(render(&edges(builder), 2, 1, FillRule::NonZero), [191, 191]);
    }

    #[test] fn crossing_edges_are_split_inside_the_row() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((2.0, 2.0)).unwrap()
            .line_to((0.0, 2.0)).unwrap().line_to((2.0, 0.0)).unwrap();
        assert_eq!(render(&edges(builder), 2, 2, FillRule::EvenOdd), [128; 4]);
    }

    #[test] fn nested_contours_distinguish_non_zero_and_even_odd() {
        let mut builder = PathBuilder::new();
        for (x0, y0, x1, y1) in [(0.0, 0.0, 3.0, 3.0), (1.0, 1.0, 2.0, 2.0)] {
            builder.move_to((x0, y0)).line_to((x1, y0)).unwrap()
                .line_to((x1, y1)).unwrap().line_to((x0, y1)).unwrap();
        }
        let edges = edges(builder);
        assert_eq!(render(&edges, 3, 3, FillRule::NonZero)[4], 255);
        assert_eq!(render(&edges, 3, 3, FillRule::EvenOdd)[4], 0);
    }
}
