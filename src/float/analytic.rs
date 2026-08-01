//! Allocation-free analytic scan conversion for linear directed edges.
//!
//! The production sparse-cell path splits rows only at edge endpoints and real
//! crossings, integrates boundary cells in closed form, and records full spans
//! as range deltas. The retained dense reference path additionally splits at
//! integer-x crossings and integrates trapezoids into a row buffer.
//!
//! Active edges persist in caller-owned storage across slabs and rows. Empty
//! vertical ranges are skipped, while active edges are ordered only at event
//! boundaries. Only adjacent pairs need crossing checks because the first
//! future ordering change must occur between neighbors.

use crate::{common::{edge::Edge, raster::{CoverageSink, FillRule}}, float::{ceil, floor,
        raster::{checked_width, emit_coverage_runs, RasterError}},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct Intersection { x0: f32, x1: f32, slope: f32, y_end: f32, winding: i8 }

pub struct Workspace<'a> {
    pub intersections: &'a mut [Intersection],
    pub  row_coverage: &'a mut [f32],
}

#[derive(Clone, Copy, Debug, Default)] pub struct Cell { coverage: f32, delta: f32 }

pub struct CellWorkspace<'a> {
    pub intersections: &'a mut [Intersection],
    pub cells: &'a mut [Cell],
}

#[derive(Clone, Copy)] struct CellRange { start: usize, end: usize }

impl CellRange {
    const EMPTY: Self = Self { start: usize::MAX, end: 0 };
    fn include(&mut self, start: usize, end: usize) {
        if start >= end { return; }
        self.start = self.start.min(start);
        self.end = self.end.max(end);
    }
    fn is_empty(self) -> bool { self.start == usize::MAX }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinRequirements { pub offsets: usize, pub indices: usize }

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum BinError {
    DimensionsOverflow,
    OffsetCapacity { required: usize },
    IndexCapacity { required: usize },
}

pub struct BinWorkspace<'a> {
    pub row_offsets: &'a mut [u32],
    pub edge_indices: &'a mut [u32],
}

#[derive(Clone, Copy, Debug)]
pub struct RowBins<'a> {
    offsets: &'a [u32], indices: &'a [u32], height: u32, edge_count: usize,
}

impl RowBins<'_> {
    fn indices(&self, row: u32) -> &[u32] {
        let (start, end) = (
            self.offsets[row as usize] as usize,
            self.offsets[row as usize + 1] as usize,
        );
        &self.indices[start..end]
    }
}

pub fn bin_requirements(edges: &[Edge], height: u32) ->
    Result<BinRequirements, BinError> {
    let offsets = usize::try_from(height).map_err(|_| BinError::DimensionsOverflow)?
        .checked_add(1).ok_or(BinError::DimensionsOverflow)?;
    if edges.len() > u32::MAX as usize { return Err(BinError::DimensionsOverflow); }
    let indices = edges.iter().filter(|edge| height != 0 &&
        edge.lower.y > 0.0 && edge.upper.y < height as f32).count();
    Ok(BinRequirements { offsets, indices })
}

pub fn build_row_bins<'a>(edges: &[Edge], height: u32,
    workspace: BinWorkspace<'a>) ->
    Result<RowBins<'a>, BinError> {
    let required = bin_requirements(edges, height)?;
    if workspace.row_offsets.len() < required.offsets {
        return Err(BinError::OffsetCapacity { required: required.offsets });
    }
    if workspace.edge_indices.len() < required.indices {
        return Err(BinError::IndexCapacity { required: required.indices });
    }
    let offsets = &mut workspace.row_offsets[..required.offsets];
    let indices = &mut workspace.edge_indices[..required.indices];
    offsets.fill(0);
    let row_of = |edge: Edge| floor(edge.upper.y)
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
    Ok(RowBins { offsets, indices, height, edge_count: edges.len() })
}

pub fn rasterize_edges_binned<S>(edges: &[Edge], bins: RowBins<'_>,
    width: u32, height: u32, fill_rule: FillRule, workspace: &mut Workspace<'_>,
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

/// Rasterizes exact-area coverage through sparse per-pixel accumulators.
///
/// Boundary pixels receive direct analytic area, while guaranteed-full spans
/// become two range-delta writes and one fused prefix scan during run emission.
pub fn rasterize_edges_cells<S>(edges: &[Edge], bins: RowBins<'_>, width: u32,
    height: u32, fill_rule: FillRule, workspace: &mut CellWorkspace<'_>,
    sink: &mut S) -> Result<(), RasterError<S::Error>> where S: CoverageSink {
    rasterize_edges_cells_region(edges, bins, (width, height), fill_rule,
        (0, 0, width, height), workspace, sink)
}

pub(crate) fn rasterize_edges_cells_region<S>(edges: &[Edge], bins: RowBins<'_>,
    dimensions: (u32, u32), fill_rule: FillRule, region: (u32, u32, u32, u32),
    workspace: &mut CellWorkspace<'_>, sink: &mut S) ->
    Result<(), RasterError<S::Error>> where S: CoverageSink {
    let (width, height) = dimensions;
    if bins.height != height || bins.edge_count != edges.len() {
        return Err(RasterError::InvalidEdgeBins);
    }
    if edges.iter().any(|edge| !edge.is_valid()) { return Err(RasterError::InvalidEdge); }
    checked_width(width).ok_or(RasterError::DimensionsOverflow)?;
    let (x0, y0, x1, y1) = region;
    let (x0, y0, x1, y1) = (
        x0.min(width), y0.min(height), x1.min(width), y1.min(height));
    let region_width = checked_width(x1.saturating_sub(x0))
        .ok_or(RasterError::DimensionsOverflow)?;
    if workspace.intersections.len() < edges.len() ||
        workspace.cells.len() < region_width {
        return Err(RasterError::WorkspaceTooSmall {
            intersections: edges.len(), row_coverage: region_width,
        });
    }
    if x0 >= x1 || y0 >= y1 { return Ok(()); }
    let mut active_count = 0;
    for edge in edges {
        if y0 != 0 && edge.upper.y < y0 as f32 && edge.lower.y > y0 as f32 {
            let (slope, x) = (edge.slope(), edge.x_at(y0 as _));
            workspace.intersections[active_count] = Intersection {
                x0: x, x1: x, slope, y_end: edge.lower.y, winding: edge.winding,
            };
            active_count += 1;
        }
    }
    order_cell_edges(&mut workspace.intersections[..active_count]);
    let mut previous_dirty = CellRange::EMPTY;
    let mut reusable_vertical_count = None;
    let mut initialized = false;
    for y in y0..y1 {
        active_count = retain_active(workspace.intersections, active_count, y as _);
        let row_edges = bins.indices(y);
        if active_count == 0 && row_edges.is_empty() { continue; }
        let cells = &mut workspace.cells[..region_width];
        let active = &workspace.intersections[..active_count];
        if row_edges.is_empty() && reusable_vertical_count == Some(active_count) &&
            vertical_edges_span_row(active, y) {
            emit_vertical_runs(active, fill_rule, x0, region_width as _, y, sink)?;
            continue;
        }
        if row_edges.is_empty() && prepare_direct_row(
            &mut workspace.intersections[..active_count], y) &&
            emit_disjoint_row_spans(&workspace.intersections[..active_count], fill_rule,
                x0, region_width as _, y, sink)? {
            for edge in &mut workspace.intersections[..active_count] { edge.x0 = edge.x1; }
            reusable_vertical_count = None;
            continue;
        }
        if initialized {
            if !previous_dirty.is_empty() {
                cells[previous_dirty.start..previous_dirty.end].fill(Cell::default());
            }
        } else {
            cells.fill(Cell::default());
            initialized = true;
        }
        let (next_active, dirty) = integrate_binned_row_cells(
            edges, row_edges, y as _, fill_rule,
            workspace.intersections, active_count, (cells, x0 as _));
        active_count = next_active;
        if !dirty.is_empty() {
            emit_cell_runs(&cells[dirty.start..dirty.end],
                x0 as usize + dirty.start, y, sink)?;
        }
        reusable_vertical_count = (row_edges.is_empty() &&
            vertical_edges_span_row(&workspace.intersections[..active_count], y))
            .then_some(active_count);
        previous_dirty = dirty;
    }
    Ok(())
}

fn prepare_direct_row(active: &mut [Intersection], y: u32) -> bool {
    let row_end = y as f32 + 1.0;
    if active.is_empty() || active.iter().any(|edge| edge.y_end < row_end) {
        return false;
    }
    for edge in &mut *active { edge.x1 = edge.x0 + edge.slope; }
    !active.windows(2).any(|pair| pair[0].x1 > pair[1].x1)
}

fn emit_disjoint_row_spans<S>(intersections: &[Intersection], fill_rule: FillRule,
    x_origin: u32, width: u32, y: u32, sink: &mut S) ->
    Result<bool, RasterError<S::Error>> where S: CoverageSink {
    let mut previous_end = 0;
    let (mut winding, mut left) = (0_i32, None::<&Intersection>);
    for right in intersections {
        if let Some(left) = left && fill_rule.contains(winding) {
            let start = floor(left.x0.min(left.x1) - x_origin as f32)
                .clamp(0.0, width as _) as u32;
            let end = ceil(right.x0.max(right.x1) - x_origin as f32)
                .clamp(0.0, width as _) as u32;
            if start < previous_end { return Ok(false); }
            previous_end = end;
        }
        winding += right.winding as i32;
        left = Some(right);
    }

    fn flush<S>(run: &mut Option<(u32, u32, u8)>, x_origin: u32, y: u32,
        sink: &mut S) -> Result<(), RasterError<S::Error>> where S: CoverageSink {
        let Some((x, len, coverage)) = run.take() else { return Ok(()); };
        if coverage != 0 {
            sink.span(x_origin + x, y, len, coverage).map_err(RasterError::Sink)?;
        }
        Ok(())
    }
    fn append<S>(run: &mut Option<(u32, u32, u8)>, x: u32, len: u32, coverage: u8,
        x_origin: u32, y: u32, sink: &mut S) -> Result<(), RasterError<S::Error>>
        where S: CoverageSink {
        if len == 0 { return Ok(()); }
        if let Some((run_x, run_len, run_coverage)) = run {
            if *run_x + *run_len == x && *run_coverage == coverage {
                *run_len += len;
                return Ok(());
            }
            flush(run, x_origin, y, sink)?;
        }
        *run = Some((x, len, coverage));
        Ok(())
    }

    let (mut winding, mut left, mut run) = (0_i32, None::<&Intersection>, None);
    for right in intersections {
        if let Some(left) = left && fill_rule.contains(winding) {
            let start = floor(left.x0.min(left.x1) - x_origin as f32)
                .clamp(0.0, width as _) as u32;
            let end = ceil(right.x0.max(right.x1) - x_origin as f32)
                .clamp(0.0, width as _) as u32;
            let full_start = ceil(left.x0.max(left.x1) - x_origin as f32)
                .clamp(0.0, width as _) as u32;
            let full_end = floor(right.x0.min(right.x1) - x_origin as f32)
                .clamp(0.0, width as _) as u32;
            if full_start < full_end {
                for x in start..full_start {
                    let cell_x = x_origin as f32 + x as f32;
                    let area = integrate_clamped_line(
                        right.x0 - cell_x, right.x1 - cell_x, 1.0) -
                        integrate_clamped_line(left.x0 - cell_x, left.x1 - cell_x, 1.0);
                    append(&mut run, x, 1,
                        (area.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                        x_origin, y, sink)?;
                }
                append(&mut run, full_start, full_end - full_start, u8::MAX,
                    x_origin, y, sink)?;
                for x in full_end..end {
                    let cell_x = x_origin as f32 + x as f32;
                    let area = integrate_clamped_line(
                        right.x0 - cell_x, right.x1 - cell_x, 1.0) -
                        integrate_clamped_line(left.x0 - cell_x, left.x1 - cell_x, 1.0);
                    append(&mut run, x, 1,
                        (area.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                        x_origin, y, sink)?;
                }
            } else {
                for x in start..end {
                    let cell_x = x_origin as f32 + x as f32;
                    let area = integrate_clamped_line(
                        right.x0 - cell_x, right.x1 - cell_x, 1.0) -
                        integrate_clamped_line(left.x0 - cell_x, left.x1 - cell_x, 1.0);
                    append(&mut run, x, 1,
                        (area.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                        x_origin, y, sink)?;
                }
            }
        }
        winding += right.winding as i32;
        left = Some(right);
    }
    flush(&mut run, x_origin, y, sink)?;
    Ok(true)
}

fn vertical_edges_span_row(active: &[Intersection], y: u32) -> bool {
    !active.is_empty() && active.iter().all(|edge|
        edge.slope == 0.0 && edge.y_end >= y as f32 + 1.0)
}

fn emit_vertical_runs<S>(intersections: &[Intersection], fill_rule: FillRule,
    x_origin: u32, width: u32, y: u32, sink: &mut S) ->
    Result<(), RasterError<S::Error>> where S: CoverageSink {
    fn emit_pending<S>(pending: &mut Option<(u32, f32)>, x_origin: u32, y: u32,
        sink: &mut S) -> Result<(), RasterError<S::Error>> where S: CoverageSink {
        let Some((x, coverage)) = pending.take() else { return Ok(()); };
        let coverage = (coverage.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        if coverage != 0 {
            sink.span(x_origin + x, y, 1, coverage).map_err(RasterError::Sink)?;
        }
        Ok(())
    }

    fn add_partial<S>(pending: &mut Option<(u32, f32)>, x: u32, coverage: f32,
        x_origin: u32, y: u32, sink: &mut S) ->
        Result<(), RasterError<S::Error>> where S: CoverageSink {
        if let Some((pending_x, pending_coverage)) = pending {
            if *pending_x == x {
                *pending_coverage += coverage;
                return Ok(());
            }
            emit_pending(pending, x_origin, y, sink)?;
        }
        *pending = Some((x, coverage));
        Ok(())
    }

    let (mut winding, mut left, mut pending) =
        (0_i32, None::<&Intersection>, None);
    for right in intersections {
        if let Some(left) = left && fill_rule.contains(winding) {
            let (left, right) = (
                (left.x0 - x_origin as f32).clamp(0.0, width as _),
                (right.x0 - x_origin as f32).clamp(0.0, width as _),
            );
            if left < right {
                let (start, end) = (floor(left) as u32, ceil(right) as u32);
                if end - start <= 1 {
                    add_partial(&mut pending, start, right - left,
                        x_origin, y, sink)?;
                } else {
                    let (full_start, full_end) = (ceil(left) as u32, floor(right) as u32);
                    if left < full_start as f32 {
                        add_partial(&mut pending, full_start - 1,
                            full_start as f32 - left, x_origin, y, sink)?;
                    }
                    emit_pending(&mut pending, x_origin, y, sink)?;
                    if full_start < full_end {
                        sink.span(x_origin + full_start, y, full_end - full_start, u8::MAX)
                            .map_err(RasterError::Sink)?;
                    }
                    if (full_end as f32) < right {
                        add_partial(&mut pending, full_end, right - full_end as f32,
                            x_origin, y, sink)?;
                    }
                }
            }
        }
        winding += right.winding as i32;
        left = Some(right);
    }
    emit_pending(&mut pending, x_origin, y, sink)
}

fn integrate_binned_row_cells(edges: &[Edge], row_edges: &[u32], row_y: f32,
    fill_rule: FillRule, active: &mut [Intersection], mut active_count: usize,
    row: (&mut [Cell], f32)) -> (usize, CellRange) {
    let (cells, x_origin) = row;
    let (row_end, mut y0, mut pending) = (row_y + 1.0, row_y, 0);
    let mut dirty = CellRange::EMPTY;
    while y0 < row_end {
        active_count = retain_active(active, active_count, y0);
        let before_activation = active_count;
        while let Some(&index) = row_edges.get(pending) {
            let edge = edges[index as usize];
            if edge.upper.y > y0 { break; }
            if edge.lower.y > y0 {
                let (slope, x0) = (edge.slope(), edge.x_at(y0));
                active[active_count] = Intersection {
                    x0, x1: x0, slope, y_end: edge.lower.y, winding: edge.winding,
                };
                active_count += 1;
            }
            pending += 1;
        }
        if active_count != before_activation {
            order_new_cell_edges(&mut active[..active_count], before_activation);
        }
        let next_start = row_edges.get(pending)
            .map(|&index| edges[index as usize].upper.y).unwrap_or(row_end).min(row_end);
        if active_count == 0 {
            if next_start >= row_end { break; }
            y0 = next_start;  continue;
        }
        let (y1, crossing, coalesced) =
            prepare_cell_slab(y0, next_start, &mut active[..active_count]);
        if y1 <= y0 { break; }
        if coalesced {
            integrate_coalesced_cell_spans(&active[..active_count], y1 - y0,
                fill_rule, cells, x_origin, &mut dirty);
        } else {
            integrate_cell_spans(&active[..active_count], y1 - y0,
                fill_rule, cells, x_origin, &mut dirty);
        }
        for edge in &mut active[..active_count] { edge.x0 = edge.x1; }
        if crossing { order_cell_edges(&mut active[..active_count]); }
        y0 = y1;
    }
    (active_count, dirty)
}

fn prepare_cell_slab(y0: f32, limit: f32,
    active: &mut [Intersection]) -> (f32, bool, bool) {
    let mut next = limit;
    if active.iter().all(|edge| edge.slope == 0.0) {
        for edge in &*active {
            if edge.y_end > y0 { next = next.min(edge.y_end); }
        }
        for edge in active { edge.x1 = edge.x0; }
        return (next, false, false);
    }
    for edge in &*active {
        if edge.y_end > y0 { next = next.min(edge.y_end); }
    }
    let mut crossing = false;
    for pair in active.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let relative = a.slope - b.slope;
        if relative <= 0.0 { continue; }
        let separation = b.x0 - a.x0;
        let epsilon = f32::EPSILON * y0.abs().max(1.0) * 4.0;
        if separation <= relative * epsilon
            || separation > relative * (next - y0) { continue; }
        next = y0 + separation / relative;
        crossing = true;
    }
    let height = next - y0;
    for edge in &mut *active { edge.x1 = edge.x0 + edge.slope * height; }
    // A crossing inside the numerical event tolerance can be too close to split
    // safely, but its midpoint order must still define the span pairing.
    let reordered = active.windows(2).any(|pair|
        pair[0].x0 + pair[0].x1 > pair[1].x0 + pair[1].x1);
    if reordered {
        order_active_midpoints(active);
        crossing = true;
    }
    let coalesced = reordered && active.windows(2).any(|pair|
        (pair[1].x0 - pair[0].x0) * (pair[1].x1 - pair[0].x1) < 0.0);
    (next, crossing, coalesced)
}

fn integrate_cell_spans(intersections: &[Intersection], height: f32,
    fill_rule: FillRule, cells: &mut [Cell], x_origin: f32,
    dirty: &mut CellRange) {
    let (mut winding, mut left) = (0_i32, None);
    for right in intersections {
        if let Some(left) = left && fill_rule.contains(winding) {
            integrate_cell_span(left, right, height, cells, x_origin, dirty);
        }
        winding += right.winding as i32;
        left = Some(right);
    }
}

fn integrate_cell_span(left: &Intersection, right: &Intersection,
    height: f32, cells: &mut [Cell], x_origin: f32, dirty: &mut CellRange) {
    let start = floor(left.x0.min(left.x1) - x_origin)
        .clamp(0.0, cells.len() as _) as _;
    let end = ceil(right.x0.max(right.x1) - x_origin)
        .clamp(0.0, cells.len() as _) as _;
    let full_start = ceil(left.x0.max(left.x1) - x_origin)
        .clamp(0.0, cells.len() as _) as usize;
    let full_end = floor(right.x0.min(right.x1) - x_origin)
        .clamp(0.0, cells.len() as _) as usize;
    dirty.include(start, end);
    if full_start < full_end {
        integrate_partial_cells(left, right, height, cells, x_origin, start, full_start);
        cells[full_start].delta += height;
        if full_end < cells.len() {
            cells[full_end].delta -= height;
            dirty.include(full_end, full_end + 1);
        }
        integrate_partial_cells(left, right, height, cells, x_origin, full_end, end);
    } else {
        integrate_partial_cells(left, right, height, cells, x_origin, start, end);
    }
}

fn integrate_partial_cells(left: &Intersection, right: &Intersection,
    height: f32, cells: &mut [Cell], x_origin: f32, start: usize, end: usize) {
    for (x, cell) in cells.iter_mut().enumerate().take(end).skip(start) {
        let x = x_origin + x as f32;
        let right_area = integrate_clamped_line(right.x0 - x, right.x1 - x, height);
        let left_area = integrate_clamped_line(left.x0 - x, left.x1 - x, height);
        cell.coverage += (right_area - left_area).max(0.0);
    }
}

fn integrate_coalesced_cell_spans(intersections: &[Intersection], height: f32,
    fill_rule: FillRule, cells: &mut [Cell], x_origin: f32,
    dirty: &mut CellRange) {
    let (mut winding, mut left) = (0_i32, None::<&Intersection>);
    for right in intersections {
        if let Some(left) = left && fill_rule.contains(winding) {
            let (d0, d1) = (right.x0 - left.x0, right.x1 - left.x1);
            if d0 * d1 < 0.0 {
                let start = floor(left.x0.min(left.x1).min(right.x0).min(right.x1) -
                    x_origin).clamp(0.0, cells.len() as _) as _;
                let end = ceil(left.x0.max(left.x1).max(right.x0).max(right.x1) -
                    x_origin).clamp(0.0, cells.len() as _) as _;
                dirty.include(start, end);
                integrate_crossing_cells(
                    left, right, height, cells, x_origin, start, end);
            } else {
                integrate_cell_span(left, right, height, cells, x_origin, dirty);
            }
        }
        winding += right.winding as i32;
        left = Some(right);
    }
}

fn integrate_crossing_cells(left: &Intersection, right: &Intersection,
    height: f32, cells: &mut [Cell], x_origin: f32, start: usize, end: usize) {
    // Numerically coalesced crossings can leave a paired boundary reversed
    // for part of a slab. Split there so coverage integrates |right-left|
    // instead of clamping only the final signed integral.
    let (d0, d1) = (right.x0 - left.x0, right.x1 - left.x1);
    let t = d0 / (d0 - d1);
    let first_height = height * t;
    for (x, cell) in cells.iter_mut().enumerate().take(end).skip(start) {
        let x = x_origin + x as f32;
        let (l0, l1, r0, r1) =
            (left.x0 - x, left.x1 - x, right.x0 - x, right.x1 - x);
        let middle = l0 + (l1 - l0) * t;
        let first = integrate_clamped_line(r0, middle, first_height)
            - integrate_clamped_line(l0, middle, first_height);
        let second_height = height - first_height;
        let second = integrate_clamped_line(middle, l1, second_height)
            - integrate_clamped_line(middle, r1, second_height);
        cell.coverage += first.abs() + second.abs();
    }
}

fn integrate_clamped_line(start: f32, end: f32, height: f32) -> f32 {
    // For a cell [x, x + 1], its instantaneous covered width is
    // clamp(right - x, 0, 1) - clamp(left - x, 0, 1).  Integrating each
    // affine clamp independently reduces boundary-cell clipping to the
    // piecewise primitive 0, z²/2, z - 1/2.
    let (low, high) = if start < end { (start, end) } else { (end, start) };
    if high <= 0.0 { return 0.0; }
    if low >= 1.0 { return height; }
    if low >= 0.0 && high <= 1.0 { return (start + end) * 0.5 * height; }
    let primitive = |value: f32| {
        if value <= 0.0 { 0.0 }
        else if value < 1.0 { value * value * 0.5 }
        else { value - 0.5 }
    };
    (primitive(high) - primitive(low)) * height / (high - low)
}

fn emit_cell_runs<S>(cells: &[Cell], x_offset: usize, y: u32,
    sink: &mut S) -> Result<(), RasterError<S::Error>> where S: CoverageSink {
    let quantize = |coverage: f32| (coverage.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let (mut accumulated, mut run_start, mut run_coverage) = (0.0, 0, 0);
    for (x, cell) in cells.iter().enumerate() {
        accumulated += cell.delta;
        let coverage = quantize(accumulated + cell.coverage);
        if coverage == run_coverage { continue; }
        if run_coverage != 0 {
            sink.span((x_offset + run_start) as _, y, (x - run_start) as _, run_coverage)
                .map_err(RasterError::Sink)?;
        }
        run_start = x;  run_coverage = coverage;
    }
    if run_coverage != 0 {
        sink.span((x_offset + run_start) as _, y,
            (cells.len() - run_start) as _, run_coverage)
            .map_err(RasterError::Sink)?;
    }
    Ok(())
}

pub fn rasterize_edges<S>(edges: &[Edge], width: u32, height: u32,
    fill_rule: FillRule, workspace: &mut Workspace<'_>, sink: &mut S) ->
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
    active: &mut [Intersection], mut active_count: usize, first_slab: bool,
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
    fill_rule: FillRule, active: &mut [Intersection], mut active_count: usize,
    row: &mut [f32]) -> usize {
    let (row_end, mut y0, mut pending) = (row_y + 1.0, row_y, 0);
    while y0 < row_end {
        active_count = retain_active(active, active_count, y0);
        while let Some(&index) = row_edges.get(pending) {
            let edge = edges[index as usize];
            if edge.upper.y > y0 { break; }
            if edge.lower.y > y0 {
                let (slope, x0) = (edge.slope(), edge.x_at(y0));
                active[active_count] = Intersection {
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

fn prepare_binned_slab(y0: f32, limit: f32, active: &mut [Intersection]) -> f32 {
    let mut next = limit;
    // Vertical edges never cross x boundaries or each other. The initial x
    // ordering is still required when a newly activated batch is unordered.
    if active.iter().all(|edge| edge.slope == 0.0) {
        for edge in &*active {
            if edge.y_end > y0 { next = next.min(edge.y_end); }
        }
        order_active_edges(active);
        for edge in active { edge.x1 = edge.x0; }
        return next;
    }
    for edge in &*active {
        if edge.y_end > y0 { next = next.min(edge.y_end); }
        if edge.slope != 0.0 {
            let step = if edge.slope > 0.0 { 1.0 } else { -1.0 };
            let mut boundary = if edge.slope > 0.0 { floor(edge.x0) + 1.0 }
                else { ceil(edge.x0) - 1.0 };
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
        if is_distinct_event(y, y0) && y < next { next = y; }
    }
    let height = next - y0;
    for intersection in &mut *active {
        intersection.x1 = intersection.x0 + intersection.slope * height;
    }
    order_active_midpoints(active);
    next
}

fn occupied_rows(edges: &[Edge], height: u32) -> Option<(u32, u32)> {
    let first = edges.iter().map(|edge| floor(edge.upper.y))
        .fold(f32::INFINITY, f32::min).clamp(0.0, height as _) as u32;
    let last = edges.iter().map(|edge| ceil(edge.lower.y))
        .fold(f32::NEG_INFINITY, f32::max).clamp(0.0, height as _) as u32;
    (first < last).then_some((first, last))
}

fn next_occupied_row(edges: &[Edge], current: u32, limit: u32) -> Option<u32> {
    let current_y = current as f32;
    edges.iter().filter(|edge| edge.lower.y > current_y)
        .map(|edge| floor(edge.upper.y).max(current_y) as u32)
        .filter(|&row| row < limit).min()
}

fn retain_active(active: &mut [Intersection], count: usize, y: f32) -> usize {
    let mut retained = 0;
    for index in 0..count {
        if active[index].y_end > y {
            active[retained] = active[index];
            retained += 1;
        }
    }       retained
}

fn activate_edges(edges: &[Edge], y: f32, include_spanning: bool,
    active: &mut [Intersection], mut count: usize) -> usize {
    for edge in edges {
        if edge.upper.y == y || include_spanning && edge.upper.y < y && edge.lower.y > y {
            let (slope, x0) = (edge.slope(), edge.x_at(y));
            active[count] = Intersection {
                x0, x1: x0, slope, y_end: edge.lower.y, winding: edge.winding,
            };
            count += 1;
        }
    }       count
}

fn prepare_active_slab(edges: &[Edge], y0: f32, limit: f32,
    active: &mut [Intersection]) -> f32 {
    let mut next = limit;
    for start in edges.iter().map(|edge| edge.upper.y) {
        if start > y0 && start < next { next = start; }
    }
    // Keep this path equivalent to the binned implementation above.
    if active.iter().all(|edge| edge.slope == 0.0) {
        for edge in &*active {
            if edge.y_end > y0 { next = next.min(edge.y_end); }
        }
        order_active_edges(active);
        for edge in active { edge.x1 = edge.x0; }
        return next;
    }
    for edge in &*active {
        if edge.y_end > y0 { next = next.min(edge.y_end); }
        if edge.slope != 0.0 {
            let step = if edge.slope > 0.0 { 1.0 } else { -1.0 };
            let mut boundary = if edge.slope > 0.0 { floor(edge.x0) + 1.0 }
                else { ceil(edge.x0) - 1.0 };
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
        if is_distinct_event(y, y0) && y < next { next = y; }
    }
    let height = next - y0;
    for intersection in &mut *active {
        intersection.x1 = intersection.x0 + intersection.slope * height;
    }
    order_active_midpoints(active);
    next
}

fn order_active_edges(active: &mut [Intersection]) {
    insertion_sort_active_by(active, |previous, edge|
        previous.x0.total_cmp(&edge.x0)
            .then_with(|| previous.slope.total_cmp(&edge.slope)).is_gt());
}

fn order_cell_edges(active: &mut [Intersection]) {
    order_new_cell_edges(active, 1);
}

fn order_new_cell_edges(active: &mut [Intersection], start: usize) {
    insertion_sort_active_from_by(active, start, |previous, edge| {
        let tolerance = f32::EPSILON * previous.x0.abs().max(edge.x0.abs()).max(1.0) * 8.0;
        if (previous.x0 - edge.x0).abs() <= tolerance {
            previous.slope.total_cmp(&edge.slope).is_gt()
        } else {
            previous.x0.total_cmp(&edge.x0).is_gt()
        }
    });
}

fn is_distinct_event(y: f32, current: f32) -> bool {
    y - current > f32::EPSILON * current.abs().max(1.0) * 4.0
}

fn order_active_midpoints(active: &mut [Intersection]) {
    insertion_sort_active_by(active, |previous, edge|
        (previous.x0 + previous.x1).total_cmp(&(edge.x0 + edge.x1)).is_gt());
}

fn insertion_sort_active_by(active: &mut [Intersection],
    is_after: impl Fn(Intersection, Intersection) -> bool) {
    insertion_sort_active_from_by(active, 1, is_after);
}

fn insertion_sort_active_from_by(active: &mut [Intersection], start: usize,
    is_after: impl Fn(Intersection, Intersection) -> bool) {
    for index in start.max(1)..active.len() {
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

fn integrate_spans(intersections: &[Intersection], height: f32,
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

fn integrate_span(left: &Intersection,
                 right: &Intersection, height: f32, row: &mut [f32]) {
    let start = floor(left.x0.min(left.x1)).clamp(0.0, row.len() as _) as _;
    let end   = ceil(right.x0.max(right.x1)).clamp(0.0, row.len() as _) as _;
    let full_start =
        ceil(left.x0.max(left.x1)).clamp(0.0, row.len() as _) as usize;
    let full_end =
        floor(right.x0.min(right.x1)).clamp(0.0, row.len() as _) as usize;
    if full_start < full_end {
        integrate_partial_span(left, right, height, row, start, full_start);
        for coverage in &mut row[full_start..full_end] { *coverage += height; }
        integrate_partial_span(left, right, height, row, full_end, end);
    } else {
        integrate_partial_span(left, right, height, row, start, end);
    }
}

fn integrate_partial_span(left: &Intersection, right: &Intersection,
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
    use crate::{float::flatten::{FlattenOptions, build_fill_edges},
        common::{geometry::{Affine, PathBuilder}, raster::FillRule, edge::Edge},
        float::raster::{rasterize_edges as rasterize_edges_sampled,
            Intersection as SampledIntersection, RasterOptions, RasterWorkspace},
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
        let mut intersections = vec![Intersection::default(); edges.len()];
        let mut row = vec![0.0; width as usize];
        rasterize_edges(edges, width, height, fill_rule, &mut Workspace {
                intersections: &mut intersections, row_coverage: &mut row,
            }, &mut |x, y, coverage| {
                pixels[(y * width + x) as usize] = coverage;
                Ok::<_, Infallible>(())
            },
        ).unwrap();     pixels
    }

    fn render_binned(edges: &[Edge], width: u32, height: u32,
        fill_rule: FillRule) -> Vec<u8> {
        let requirements = bin_requirements(edges, height).unwrap();
        let (mut offsets, mut indices) = (
            vec![0; requirements.offsets], vec![0; requirements.indices],
        );
        let bins = build_row_bins(edges, height, BinWorkspace {
            row_offsets: &mut offsets, edge_indices: &mut indices,
        }).unwrap();
        let mut pixels = vec![0; width as usize * height as usize];
        let mut intersections = vec![Intersection::default(); edges.len()];
        let mut row = vec![0.0; width as usize];
        rasterize_edges_binned(edges, bins, width, height, fill_rule,
            &mut Workspace {
                intersections: &mut intersections, row_coverage: &mut row,
            }, &mut |x, y, coverage| {
                pixels[(y * width + x) as usize] = coverage;
                Ok::<_, Infallible>(())
            },
        ).unwrap();     pixels
    }

    fn render_cells(edges: &[Edge], width: u32, height: u32,
        fill_rule: FillRule) -> Vec<u8> {
        let requirements = bin_requirements(edges, height).unwrap();
        let (mut offsets, mut indices) = (
            vec![0; requirements.offsets], vec![0; requirements.indices],
        );
        let bins = build_row_bins(edges, height, BinWorkspace {
            row_offsets: &mut offsets, edge_indices: &mut indices,
        }).unwrap();
        let mut pixels = vec![0; width as usize * height as usize];
        let mut intersections = vec![Intersection::default(); edges.len()];
        let mut cells = vec![Cell { coverage: 3.0, delta: -2.0 }; width as usize];
        rasterize_edges_cells(edges, bins, width, height, fill_rule,
            &mut CellWorkspace { intersections: &mut intersections, cells: &mut cells },
            &mut |x, y, coverage| {
                pixels[(y * width + x) as usize] = coverage;
                Ok::<_, Infallible>(())
            },
        ).unwrap();     pixels
    }

    fn render_cells_region(edges: &[Edge], width: u32, height: u32,
        region: (u32, u32, u32, u32), fill_rule: FillRule) -> Vec<u8> {
        let requirements = bin_requirements(edges, height).unwrap();
        let (mut offsets, mut indices) = (
            vec![0; requirements.offsets], vec![0; requirements.indices]);
        let bins = build_row_bins(edges, height, BinWorkspace {
            row_offsets: &mut offsets, edge_indices: &mut indices,
        }).unwrap();
        let mut pixels = vec![0; width as usize * height as usize];
        let mut intersections = vec![Intersection::default(); edges.len()];
        let mut cells = vec![Cell::default(); (region.2 - region.0) as usize];
        rasterize_edges_cells_region(edges, bins, (width, height), fill_rule, region,
            &mut CellWorkspace { intersections: &mut intersections, cells: &mut cells },
            &mut |x, y, coverage| {
                pixels[(y * width + x) as usize] = coverage;
                Ok::<_, Infallible>(())
            }).unwrap();
        pixels
    }

    fn render_sampled(edges: &[Edge], width: u32, height: u32,
        fill_rule: FillRule) -> Vec<u8> {
        let mut pixels = vec![0; width as usize * height as usize];
        let mut intersections = vec![SampledIntersection::default(); edges.len()];
        let mut row  = vec![0.0; width as usize];
        rasterize_edges_sampled(edges, width, height, fill_rule,
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
            ([Intersection::default(); 1], [0.0; 1]);
        let result = rasterize_edges(&edges, 1, 1, FillRule::NonZero,
            &mut Workspace {
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

    #[test] fn repeated_vertical_rows_merge_disjoint_intervals_in_one_pixel() {
        let mut builder = PathBuilder::new();
        for (left, right) in [(0.1, 0.3), (0.6, 0.9)] {
            builder.move_to((left, 0.0)).line_to((right, 0.0))
                .line_to((right, 3.0)).line_to((left, 3.0));
        }
        let edges = edges(builder);
        for fill_rule in [FillRule::NonZero, FillRule::EvenOdd] {
            assert_eq!(render_cells(&edges, 1, 3, fill_rule), [127, 127, 127]);
        }
    }

    #[test] fn local_region_matches_full_raster_with_spanning_edges() {
        let mut builder = PathBuilder::new();
        builder.move_to((-1.0, -1.0)).line_to((9.0, 1.25))
            .line_to((7.5, 9.0)).line_to((0.5, 7.25));
        let edges = edges(builder);
        let (width, height, region) = (8, 8, (2, 2, 7, 7));
        let full = render_cells(&edges, width, height, FillRule::NonZero);
        let local = render_cells_region(&edges, width, height, region, FillRule::NonZero);
        for y in 0..height { for x in 0..width {
            let index = (y * width + x) as usize;
            if x >= region.0 && x < region.2 && y >= region.1 && y < region.3 {
                assert_eq!(local[index], full[index], "({x}, {y})");
            } else { assert_eq!(local[index], 0, "({x}, {y})"); }
        } }
    }

    #[test] fn clamped_line_integral_covers_each_piecewise_region() {
        for (start, end, expected) in [
            (-1.0, -0.5, 0.0), (1.5, 2.0, 1.0), (0.25, 0.75, 0.5),
            (-1.0, 2.0, 0.5), (-1.0, 0.5, 1.0 / 12.0),
            (0.5, 2.0, 11.0 / 12.0),
        ] {
            let actual = integrate_clamped_line(start, end, 1.0);
            let reverse = integrate_clamped_line(end, start, 1.0);
            assert!((actual - expected).abs() < 1e-6, "{start}..{end}: {actual}");
            assert!((reverse - expected).abs() < 1e-6, "{end}..{start}: {reverse}");
        }
    }

    #[test] fn newly_activated_edges_merge_into_the_ordered_prefix() {
        let edge = |x| Intersection {
            x0: x, x1: x, slope: 0.0, y_end: 2.0, winding: 1,
        };
        let mut incremental = [edge(1.0), edge(4.0), edge(3.0), edge(2.0)];
        let mut reference = incremental;
        order_new_cell_edges(&mut incremental, 2);
        order_cell_edges(&mut reference);
        assert_eq!(incremental.map(|edge| edge.x0), reference.map(|edge| edge.x0));
    }

    #[test] fn crossing_edges_are_split_inside_the_row() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((2.0, 2.0))
               .line_to((0.0, 2.0)).line_to((2.0, 0.0));
        assert_eq!(render_analytic(&edges(builder), 2, 2, FillRule::EvenOdd), [128; 4]);
    }

    #[test] fn coincident_crossings_are_coalesced_without_changing_coverage() {
        let edges: Vec<_> = (0..16).map(|index| Edge {
            upper: (index as f32 * 0.45 + 0.5, 0.25).into(),
            lower: ((15 - index) as f32 * 0.45 + 0.5, 7.75).into(),
            winding: if index & 1 == 0 { -1 } else { 1 },
        }).collect();
        for fill_rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let (analytic, sampled) = (
                render_analytic(&edges, 8, 8, fill_rule),
                render_sampled(&edges, 8, 8, fill_rule),
            );
            assert_eq!(render_binned(&edges, 8, 8, fill_rule), analytic);
            assert_eq!(render_cells(&edges, 8, 8, fill_rule), analytic);
            for (&actual, &reference) in analytic.iter().zip(&sampled) {
                assert!(actual.abs_diff(reference) <= 1,
                    "{fill_rule:?}: analytic={actual}, sampled={reference}");
            }
        }
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
        let result = rasterize_edges(&edges, 1, 1, FillRule::NonZero,
            &mut Workspace {
                intersections: &mut intersections, row_coverage: &mut row,
            }, &mut |_, _, _| Ok::<_, Infallible>(()),
        );
        assert_eq!(result,
            Err(RasterError::WorkspaceTooSmall { intersections: 2, row_coverage: 1 }));

        let mut intersections = [Intersection::default(); 2];
        let result = rasterize_edges(&edges, 1, 1, FillRule::NonZero,
            &mut Workspace {
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
        assert_eq!(bin_requirements(&edges, 2).unwrap(),
            BinRequirements { offsets: 3, indices: 2 });
        assert!(matches!(build_row_bins(&edges, 2, BinWorkspace {
                row_offsets: &mut [0; 2], edge_indices: &mut [0; 2],
            }), Err(BinError::OffsetCapacity { required: 3 })));
        assert!(matches!(build_row_bins(&edges, 2, BinWorkspace {
                row_offsets: &mut [0; 3], edge_indices: &mut [0; 1],
            }), Err(BinError::IndexCapacity { required: 2 })));

        let (mut offsets, mut indices) = ([0; 3], [0; 2]);
        let bins = build_row_bins(&edges, 2, BinWorkspace {
            row_offsets: &mut offsets, edge_indices: &mut indices,
        }).unwrap();
        let (mut intersections, mut row) =
            ([Intersection::default(); 3], [0.0; 1]);
        assert_eq!(rasterize_edges_binned(&edges, bins, 1, 1,
            FillRule::NonZero, &mut Workspace {
                intersections: &mut intersections, row_coverage: &mut row,
            }, &mut |_, _, _| Ok::<_, Infallible>(())),
            Err(RasterError::InvalidEdgeBins));
    }

    #[test] fn empty_or_fully_clipped_targets_emit_no_spans() {
        let edges = [
            Edge { upper: (2.0, 0.0).into(), lower: (2.0, 1.0).into(), winding: -1 },
            Edge { upper: (3.0, 0.0).into(), lower: (3.0, 1.0).into(), winding: 1 },
        ];
        let mut intersections = [Intersection::default(); 2];
        let mut row = [0.0];
        let mut calls = 0;
        for (width, height) in [(0, 1), (1, 0), (1, 1)] {
            rasterize_edges(&edges, width, height, FillRule::NonZero,
                &mut Workspace {
                    intersections: &mut intersections, row_coverage: &mut row,
                }, &mut |_, _, _| { calls += 1; Ok::<_, Infallible>(()) },
            ).unwrap();
        }
        assert_eq!(calls, 0);
    }

    #[test] fn analytic_polygons_match_high_sample_reference() {
        let mut state = 0x7a31_4f29_u32;
        for case in 0..128 {
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
                let cells = render_cells(&edges, 8, 8, fill_rule);
                if case < 32 {
                    assert_eq!(render_binned(&edges, 8, 8, fill_rule), analytic,
                        "binned mismatch in case {case}, {fill_rule:?}, points {points:?}");
                }
                for (pixel, (&cell, &reference)) in cells.iter().zip(&sampled).enumerate() {
                    assert!(cell.abs_diff(reference) <= 1,
                        "cell mismatch in case {case}, pixel {pixel}, {fill_rule:?}, \
                         points {points:?}: cell={cell}, sampled={reference}, \
                         analytic={}", analytic[pixel]);
                    if case < 32 {
                        assert!(cell.abs_diff(analytic[pixel]) <= 1,
                            "cell/dense mismatch in case {case}, pixel {pixel}, \
                             {fill_rule:?}, points {points:?}: cell={cell}, \
                             analytic={}", analytic[pixel]);
                    }
                }
                if case < 32 {
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

    #[test] fn sparse_cells_handle_complex_even_odd_self_intersection() {
        let points = [
            (5.809253, 4.2083783), (7.8110056, 0.8302816), (1.4902749, 6.7414265),
            (-0.66801107, 5.8478894), (4.8945427, 5.9314833),
            (-0.78560984, -0.17064488), (4.099824, 6.719939),
        ];
        let mut builder = PathBuilder::new();
        builder.move_to(points[0]);
        for &point in &points[1..] { builder.line_to(point); }
        let edges = edges(builder);
        let (cells, reference) = (
            render_cells(&edges, 8, 8, FillRule::EvenOdd),
            render_analytic(&edges, 8, 8, FillRule::EvenOdd),
        );
        for (pixel, (&cell, reference)) in cells.iter().zip(reference).enumerate() {
            assert!(cell.abs_diff(reference) <= 1,
                "pixel {pixel}: cells={cell}, reference={reference}");
        }
    }
}
