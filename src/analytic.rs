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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalyticBinRequirements { pub offsets: usize, pub indices: usize }

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum AnalyticBinError {
    DimensionsOverflow,
    OffsetCapacity { required: usize },
    IndexCapacity { required: usize },
}

pub struct AnalyticBinWorkspace<'a> {
    pub row_offsets: &'a mut [u32],
    pub edge_indices: &'a mut [u32],
}

#[derive(Clone, Copy, Debug)]
pub struct AnalyticRowBins<'a> {
    offsets: &'a [u32], indices: &'a [u32], height: u32, edge_count: usize,
}

impl AnalyticRowBins<'_> {
    fn indices(&self, row: u32) -> &[u32] {
        let (start, end) = (
            self.offsets[row as usize] as usize,
            self.offsets[row as usize + 1] as usize,
        );
        &self.indices[start..end]
    }
}

pub fn analytic_bin_requirements(edges: &[Edge], height: u32) ->
    Result<AnalyticBinRequirements, AnalyticBinError> {
    let offsets = usize::try_from(height).map_err(|_| AnalyticBinError::DimensionsOverflow)?
        .checked_add(1).ok_or(AnalyticBinError::DimensionsOverflow)?;
    if edges.len() > u32::MAX as usize { return Err(AnalyticBinError::DimensionsOverflow); }
    let indices = edges.iter().filter(|edge| height != 0 &&
        edge.lower.y > 0.0 && edge.upper.y < height as f32).count();
    Ok(AnalyticBinRequirements { offsets, indices })
}

pub fn build_analytic_row_bins<'a>(edges: &[Edge], height: u32,
    workspace: AnalyticBinWorkspace<'a>) ->
    Result<AnalyticRowBins<'a>, AnalyticBinError> {
    let required = analytic_bin_requirements(edges, height)?;
    if workspace.row_offsets.len() < required.offsets {
        return Err(AnalyticBinError::OffsetCapacity { required: required.offsets });
    }
    if workspace.edge_indices.len() < required.indices {
        return Err(AnalyticBinError::IndexCapacity { required: required.indices });
    }
    let offsets = &mut workspace.row_offsets[..required.offsets];
    let indices = &mut workspace.edge_indices[..required.indices];
    offsets.fill(0);
    let row_of = |edge: Edge| libm::floorf(edge.upper.y)
        .clamp(0.0, height.saturating_sub(1) as f32) as usize;
    for edge in edges {
        if height != 0 && edge.lower.y > 0.0 && edge.upper.y < height as f32 {
            offsets[row_of(*edge) + 1] += 1;
        }
    }
    for row in 1..offsets.len() { offsets[row] += offsets[row - 1]; }
    for (index, edge) in edges.iter().enumerate() {
        if height != 0 && edge.lower.y > 0.0 && edge.upper.y < height as f32 {
            let row = row_of(*edge);
            let position = offsets[row] as usize;
            indices[position] = index as _;
            offsets[row] += 1;
        }
    }
    for row in (1..offsets.len()).rev() { offsets[row] = offsets[row - 1]; }
    offsets[0] = 0;
    for row in 0..height as usize {
        let (start, end) = (offsets[row] as usize, offsets[row + 1] as usize);
        indices[start..end].sort_unstable_by(|left, right| {
            let (left, right) = (edges[*left as usize], edges[*right as usize]);
            left.upper.y.total_cmp(&right.upper.y)
                .then_with(|| left.lower.y.total_cmp(&right.lower.y))
        });
    }
    Ok(AnalyticRowBins { offsets, indices, height, edge_count: edges.len() })
}

pub fn rasterize_edges_analytic_binned<S>(edges: &[Edge], bins: AnalyticRowBins<'_>,
    width: u32, height: u32, fill_rule: FillRule, workspace: &mut AnalyticWorkspace<'_>,
    sink: &mut S) -> Result<(), RasterError<S::Error>> where S: CoverageSink {
    if bins.height != height || bins.edge_count != edges.len() {
        return Err(RasterError::InvalidEdgeBins);
    }
    if edges.iter().any(|edge| !edge.is_valid()) { return Err(RasterError::InvalidEdge); }
    let width = checked_width(width).ok_or(RasterError::DimensionsOverflow)?;
    if workspace.intersections.len() < edges.len() || workspace.row_coverage.len() < width {
        return Err(RasterError::WorkspaceTooSmall {
            intersections: edges.len(), row_coverage: width,
        });
    }
    let mut active_count = 0;
    for y in 0..height {
        active_count = retain_active(workspace.intersections, active_count, y as _);
        let row_edges = bins.indices(y);
        if active_count == 0 && row_edges.is_empty() { continue; }
        let row = &mut workspace.row_coverage[..width];  row.fill(0.0);
        active_count = integrate_binned_row(edges, row_edges, y as _, fill_rule,
            workspace.intersections, active_count, row);
        emit_coverage_runs(row, y, sink)?;
    }   Ok(())
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

fn integrate_binned_row(edges: &[Edge], row_edges: &[u32], row_y: f32,
    fill_rule: FillRule, active: &mut [AnalyticIntersection], mut active_count: usize,
    row: &mut [f32]) -> usize {
    let (row_end, mut y0, mut pending) = (row_y + 1.0, row_y, 0);
    while y0 < row_end {
        active_count = retain_active(active, active_count, y0);
        while let Some(&index) = row_edges.get(pending) {
            let edge = edges[index as usize];
            if edge.upper.y > y0 { break; }
            if edge.lower.y > y0 {
                let (slope, x0) = (edge.slope(), edge.x_at(y0));
                active[active_count] = AnalyticIntersection {
                    x0, x1: x0, slope, y_end: edge.lower.y, winding: edge.winding,
                };
                active_count += 1;
            }
            pending += 1;
        }
        let next_start = row_edges.get(pending)
            .map(|&index| edges[index as usize].upper.y).unwrap_or(row_end).min(row_end);
        if active_count == 0 {
            if next_start >= row_end { break; }
            y0 = next_start;  continue;
        }
        let y1 = prepare_binned_slab(y0, next_start, &mut active[..active_count]);
        if y1 <= y0 { break; }
        integrate_spans(&active[..active_count], y1 - y0, fill_rule, row);
        for edge in &mut active[..active_count] { edge.x0 = edge.x1; }
        y0 = y1;
    }   active_count
}

fn prepare_binned_slab(y0: f32, limit: f32, active: &mut [AnalyticIntersection]) -> f32 {
    let mut next = active.iter().map(|edge| edge.y_end)
        .filter(|&end| end > y0).fold(limit, f32::min);
    for edge in &*active {
        if edge.slope != 0.0 {
            let step = if edge.slope > 0.0 { 1.0 } else { -1.0 };
            let mut boundary = if edge.slope > 0.0 { libm::floorf(edge.x0) + 1.0 }
                else { libm::ceilf(edge.x0) - 1.0 };
            let mut y = y0 + (boundary - edge.x0) / edge.slope;
            if y <= y0 {
                boundary += step;
                y = y0 + (boundary - edge.x0) / edge.slope;
            }
            if y > y0 && y < next && y < edge.y_end { next = y; }
        }
    }
    order_active_edges(active);
    for pair in active.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if a.slope == b.slope { continue; }
        let y = y0 + (b.x0 - a.x0) / (a.slope - b.slope);
        if y > y0 && y < next { next = y; }
    }
    let height = next - y0;
    for intersection in &mut *active {
        intersection.x1 = intersection.x0 + intersection.slope * height;
    }
    order_active_midpoints(active);
    next
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
    order_active_edges(active);
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
    order_active_midpoints(active);
    next
}

fn order_active_edges(active: &mut [AnalyticIntersection]) {
    insertion_sort_active_by(active, |previous, edge|
        previous.x0.total_cmp(&edge.x0)
            .then_with(|| previous.slope.total_cmp(&edge.slope)).is_gt());
}

fn order_active_midpoints(active: &mut [AnalyticIntersection]) {
    insertion_sort_active_by(active, |previous, edge|
        (previous.x0 + previous.x1).total_cmp(&(edge.x0 + edge.x1)).is_gt());
}

fn insertion_sort_active_by(active: &mut [AnalyticIntersection],
    is_after: impl Fn(AnalyticIntersection, AnalyticIntersection) -> bool) {
    for index in 1..active.len() {
        let edge = active[index];
        let mut position = index;
        while position != 0 {
            let previous = active[position - 1];
            if !is_after(previous, edge) { break; }
            active[position] = previous;
            position -= 1;
        }
        active[position] = edge;
    }
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
    let full_start =
        libm::ceilf(left.x0.max(left.x1)).clamp(0.0, row.len() as _) as usize;
    let full_end =
        libm::floorf(right.x0.min(right.x1)).clamp(0.0, row.len() as _) as usize;
    if full_start < full_end {
        integrate_partial_span(left, right, height, row, start, full_start);
        for coverage in &mut row[full_start..full_end] { *coverage += height; }
        integrate_partial_span(left, right, height, row, full_end, end);
    } else {
        integrate_partial_span(left, right, height, row, start, end);
    }
}

fn integrate_partial_span(left: &AnalyticIntersection, right: &AnalyticIntersection,
    height: f32, row: &mut [f32], start: usize, end: usize) {
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

    fn render_binned(edges: &[Edge], width: u32, height: u32,
        fill_rule: FillRule) -> Vec<u8> {
        let requirements = analytic_bin_requirements(edges, height).unwrap();
        let (mut offsets, mut indices) = (
            vec![0; requirements.offsets], vec![0; requirements.indices],
        );
        let bins = build_analytic_row_bins(edges, height, AnalyticBinWorkspace {
            row_offsets: &mut offsets, edge_indices: &mut indices,
        }).unwrap();
        let mut pixels = vec![0; width as usize * height as usize];
        let mut intersections = vec![AnalyticIntersection::default(); edges.len()];
        let mut row = vec![0.0; width as usize];
        rasterize_edges_analytic_binned(edges, bins, width, height, fill_rule,
            &mut AnalyticWorkspace {
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

    #[test] fn analytic_row_bins_report_exact_capacity_and_binding_errors() {
        let edges = [
            Edge { upper: (0.0, -1.0).into(), lower: (0.0, 1.0).into(), winding: -1 },
            Edge { upper: (1.0,  0.5).into(), lower: (1.0, 2.0).into(), winding:  1 },
            Edge { upper: (2.0,  3.0).into(), lower: (2.0, 4.0).into(), winding:  1 },
        ];
        assert_eq!(analytic_bin_requirements(&edges, 2).unwrap(),
            AnalyticBinRequirements { offsets: 3, indices: 2 });
        assert!(matches!(build_analytic_row_bins(&edges, 2, AnalyticBinWorkspace {
                row_offsets: &mut [0; 2], edge_indices: &mut [0; 2],
            }), Err(AnalyticBinError::OffsetCapacity { required: 3 })));
        assert!(matches!(build_analytic_row_bins(&edges, 2, AnalyticBinWorkspace {
                row_offsets: &mut [0; 3], edge_indices: &mut [0; 1],
            }), Err(AnalyticBinError::IndexCapacity { required: 2 })));

        let (mut offsets, mut indices) = ([0; 3], [0; 2]);
        let bins = build_analytic_row_bins(&edges, 2, AnalyticBinWorkspace {
            row_offsets: &mut offsets, edge_indices: &mut indices,
        }).unwrap();
        let (mut intersections, mut row) =
            ([AnalyticIntersection::default(); 3], [0.0; 1]);
        assert_eq!(rasterize_edges_analytic_binned(&edges, bins, 1, 1,
            FillRule::NonZero, &mut AnalyticWorkspace {
                intersections: &mut intersections, row_coverage: &mut row,
            }, &mut |_, _, _| Ok::<_, Infallible>(())),
            Err(RasterError::InvalidEdgeBins));
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
                assert_eq!(render_binned(&edges, 8, 8, fill_rule), analytic,
                    "binned mismatch in case {case}, {fill_rule:?}, points {points:?}");
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
