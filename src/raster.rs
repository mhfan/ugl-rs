//! Deterministic allocation-free reference scan conversion.
//!
//! This module prioritizes a transparent contract over production throughput.
//! It uses stratified vertical samples and exact horizontal span overlap.

use core::convert::Infallible;
use crate::{edge::Edge, geometry::Rect};

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum FillRule { NonZero, EvenOdd }

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

/// Coverage adapter that intersects incoming spans with an antialiased rectangle.
pub struct RectClipSink<'a, S> { rect: Rect, sink: &'a mut S }

impl<'a, S> RectClipSink<'a, S> {
    pub fn new(rect: Rect, sink: &'a mut S) -> Self { Self { rect, sink } }
}

impl<S> CoverageSink for RectClipSink<'_, S> where S: CoverageSink {
    type Error = S::Error;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        let overlap = |from: f32, to: f32, pixel: u32| {
            (to.min(pixel as f32 + 1.0) - from.max(pixel as f32)).clamp(0.0, 1.0)
        };
        let vertical = overlap(self.rect.top(), self.rect.bottom(), y);
        if  vertical == 0.0 { return Ok(()); }
        let (start, end) = (x.max(libm::floorf(self.rect. left()).max(0.0) as _),
                    (x + len).min(libm:: ceilf(self.rect.right()).max(0.0) as _));
        if start >= end { return Ok(()); }
        let combined = |x| {
            let clip = overlap(self.rect.left(), self.rect.right(), x) * vertical;
            ((coverage as f32 * clip) + 0.5).clamp(0.0, 255.0) as u8
        };
        let mut cursor = start;
        while   cursor < end {
            let (clipped, run_start) = (combined(cursor), cursor);
                cursor += 1;
            while cursor < end && combined(cursor) == clipped { cursor += 1; }
            if clipped != 0 {
                self.sink.span(run_start, y, cursor - run_start, clipped)?;
            }
        }   Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum CoverageMaskError {
    StrideTooSmall { minimum: u32, actual: u32 },
    BufferTooSmall { minimum: usize, actual: usize },
    DimensionsOverflow,
}

/// Borrowed 8-bit coverage mask with explicit row stride.
#[derive(Clone, Copy, Debug)] pub struct CoverageMask<'a> {
    data: &'a [u8], width: u32, height: u32, stride: u32,
}

/// Mutable storage used to rasterize a coverage mask without allocation.
#[derive(Debug)] pub struct CoverageMaskMut<'a> {
    data: &'a mut [u8], width: u32, height: u32, stride: u32,
}

fn validate_mask_buffer(length: usize, width: u32, height: u32, stride: u32) ->
    Result<(), CoverageMaskError> {
    if stride < width {
        return Err(CoverageMaskError::StrideTooSmall { minimum: width, actual: stride });
    }
    let (height, stride, width) = (
        usize::try_from(height).map_err(|_| CoverageMaskError::DimensionsOverflow)?,
        usize::try_from(stride).map_err(|_| CoverageMaskError::DimensionsOverflow)?,
        usize::try_from(width).map_err(|_| CoverageMaskError::DimensionsOverflow)?,
    );
    let minimum = if height == 0 { 0 } else {
        stride.checked_mul(height - 1).and_then(|offset| offset.checked_add(width))
            .ok_or(CoverageMaskError::DimensionsOverflow)?
    };
    if length < minimum {
        return Err(CoverageMaskError::BufferTooSmall { minimum, actual: length });
    }   Ok(())
}

impl<'a> CoverageMask<'a> {
    pub fn new(data: &'a [u8], width: u32, height: u32, stride: u32) ->
        Result<Self, CoverageMaskError> {
        validate_mask_buffer(data.len(), width, height, stride)?;
        Ok(Self { data, width, height, stride })
    }

    pub fn  width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn stride(&self) -> u32 { self.stride }

    fn coverage(&self, x: u32, y: u32) -> u8 {
        self.data[y as usize * self.stride as usize + x as usize]
    }
}

impl<'a> CoverageMaskMut<'a> {
    pub fn new(data: &'a mut [u8], width: u32, height: u32, stride: u32) ->
        Result<Self, CoverageMaskError> {
        validate_mask_buffer(data.len(), width, height, stride)?;
        Ok(Self { data, width, height, stride })
    }

    pub fn  width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn stride(&self) -> u32 { self.stride }
    pub fn as_mask(&self) -> CoverageMask<'_> { CoverageMask {
        data: self.data, width: self.width, height: self.height, stride: self.stride
    } }

    pub fn clear(&mut self) {
        for y in 0..self.height as usize {
            let start = y * self.stride as usize;
            self.data[start..start + self.width as usize].fill(0);
        }
    }
}

impl CoverageSink for CoverageMaskMut<'_> {
    type Error = Infallible;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        if x >= self.width || y >= self.height { return Ok(()); }
        let len = len.min(self.width - x);
        let start = y as usize * self.stride as usize + x as usize;
        self.data[start..start + len as usize].fill(coverage);
        Ok(())
    }
}

/// Coverage adapter that multiplies incoming spans by a borrowed mask.
pub struct  MaskClipSink<'a, S> { mask: CoverageMask<'a>, sink: &'a mut S }

impl<'a, S> MaskClipSink<'a, S> {
    pub fn new(mask: CoverageMask<'a>, sink: &'a mut S) -> Self { Self { mask, sink } }
}

impl<S> CoverageSink for MaskClipSink<'_, S> where S: CoverageSink {
    type Error = S::Error;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        if y >= self.mask.height { return Ok(()); }
        let (mut cursor, end, mask) = (x, (x + len).min(self.mask.width), self.mask);
        let clipped_coverage = |x|
            (coverage as u16 * mask.coverage(x, y) as u16 + 127).div_euclid(255) as u8;
        while cursor < end {
            let clipped   = clipped_coverage(cursor);
            let start = cursor;
            cursor += 1;
            while cursor < end {
                let next  = clipped_coverage(cursor);
                if  next != clipped { break; }
                cursor += 1;
            }
            if clipped != 0 { self.sink.span(start, y, cursor - start, clipped)?; }
        }   Ok(())
    }
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
        let mut intersections = vec![Intersection::default(); edges.len()];
        let (mut spans, mut row) =  (SpanRecorder::default(), vec![0.0; 5]);
        rasterize_edges(&edges, 5, 1, FillRule::NonZero, RasterOptions::default(),
            &mut RasterWorkspace { intersections: &mut intersections, row_coverage: &mut row },
            &mut spans,
        ).unwrap();
        assert_eq!(spans.0, [(1, 0, 3, 255)]);
    }

    #[test] fn rectangular_clip_multiplies_boundary_coverage_and_coalesces_interior() {
        let mut spans = SpanRecorder::default();
        let rect = Rect::from_ltrb(0.5, 0.25, 3.25, 1.0).unwrap();
        RectClipSink::new(rect, &mut spans).span(0, 0, 5, u8::MAX).unwrap();
        assert_eq!(spans.0, [(0, 0, 1, 96), (1, 0, 2, 191), (3, 0, 1, 48)]);

        spans.0.clear();
        let rect = Rect::from_ltrb(1.0, 0.0, 4.0, 1.0).unwrap();
        RectClipSink::new(rect, &mut spans).span(0, 0, 5, 128).unwrap();
        assert_eq!(spans.0, [(1, 0, 3, 128)]);
    }

    #[test] fn coverage_masks_validate_storage_preserve_padding_and_coalesce() {
        assert_eq!(CoverageMask::new(&[0; 4], 3, 2, 2).unwrap_err(),
            CoverageMaskError::StrideTooSmall { minimum: 3, actual: 2 });
        assert_eq!(CoverageMask::new(&[0; 6], 3, 2, 4).unwrap_err(),
            CoverageMaskError::BufferTooSmall { minimum: 7, actual: 6 });

        let (mut spans, mut data) = (SpanRecorder::default(), vec![9; 8]);
        let mut mask = CoverageMaskMut::new(&mut data, 3, 2, 4).unwrap();
        mask.clear();   mask.span(1, 0, 8, 128).unwrap();
        MaskClipSink::new(mask.as_mask(), &mut spans).span(0, 0, 3, 128).unwrap();
        assert_eq!(spans.0, [(1, 0, 2, 64)]);
        assert_eq!(data, [0, 128, 128, 9, 0, 0, 0, 9]);
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
