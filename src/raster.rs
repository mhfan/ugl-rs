//! Deterministic allocation-free reference scan conversion.
//!
//! This module prioritizes a transparent contract over production throughput.
//! It uses stratified vertical samples and exact horizontal span overlap.

use crate::edge::Edge;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FillRule { NonZero, EvenOdd }

impl FillRule {
    pub(crate) fn contains(self, winding: i32) -> bool {
        match self {
            Self::NonZero => winding != 0,
            Self::EvenOdd => winding & 1 != 0,
        }
    }
}

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

pub trait CoverageSink {    type Error;
    /// Receives a non-empty horizontal run with uniform non-zero coverage.
    ///
    /// Producers guarantee that `x + len` is representable and lies inside the
    /// target row. Consumers may therefore stream the run without clipping.
    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error>;

    fn pixel(&mut self, x: u32, y: u32, coverage: u8) -> Result<(), Self::Error> {
        self.span(x, y, 1, coverage)
    }
}

impl<E, F> CoverageSink for F where F: FnMut(u32, u32, u8) -> Result<(), E> {
    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        for x in x..x + len { self(x, y, coverage)?; }  Ok(())
    }   type Error = E;
}

#[derive(Clone, Copy, Debug, PartialEq)] pub enum RasterError<E> {
    WorkspaceTooSmall { intersections: usize, row_coverage: usize },
    DimensionsOverflow, InvalidEdge, InvalidSampleCount, Sink(E),
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
        }

        emit_coverage_runs(row, y, sink)?;
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
        if let Some(from) = previous_x {
            if fill_rule.contains(winding) {
                accumulate_span(from, intersection.x, width, sample_weight, row);
            }
        }
        winding += intersection.winding as i32;
        previous_x = Some(intersection.x);
    }
}

fn accumulate_span(from: f32, to: f32, width: usize, weight: f32, row: &mut [f32]) {
    let start = from.clamp(0.0, width as _);
    let end = to.clamp(0.0, width as _);
    if  end <= start { return; }

    let first = libm::floorf(start) as _;
    let last = (libm::ceilf(end) as usize).min(width);
    for (x, coverage) in row.iter_mut().enumerate().take(last).skip(first) {
        let overlap = end.min(x as f32 + 1.0) - start.max(x as f32);
        *coverage += overlap * weight;
    }
}

#[cfg(test)] mod tests { use super::*;
    use crate::{flatten::FlattenOptions, geometry::{Affine, PathBuilder},
        edge::{build_fill_edges, Edge}};
    use core::convert::Infallible;
    use alloc::{vec, vec::Vec};

    fn path_edges(builder: PathBuilder) -> Vec<Edge> {
        let mut edges = Vec::new();
        build_fill_edges(&builder.build(), Affine::identity(), FlattenOptions::default(),
            &mut |edge| { edges.push(edge); Ok::<_, Infallible>(()) }).unwrap();
        edges
    }

    fn render(edges: &[Edge], width: usize, height: usize, rule: FillRule) -> Vec<u8> {
        let mut pixels = vec![0; width * height];
        let mut intersections = vec![Intersection::default(); edges.len()];
        let mut row_coverage = vec![0.0; width];
        rasterize_edges(edges, width as _, height as _, rule, RasterOptions::default(),
            &mut RasterWorkspace {
                intersections: &mut intersections,
                row_coverage: &mut row_coverage,
            },
            &mut |x, y, coverage| {
                pixels[y as usize * width + x as usize] = coverage;
                Ok::<_, Infallible>(())
            },
        ).unwrap();     pixels
    }

    #[derive(Default)] struct SpanRecorder(Vec<(u32, u32, u32, u8)>);

    impl CoverageSink for SpanRecorder {    type Error = Infallible;
        fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
            Result<(), Self::Error> {
            self.0.push((x, y, len, coverage));     Ok(())
        }
    }

    #[test] fn aligned_rectangle_has_exact_full_coverage() {
        let mut builder = PathBuilder::new();
        builder.move_to((1.0, 1.0)).line_to((3.0, 1.0))
               .line_to((3.0, 3.0)).line_to((1.0, 3.0));
        assert_eq!(render(&path_edges(builder), 4, 4, FillRule::NonZero),
            [0, 0,   0,   0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 0,   0,   0]);
    }

    #[test] fn invalid_public_edges_are_rejected_before_rasterization() {
        let edges = [Edge {
            upper: (f32::NAN, 0.0).into(), lower: (0.0, 1.0).into(), winding: 1,
        }];
        let (mut intersections, mut row) = ([Intersection::default(); 1], [0.0; 1]);
        let result = rasterize_edges(&edges, 1, 1, FillRule::NonZero,
            RasterOptions::default(), &mut RasterWorkspace {
                intersections: &mut intersections, row_coverage: &mut row,
            }, &mut |_, _, _| Ok::<_, Infallible>(()));
        assert_eq!(result, Err(RasterError::InvalidEdge));
    }

    #[test] fn fractional_rectangle_uses_exact_horizontal_span_overlap() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.5, 0.0)).line_to((1.5, 0.0))
               .line_to((1.5, 1.0)).line_to((0.5, 1.0));
        assert_eq!(render(&path_edges(builder), 2, 1, FillRule::NonZero), [128, 128]);
    }

    #[test] fn equal_non_zero_coverage_is_coalesced_into_spans() {
        let mut builder = PathBuilder::new();
        builder.move_to((1.0, 0.0)).line_to((4.0, 0.0))
               .line_to((4.0, 1.0)).line_to((1.0, 1.0));
        let edges = path_edges(builder);
        let (mut intersections, mut row) = (
            vec![Intersection::default(); edges.len()], [0.0; 5]);
        let mut spans = SpanRecorder::default();
        rasterize_edges(&edges, 5, 1, FillRule::NonZero, RasterOptions::default(),
            &mut RasterWorkspace {
                intersections: &mut intersections,
                row_coverage: &mut row,
            }, &mut spans,
        ).unwrap();
        assert_eq!(spans.0, [(1, 0, 3, 255)]);
    }

    #[test] fn non_zero_and_even_odd_differ_for_nested_same_direction_subpaths() {
        let mut builder = PathBuilder::new();
        for (x0, y0, x1, y1) in [(0.0, 0.0, 4.0, 4.0), (1.0, 1.0, 3.0, 3.0)] {
            builder.move_to((x0, y0)).line_to((x1, y0))
                   .line_to((x1, y1)).line_to((x0, y1));
        }
        let edges = path_edges(builder);
        assert_eq!(render(&edges, 4, 4, FillRule::NonZero)[5], 255);
        assert_eq!(render(&edges, 4, 4, FillRule::EvenOdd)[5], 0);
    }

    #[test] fn workspace_requirements_and_sink_errors_are_explicit() {
        let edges = [
            Edge { upper: (0.0, 0.0).into(), lower: (0.0, 1.0).into(), winding: -1 },
            Edge { upper: (1.0, 0.0).into(), lower: (1.0, 1.0).into(), winding: 1 },
        ];
        let (mut intersections, mut row) = ([], [0.0]);
        let result = rasterize_edges(
            &edges, 1, 1, FillRule::NonZero, RasterOptions::default(),
            &mut RasterWorkspace { intersections: &mut intersections, row_coverage: &mut row },
            &mut |_, _, _| Ok::<_, Infallible>(()),
        );
        assert_eq!(result,
            Err(RasterError::WorkspaceTooSmall { intersections: 2, row_coverage: 1 }));

        let mut intersections = [Intersection::default(); 2];
        let result = rasterize_edges(&edges, 1, 1, FillRule::NonZero, RasterOptions::default(),
            &mut RasterWorkspace { intersections: &mut intersections, row_coverage: &mut row },
            &mut |_, _, _| Err("stop"),
        );
        assert_eq!(result, Err(RasterError::Sink("stop")));
    }
}
