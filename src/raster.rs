//! Deterministic allocation-free reference scan conversion.
//!
//! This module prioritizes a transparent contract over production throughput.
//! It uses stratified vertical samples and exact horizontal span overlap.

use crate::edge::Edge;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FillRule { NonZero, EvenOdd }

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
    fn pixel(&mut self, x: usize, y: usize, coverage: u8) -> Result<(), Self::Error>;
}

impl<E, F> CoverageSink for F where F: FnMut(usize, usize, u8) -> Result<(), E> {
    type Error = E;

    fn pixel(&mut self, x: usize, y: usize, coverage: u8) -> Result<(), Self::Error> {
        self(x, y, coverage)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)] pub enum RasterError<E> {
    InvalidSampleCount, Sink(E),
    WorkspaceTooSmall { intersections: usize, row_coverage: usize },
}

pub fn rasterize_edges<S>(edges: &[Edge], width: usize, height: usize, fill_rule: FillRule,
    options: RasterOptions, workspace: &mut RasterWorkspace<'_>, sink: &mut S) ->
    Result<(), RasterError<S::Error>> where S: CoverageSink {
    if options.vertical_samples == 0 { return Err(RasterError::InvalidSampleCount); }
    if workspace.intersections.len() < edges.len() || workspace.row_coverage.len() < width {
        return Err(RasterError::WorkspaceTooSmall {
            intersections: edges.len(),
            row_coverage: width,
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

        for (x, coverage) in row.iter().copied().enumerate() {
            let coverage = (coverage.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            if coverage != 0 {
                sink.pixel(x, y, coverage).map_err(RasterError::Sink)?;
            }
        }
    }   Ok(())
}

fn collect_intersections(edges: &[Edge], y: f32, output: &mut [Intersection]) -> usize {
    let mut count = 0;
    for edge in edges {
        if edge.upper.y <= y && y < edge.lower.y {
            let dy = edge.lower.y - edge.upper.y;
            let t = (y - edge.upper.y) / dy;
            output[count] = Intersection {
                x: edge.upper.x + (edge.lower.x - edge.upper.x) * t,
                winding: edge.winding,
            };  count += 1;
        }
    }           count
}

fn accumulate_spans(intersections: &[Intersection], width: usize, fill_rule: FillRule,
    sample_weight: f32, row: &mut [f32]) {
    let (mut winding, mut previous_x) = (0_i32, None);
    for intersection in intersections {
        if let Some(from) = previous_x {
            let inside = match fill_rule {
                FillRule::NonZero => winding != 0,
                FillRule::EvenOdd => winding & 1 != 0,
            };
            if inside { accumulate_span(from, intersection.x, width, sample_weight, row); }
        }
        winding += intersection.winding as i32;
        previous_x = Some(intersection.x);
    }
}

fn accumulate_span(from: f32, to: f32, width: usize, weight: f32, row: &mut [f32]) {
    let start = from.clamp(0.0, width as f32);
    let end = to.clamp(0.0, width as f32);
    if  end <= start { return; }

    let first = libm::floorf(start) as usize;
    let last = (libm::ceilf(end) as usize).min(width);
    for (x, coverage) in row.iter_mut().enumerate().take(last).skip(first) {
        let overlap = end.min(x as f32 + 1.0) - start.max(x as f32);
        *coverage += overlap * weight;
    }
}

#[cfg(test)] mod tests {
    use alloc::vec;
    use alloc::vec::Vec;
    use core::convert::Infallible;
    use super::{rasterize_edges, FillRule, Intersection, RasterError, RasterOptions,
        RasterWorkspace,
    };
    use crate::edge::{build_fill_edges, Edge};
    use crate::flatten::FlattenOptions;
    use crate::geometry::{Affine, PathBuilder};

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
        rasterize_edges(
            edges, width, height, rule, RasterOptions::default(),
            &mut RasterWorkspace {
                intersections: &mut intersections,
                row_coverage: &mut row_coverage,
            },
            &mut |x, y, coverage| {
                pixels[y * width + x] = coverage;
                Ok::<_, Infallible>(())
            },
        ).unwrap();
        pixels
    }

    #[test] fn aligned_rectangle_has_exact_full_coverage() {
        let mut builder = PathBuilder::new();
        builder.move_to((1.0, 1.0))       .line_to((3.0, 1.0)).unwrap()
            .line_to((3.0, 3.0)).unwrap() .line_to((1.0, 3.0)).unwrap();
        assert_eq!(render(&path_edges(builder), 4, 4, FillRule::NonZero),
            [0, 0,   0,   0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 0,   0,   0]);
    }

    #[test] fn fractional_rectangle_uses_exact_horizontal_span_overlap() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.5, 0.0))      .line_to((1.5, 0.0)).unwrap()
            .line_to((1.5, 1.0)).unwrap().line_to((0.5, 1.0)).unwrap();
        assert_eq!(render(&path_edges(builder), 2, 1, FillRule::NonZero), [128, 128]);
    }

    #[test] fn non_zero_and_even_odd_differ_for_nested_same_direction_subpaths() {
        let mut builder = PathBuilder::new();
        for (x0, y0, x1, y1) in [(0.0, 0.0, 4.0, 4.0), (1.0, 1.0, 3.0, 3.0)] {
            builder.move_to((x0, y0))      .line_to((x1, y0)).unwrap()
                .line_to((x1, y1)).unwrap().line_to((x0, y1)).unwrap();
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
