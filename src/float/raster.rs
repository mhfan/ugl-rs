//! Supersampled floating-point reference rasterizer.

use crate::{edge::Edge, float::{ceil, floor}, raster::{CoverageSink, FillRule}};

#[derive(Clone, Copy, Debug, PartialEq)] pub struct RasterOptions {
    /// Number of deterministic vertical samples per pixel row.
    pub vertical_samples: u16,
}

impl Default for RasterOptions { fn default() -> Self { Self { vertical_samples: 256 } } }

#[derive(Clone, Copy, Debug, Default)] pub struct Intersection { x: f32, winding: i8 }

pub struct RasterWorkspace<'a> {
    pub intersections: &'a mut [Intersection],
    pub row_coverage: &'a mut [f32],
}

#[derive(Clone, Copy, Debug, PartialEq)] pub enum RasterError<E> {
    WorkspaceTooSmall { intersections: usize, row_coverage: usize },
    DimensionsOverflow, InvalidEdge, InvalidEdgeBins, InvalidSampleCount, Sink(E),
}

pub fn rasterize_edges<S>(edges: &[Edge], width: u32, height: u32, fill_rule: FillRule,
    options: RasterOptions, workspace: &mut RasterWorkspace<'_>, sink: &mut S) ->
    Result<(), RasterError<S::Error>> where S: CoverageSink {
    if options.vertical_samples == 0 { return Err(RasterError::InvalidSampleCount); }
    if edges.iter().any(|edge| !edge.is_valid()) { return Err(RasterError::InvalidEdge); }
    let width = checked_width(width).ok_or(RasterError::DimensionsOverflow)?;
    if workspace.intersections.len() < edges.len() || workspace.row_coverage.len() < width {
        return Err(RasterError::WorkspaceTooSmall {
            intersections: edges.len(), row_coverage: width,
        });
    }

    let sample_count = options.vertical_samples as usize;
    let sample_scale = 1.0 / options.vertical_samples as f32;
    for y in 0..height {
        let row = &mut workspace.row_coverage[..width];
        row.fill(0.0);

        for sample in 0..sample_count {
            let sample_y = y as f32 + (sample as f32 + 0.5) * sample_scale;
            let count = collect_intersections(
                edges, sample_y, &mut workspace.intersections[..edges.len()],
            );
            let intersections = &mut workspace.intersections[..count];
            intersections.sort_unstable_by(|a, b| a.x.total_cmp(&b.x));
            accumulate_spans(intersections, width, fill_rule, sample_scale, row);
        }   emit_coverage_runs(row, y, sink)?;
    }   Ok(())
}

pub(crate) fn checked_width(width: u32) -> Option<usize> { usize::try_from(width).ok() }

pub(crate) fn emit_coverage_runs<S>(row: &[f32], y: u32, sink: &mut S) ->
    Result<(), RasterError<S::Error>> where S: CoverageSink {
    let quantize = |coverage: f32| (coverage.clamp(0.0, 1.0) * 255.0 + 0.5) as _;
    let mut x = 0;
    while x < row.len() {
        let coverage = quantize(row[x]);
        if  coverage == 0 { x += 1; continue; }
        let start = x;      x += 1;
        while x < row.len() && quantize(row[x]) == coverage { x += 1; }
        sink.span(start as _, y, (x - start) as _, coverage).map_err(RasterError::Sink)?;
    }   Ok(())
}

fn collect_intersections(edges: &[Edge], y: f32, output: &mut [Intersection]) -> usize {
    let mut count = 0;
    for edge in edges {
        if edge.upper.y <= y && y < edge.lower.y {
            output[count] = Intersection { x: edge.x_at(y), winding: edge.winding };
            count += 1;
        }
    }       count
}

fn accumulate_spans(intersections: &[Intersection], width: usize, fill_rule: FillRule,
    sample_weight: f32, row: &mut [f32]) {
    let (mut winding, mut previous_x) = (0_i32, None);
    for intersection in intersections {
        if let Some(from) = previous_x && fill_rule.contains(winding) {
            accumulate_span(from, intersection.x, width, sample_weight, row);
        }
        winding += intersection.winding as i32;
        previous_x = Some(intersection.x);
    }
}

fn accumulate_span(from: f32, to: f32, width: usize, weight: f32, row: &mut [f32]) {
    let start = from.clamp(0.0, width as _);
    let end = to.clamp(0.0, width as _);
    if  end <= start { return; }

    let first = floor(start) as _;
    let last = (ceil(end) as usize).min(width);
    for (x, coverage) in row.iter_mut().enumerate().take(last).skip(first) {
        let overlap = end.min(x as f32 + 1.0) - start.max(x as f32);
        *coverage += overlap * weight;
    }
}
