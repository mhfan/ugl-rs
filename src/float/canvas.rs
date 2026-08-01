//! Pixel targets and allocation-free `f32` rendering paths.
//!
//! The exact-area rasterizer is the production path exposed by the unqualified
//! `render_*` API. The supersampled reference path is explicitly named
//! `render_*_sampled`.

use core::convert::Infallible;
use crate::{common::{color::{PremulSRGBA8, SRGBA}, dash::{DashContour, DashWorkspace},
    edge::Edge, geometry::{Affine, Path, Point, Rect}, Pixmap, RenderError, SolidPaint,
    raster::{CoverageMask, CoverageMaskMut, CoverageSink, FillRule, MaskClipSink},
    render::{BYTES_PER_PIXEL, EdgeCapacity, EdgeSliceSink, map_dash_error,
        solid_blend_terms, validate_coverage_dimensions},
    stroke::{StrokeContour, StrokePathWorkspace, StrokeWorkspaceError}},
    float::{analytic::{BinError as AnalyticBinError,
        BinWorkspace as AnalyticBinWorkspace, Cell as AnalyticCell,
        CellWorkspace as AnalyticWorkspace, Intersection as AnalyticIntersection,
        bin_requirements, build_row_bins, rasterize_edges_cells,
        rasterize_edges_cells_region}, dash::{dash_polyline, DashPattern},
        flatten::{FlattenError, FlattenOptions, build_fill_edges},
        raster::{Intersection, RasterError,
        RasterOptions, RasterWorkspace, RectClipSink, clip_region, rect_is_integer,
        rasterize_edges}, sampler::PaintSampler,
        stroke::{flatten_stroke_path, stroke_polyline, StrokeExpandError, StrokeOptions}},
};

fn blend_sampled_pixel(pixel: &mut [u8], color: PremulSRGBA8,
    coverage: u8) {
    if coverage == u8::MAX && pixel[3] == 0 {
        pixel.copy_from_slice(&color.to_array()); return;
    }
    let (source, alpha, inverse) = solid_blend_terms(color, coverage);
    if pixel[3] == 0 {
        pixel.copy_from_slice(&[source[0], source[1], source[2], alpha]); return;
    }
    let mul_div_255 = |a, b| (a as u16 * b as u16 + 127).div_euclid(255) as u8;
    for (channel, source) in pixel[..3].iter_mut().zip(source) {
        *channel = source.saturating_add(mul_div_255(*channel, inverse));
    }
    pixel[3] = alpha.saturating_add(mul_div_255(pixel[3], inverse));
}

#[cfg(test)] use crate::common::render::blend_solid_bytes;

impl Pixmap<'_> {
    fn blend_sampled_span<S: PaintSampler>(&mut self, x: u32, y: u32, len: u32,
        sampler: &S, coverage: u8) {
        if let Some(color) = sampler.solid_color() {
            self.blend_solid_span(x, y, len, color, coverage);
            return;
        }
        let start = y as usize * self.stride as usize +
                    x as usize * BYTES_PER_PIXEL as usize;
        let end = start + len as usize * BYTES_PER_PIXEL as usize;
        let bytes = &mut self.as_bytes_mut()[start..end];
        let mut pairs = bytes.chunks_exact_mut(8);
        let mut pending = None;
        sampler.sample_span(x as f32 + 0.5, y as f32 + 0.5, 1.0, 0.0, len, |color| {
            let Some(first) = pending.take() else { pending = Some(color); return; };
            let pair = pairs.next().expect("sampler emitted too many span pixels");
            if coverage == u8::MAX && pair == [0; 8] {
                let (first, second) = (
                    u32::from_le_bytes(first.to_array()) as u64,
                    u32::from_le_bytes(color.to_array()) as u64,
                );
                pair.copy_from_slice(&(first | second << 32).to_le_bytes());
            } else {
                blend_sampled_pixel(&mut pair[..4], first, coverage);
                blend_sampled_pixel(&mut pair[4..], color, coverage);
            }
        });
        let remainder = pairs.into_remainder();
        if let Some(color) = pending {
            blend_sampled_pixel(remainder, color, coverage);
        } else { debug_assert!(remainder.is_empty()); }
    }

}

pub struct SampledRenderWorkspace<'a> {
    pub edges: &'a mut [Edge],
    pub intersections: &'a mut [Intersection],
    pub  row_coverage: &'a mut [f32],
}

pub struct RenderWorkspace<'a> {
    pub intersections: &'a mut [AnalyticIntersection],
    pub cells: &'a mut [AnalyticCell],
    pub edges: &'a mut [Edge],
    pub row_offsets: &'a mut [u32],
    pub edge_indices: &'a mut [u32],
}

/// Caller-owned storage for the complete analytic stroke pipeline.
pub struct StrokeWorkspace<'a> {
    pub points: &'a mut [Point],
    pub  edges: &'a mut [Edge],
    pub contours: &'a mut [StrokeContour],
    pub intersections: &'a mut [AnalyticIntersection],
    pub cells: &'a mut [AnalyticCell],
    pub row_offsets: &'a mut [u32],
    pub edge_indices: &'a mut [u32],
}

/// Caller-owned storage for flattened, dashed analytic strokes.
pub struct DashedStrokeWorkspace<'a> {
    pub stroke: StrokeWorkspace<'a>,
    pub dash_points: &'a mut [Point],
    pub dash_contours: &'a mut [DashContour],
}

pub struct StrokePlanningWorkspace<'a> {
    pub points: &'a mut [Point],
    pub contours: &'a mut [StrokeContour],
    pub edges: &'a mut [Edge],
}

pub struct DashedStrokePlanningWorkspace<'a> {
    pub stroke: StrokePlanningWorkspace<'a>,
    pub dash_points: &'a mut [Point],
    pub dash_contours: &'a mut [DashContour],
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderRequirements {
    pub edges: usize,
    pub intersections: usize,
    pub cells: usize,
    pub row_offsets: usize,
    pub edge_indices: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StrokeRequirements {
    pub render: RenderRequirements,
    pub points: usize,
    pub contours: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DashedStrokeRequirements {
    pub stroke: StrokeRequirements,
    pub dash_points: usize,
    pub dash_contours: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)] pub struct SampledRenderOptions {
    pub fill_rule: FillRule,
    pub flatten: FlattenOptions,
    pub  raster:  RasterOptions,
}

impl Default for SampledRenderOptions { fn default() -> Self {
        Self { fill_rule: FillRule::NonZero,
            flatten: FlattenOptions::default(),
             raster:  RasterOptions::default(),
        }
} }

#[derive(Clone, Copy, Debug, PartialEq)] pub struct RenderOptions {
    pub fill_rule: FillRule, pub flatten: FlattenOptions,
}

impl Default for RenderOptions { fn default() -> Self {
    Self { fill_rule: FillRule::NonZero, flatten: FlattenOptions::default() }
} }

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StrokePathOptions {
    pub flatten: FlattenOptions,
    pub stroke: StrokeOptions,
}

#[derive(Clone, Copy, Debug)] pub struct DashedStrokePathOptions<'a> {
    pub flatten: FlattenOptions,
    pub stroke: StrokeOptions,
    pub dash: DashPattern<'a>,
}

/// Computes exact fill capacities without touching a render target.
///
/// `edges` is planning scratch. If it is too small, the returned
/// [`RenderError::EdgeCapacity`] gives the next required lower bound; retrying
/// with sufficient edge storage returns the complete exact requirements.
///
/// ```
/// use ugl_rs::{common::{edge::Edge, geometry::{Affine, PathBuilder}},
///     float::canvas::{RenderOptions, render_requirements}};
///
/// let mut path = PathBuilder::new();
/// path.move_to((0.5, 0.5)).line_to((3.5, 0.5))
///     .line_to((3.5, 3.5)).line_to((0.5, 3.5));
/// let mut edges = [Edge::default(); 8];
/// let required = render_requirements(&path.build(), Affine::identity(),
///     RenderOptions::default(), 4, 4, &mut edges).unwrap();
/// assert_eq!((required.edges, required.cells), (2, 4));
/// ```
pub fn render_requirements(path: &Path, transform: Affine, options: RenderOptions,
    width: u32, height: u32, edges: &mut [Edge]) ->
    Result<RenderRequirements, RenderError> {
    let edge_count = build_edges(path, transform, options.flatten, edges)?;
    requirements_from_edges(&edges[..edge_count], width, height)
}

/// Computes the exact capacities for [`rasterize_path_clip`].
pub fn path_clip_requirements(path: &Path, transform: Affine, options: RenderOptions,
    dimensions: (u32, u32), edges: &mut [Edge]) ->
    Result<RenderRequirements, RenderError> {
    render_requirements(
        path, transform, options, dimensions.0, dimensions.1, edges)
}

/// Computes exact undashed stroke capacities using caller-owned planning scratch.
pub fn stroke_requirements(path: &Path, transform: Affine, options: StrokePathOptions,
    dimensions: (u32, u32), workspace: &mut StrokePlanningWorkspace<'_>) ->
    Result<StrokeRequirements, RenderError> {
    let usage = build_stroke_edges(path, transform, options,
        workspace.points, workspace.contours, workspace.edges)?;
    Ok(StrokeRequirements {
        render: requirements_from_edges(
            &workspace.edges[..usage.edges], dimensions.0, dimensions.1)?,
        points: usage.points, contours: usage.contours,
    })
}

/// Computes exact dashed stroke capacities using caller-owned planning scratch.
pub fn dashed_stroke_requirements(path: &Path, transform: Affine,
    options: DashedStrokePathOptions<'_>, dimensions: (u32, u32),
    workspace: &mut DashedStrokePlanningWorkspace<'_>) ->
    Result<DashedStrokeRequirements, RenderError> {
    let usage = build_dashed_stroke_edges(path, transform, options,
        &mut StrokePathWorkspace {
            points: workspace.stroke.points, contours: workspace.stroke.contours,
        }, &mut DashWorkspace {
            points: workspace.dash_points, contours: workspace.dash_contours,
        }, workspace.stroke.edges)?;
    Ok(DashedStrokeRequirements {
        stroke: StrokeRequirements {
            render: requirements_from_edges(
                &workspace.stroke.edges[..usage.edges], dimensions.0, dimensions.1)?,
            points: usage.points, contours: usage.contours,
        },
        dash_points: usage.dash_points, dash_contours: usage.dash_contours,
    })
}

/// Renders a solid straight-alpha color through the reference rasterizer.
///
/// The destination is premultiplied RGBA8888. This function performs no
/// allocation; all geometry and raster storage comes from `workspace`.
pub fn render_solid_sampled(path: &Path, transform: Affine, color: SRGBA<u8>, options: SampledRenderOptions,
    target: &mut Pixmap<'_>, workspace: &mut SampledRenderWorkspace<'_>) ->
    Result<(), RenderError> {
    let edge_count = build_edges(path, transform, options.flatten, workspace.edges)?;
    let paint = SolidPaint::new(color);
    let mut compositor = PaintCompositor { target, sampler: &paint };
    rasterize_edges(&workspace.edges[..edge_count], compositor.target.width,
        compositor.target.height, options.fill_rule, options.raster, &mut RasterWorkspace {
            intersections: workspace.intersections,
             row_coverage: workspace.row_coverage,
        }, &mut compositor,
    ).map_err(map_raster_error)
}

/// Renders through the sampled reference rasterizer and an antialiased rectangle clip.
pub fn render_solid_sampled_clipped(path: &Path, transform: Affine, color: SRGBA<u8>,
    clip: Rect, options: SampledRenderOptions, target: &mut Pixmap<'_>,
    workspace: &mut SampledRenderWorkspace<'_>) -> Result<(), RenderError> {
    let edge_count = build_edges(path, transform, options.flatten, workspace.edges)?;
    let paint = SolidPaint::new(color);
    let mut compositor = PaintCompositor { target, sampler: &paint };
    rasterize_edges(&workspace.edges[..edge_count], compositor.target.width,
        compositor.target.height, options.fill_rule, options.raster, &mut RasterWorkspace {
            intersections: workspace.intersections,
             row_coverage: workspace.row_coverage,
        }, &mut RectClipSink::new(clip, &mut compositor),
    ).map_err(map_raster_error)
}

/// Renders a solid color through the exact-area `f32` rasterizer.
pub fn render_solid(path: &Path, transform: Affine, color: SRGBA<u8>,
    options: RenderOptions, target: &mut Pixmap<'_>,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError> {
    render_paint(path, transform, &SolidPaint::new(color), options, target, workspace)
}

/// Renders a statically dispatched paint sampler through the exact-area `f32` rasterizer.
///
/// Samples are evaluated at device-space pixel centers. Coverage and sampled
/// premultiplied colors are then composed source-over the target.
pub fn render_paint<S: PaintSampler>(path: &Path, transform: Affine, sampler: &S,
    options: RenderOptions, target: &mut Pixmap<'_>,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_path_to(path, transform, options, width, height,
        &mut compositor, workspace)
}

/// Renders a solid analytic stroke without allocating intermediate geometry.
pub fn render_stroke_solid(path: &Path, transform: Affine, color: SRGBA<u8>,
    options: StrokePathOptions, target: &mut Pixmap<'_>,
    workspace: &mut StrokeWorkspace<'_>) -> Result<(), RenderError> {
    render_stroke_paint(
        path, transform, &SolidPaint::new(color), options, target, workspace)
}

/// Renders an analytic stroke through the shared paint compositor.
pub fn render_stroke_paint<S: PaintSampler>(path: &Path, transform: Affine,
    sampler: &S, options: StrokePathOptions, target: &mut Pixmap<'_>,
    workspace: &mut StrokeWorkspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_stroke_to(path, transform, options, width, height,
        &mut compositor, workspace)
}

/// Renders a dashed analytic stroke without allocating intermediate geometry.
pub fn render_stroke_solid_dashed(path: &Path, transform: Affine,
    color: SRGBA<u8>, options: DashedStrokePathOptions<'_>,
    target: &mut Pixmap<'_>, workspace: &mut DashedStrokeWorkspace<'_>) ->
    Result<(), RenderError> {
    render_stroke_paint_dashed(path, transform, &SolidPaint::new(color),
        options, target, workspace)
}

pub fn render_stroke_paint_dashed<S: PaintSampler>(path: &Path,
    transform: Affine, sampler: &S, options: DashedStrokePathOptions<'_>,
    target: &mut Pixmap<'_>, workspace: &mut DashedStrokeWorkspace<'_>) ->
    Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_stroke_dashed_to(path, transform, options, width, height,
        &mut compositor, workspace)
}

pub fn render_stroke_paint_dashed_clipped<S: PaintSampler>(path: &Path,
    transform: Affine, sampler: &S, clip: Rect, options: DashedStrokePathOptions<'_>,
    target: &mut Pixmap<'_>, workspace: &mut DashedStrokeWorkspace<'_>) ->
    Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    let region = clip_region(clip, width, height);
    if rect_is_integer(clip) {
        render_stroke_dashed_to_region(path, transform, options, (width, height),
            region, &mut compositor, workspace)
    } else {
        render_stroke_dashed_to_region(path, transform, options, (width, height),
            region, &mut RectClipSink::new(clip, &mut compositor), workspace)
    }
}

pub fn render_stroke_paint_dashed_masked<S: PaintSampler>(path: &Path,
    transform: Affine, sampler: &S, mask: CoverageMask<'_>,
    options: DashedStrokePathOptions<'_>, target: &mut Pixmap<'_>,
    workspace: &mut DashedStrokeWorkspace<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_stroke_dashed_to_region(path, transform, options, (width, height),
        mask.non_zero_bounds().unwrap_or_default(),
        &mut MaskClipSink::new(mask, &mut compositor), workspace)
}

/// Renders a solid analytic stroke through an antialiased rectangle clip.
pub fn render_stroke_solid_clipped(path: &Path, transform: Affine, color: SRGBA<u8>,
    clip: Rect, options: StrokePathOptions, target: &mut Pixmap<'_>,
    workspace: &mut StrokeWorkspace<'_>) -> Result<(), RenderError> {
    render_stroke_paint_clipped(path, transform,
        &SolidPaint::new(color), clip, options, target, workspace)
}

/// Renders analytic stroke paint through an antialiased rectangle clip.
pub fn render_stroke_paint_clipped<S: PaintSampler>(path: &Path, transform: Affine,
    sampler: &S, clip: Rect, options: StrokePathOptions, target: &mut Pixmap<'_>,
    workspace: &mut StrokeWorkspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    let region = clip_region(clip, width, height);
    if rect_is_integer(clip) {
        render_stroke_to_region(path, transform, options, (width, height),
            region, &mut compositor, workspace)
    } else {
        render_stroke_to_region(path, transform, options, (width, height), region,
            &mut RectClipSink::new(clip, &mut compositor), workspace)
    }
}

/// Renders a solid analytic stroke multiplied by a borrowed path clip mask.
pub fn render_stroke_solid_masked(path: &Path, transform: Affine, color: SRGBA<u8>,
    mask: CoverageMask<'_>, options: StrokePathOptions, target: &mut Pixmap<'_>,
    workspace: &mut StrokeWorkspace<'_>) -> Result<(), RenderError> {
    render_stroke_paint_masked(
        path, transform, &SolidPaint::new(color), mask, options, target, workspace)
}

/// Renders analytic stroke paint multiplied by a borrowed path clip mask.
pub fn render_stroke_paint_masked<S: PaintSampler>(path: &Path, transform: Affine,
    sampler: &S, mask: CoverageMask<'_>, options: StrokePathOptions,
    target: &mut Pixmap<'_>, workspace: &mut StrokeWorkspace<'_>) ->
    Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_stroke_to_region(path, transform, options, (width, height),
        mask.non_zero_bounds().unwrap_or_default(),
        &mut MaskClipSink::new(mask, &mut compositor), workspace)
}

/// Renders through the exact-area rasterizer and an antialiased rectangle clip.
pub fn render_solid_clipped(path: &Path, transform: Affine, color: SRGBA<u8>,
    clip: Rect, options: RenderOptions, target: &mut Pixmap<'_>,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError> {
    render_paint_clipped(path, transform,
        &SolidPaint::new(color), clip, options, target, workspace)
}

/// Renders an analytic paint through an antialiased rectangle clip.
pub fn render_paint_clipped<S: PaintSampler>(path: &Path, transform: Affine,
    sampler: &S, clip: Rect, options: RenderOptions, target: &mut Pixmap<'_>,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    let region = clip_region(clip, width, height);
    if rect_is_integer(clip) {
        render_path_to_region(path, transform, options, (width, height), region,
            &mut compositor, workspace)
    } else {
        render_path_to_region(path, transform, options, (width, height), region,
            &mut RectClipSink::new(clip, &mut compositor), workspace)
    }
}

/// Rasterizes an analytic path clip into caller-owned 8-bit coverage.
///
/// The valid mask area is cleared after flattening succeeds. Callers must
/// discard the mask if this function returns an error.
pub fn rasterize_path_clip(path: &Path, transform: Affine,
    options: RenderOptions, mask: &mut CoverageMaskMut<'_>,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError> {
    let edge_count = build_edges(path, transform, options.flatten, workspace.edges)?;
    mask.clear();
    rasterize(&workspace.edges[..edge_count], mask.width(), mask.height(),
        options.fill_rule, AnalyticWorkspace {
            intersections: workspace.intersections, cells: workspace.cells,
        }, AnalyticBinWorkspace {
            row_offsets: workspace.row_offsets, edge_indices: workspace.edge_indices,
        }, mask)
}

/// Renders analytic solid coverage multiplied by a borrowed path clip mask.
pub fn render_solid_masked(path: &Path, transform: Affine, color: SRGBA<u8>,
    mask: CoverageMask<'_>, options: RenderOptions, target: &mut Pixmap<'_>,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError> {
    render_paint_masked(path, transform, &SolidPaint::new(color), mask,
        options, target, workspace)
}

/// Renders analytic paint coverage multiplied by a borrowed path clip mask.
pub fn render_paint_masked<S: PaintSampler>(path: &Path, transform: Affine,
    sampler: &S, mask: CoverageMask<'_>, options: RenderOptions,
    target: &mut Pixmap<'_>, workspace: &mut RenderWorkspace<'_>) ->
    Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_path_to_region(path, transform, options, (width, height),
        mask.non_zero_bounds().unwrap_or_default(),
        &mut MaskClipSink::new(mask, &mut compositor), workspace)
}


pub(crate) fn build_edges(path: &Path, transform: Affine, options: FlattenOptions,
    edges: &mut [Edge]) ->
    Result<usize, RenderError> {
    let mut sink = EdgeSliceSink { edges, len: 0 };
    build_fill_edges(path, transform, options, &mut sink).map_err(map_flatten_error)?;
    Ok(sink.len)
}

pub(crate) fn edge_region(edges: &[Edge], width: u32, height: u32) ->
    (u32, u32, u32, u32) {
    let Some(first) = edges.first() else { return (0, 0, 0, 0); };
    let (mut left, mut top, mut right, mut bottom) =
        (first.upper.x, first.upper.y, first.upper.x, first.lower.y);
    for edge in edges {
        left = left.min(edge.upper.x).min(edge.lower.x);
        top = top.min(edge.upper.y);
        right = right.max(edge.upper.x).max(edge.lower.x);
        bottom = bottom.max(edge.lower.y);
    }
    if left >= right || top >= bottom { return (0, 0, 0, 0); }
    let rect = Rect::from_ltrb(left, top, right, bottom)
        .expect("valid edges have ordered finite bounds");
    clip_region(rect, width, height)
}

pub(crate) struct StrokeUsage { points: usize, contours: usize, edges: usize }

pub(crate) fn build_stroke_edges(path: &Path, transform: Affine,
    options: StrokePathOptions,
    points: &mut [Point], contours: &mut [StrokeContour], edges: &mut [Edge]) ->
    Result<StrokeUsage, RenderError> {
    let mut path_workspace = StrokePathWorkspace { points, contours };
    let flattened = flatten_stroke_path(path, transform, options.flatten,
        &mut path_workspace).map_err(map_stroke_flatten_error)?;
    let (point_count, contour_count) =
        (flattened.point_count(), flattened.contour_count());
    let mut sink = EdgeSliceSink { edges, len: 0 };
    for (points, closed) in flattened.contours() {
        stroke_polyline(points, closed, options.stroke, &mut sink)
            .map_err(map_stroke_expand_error)?;
    }
    Ok(StrokeUsage {
        points: point_count, contours: contour_count, edges: sink.len,
    })
}

struct DashedStrokeUsage {
    points: usize, contours: usize, edges: usize,
    dash_points: usize, dash_contours: usize,
}

fn build_dashed_stroke_edges(path: &Path, transform: Affine,
    options: DashedStrokePathOptions<'_>,
    path_workspace: &mut StrokePathWorkspace<'_>,
    dash_workspace: &mut DashWorkspace<'_>, edges: &mut [Edge]) ->
    Result<DashedStrokeUsage, RenderError> {
    let flattened = flatten_stroke_path(path, transform, options.flatten,
        path_workspace).map_err(map_stroke_flatten_error)?;
    let (point_count, contour_count) =
        (flattened.point_count(), flattened.contour_count());
    let (mut dash_points, mut dash_contours) = (0, 0);
    let mut sink = EdgeSliceSink { edges, len: 0 };
    for (points, closed) in flattened.contours() {
        let dashed = dash_polyline(points, closed, options.dash, dash_workspace)
            .map_err(map_dash_error)?;
        dash_points = dash_points.max(dashed.point_count());
        dash_contours = dash_contours.max(dashed.contour_count());
        for (points, closed) in dashed.contours() {
            stroke_polyline(points, closed, options.stroke, &mut sink)
                .map_err(map_stroke_expand_error)?;
        }
    }
    Ok(DashedStrokeUsage {
        points: point_count, contours: contour_count, edges: sink.len,
        dash_points, dash_contours,
    })
}

fn requirements_from_edges(edges: &[Edge], width: u32, height: u32) ->
    Result<RenderRequirements, RenderError> {
    let cells = usize::try_from(width).map_err(|_| RenderError::DimensionsOverflow)?;
    let bins = bin_requirements(edges, height).map_err(map_bin_error)?;
    Ok(RenderRequirements {
        edges: edges.len(), intersections: edges.len(), cells,
        row_offsets: bins.offsets, edge_indices: bins.indices,
    })
}

pub(crate) fn rasterize<S>(edges: &[Edge], width: u32, height: u32,
    fill_rule: FillRule,
    mut workspace: AnalyticWorkspace<'_>, bin_workspace: AnalyticBinWorkspace<'_>,
    sink: &mut S) ->
    Result<(), RenderError> where S: CoverageSink<Error = Infallible> {
    let bins = build_row_bins(edges, height, bin_workspace)
        .map_err(map_bin_error)?;
    rasterize_edges_cells(edges, bins, width, height, fill_rule,
        &mut workspace, sink).map_err(map_raster_error)
}

fn rasterize_region<S>(edges: &[Edge], dimensions: (u32, u32),
    region: (u32, u32, u32, u32), fill_rule: FillRule,
    mut workspace: AnalyticWorkspace<'_>, bin_workspace: AnalyticBinWorkspace<'_>,
    sink: &mut S) -> Result<(), RenderError>
    where S: CoverageSink<Error = Infallible> {
    let (width, height) = dimensions;
    let bins = build_row_bins(edges, height, bin_workspace).map_err(map_bin_error)?;
    rasterize_edges_cells_region(edges, bins, (width, height), fill_rule, region,
        &mut workspace, sink).map_err(map_raster_error)
}

pub(crate) fn rasterize_built_region<S>(edge_count: usize, dimensions: (u32, u32),
    region: (u32, u32, u32, u32), fill_rule: FillRule, sink: &mut S,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError>
    where S: CoverageSink<Error = Infallible> {
    rasterize_region(&workspace.edges[..edge_count], dimensions, region, fill_rule,
        AnalyticWorkspace {
            intersections: workspace.intersections, cells: workspace.cells,
        }, AnalyticBinWorkspace {
            row_offsets: workspace.row_offsets, edge_indices: workspace.edge_indices,
        }, sink)
}

pub(crate) fn render_path_to<S>(path: &Path, transform: Affine,
    options: RenderOptions, width: u32, height: u32, sink: &mut S,
    workspace: &mut RenderWorkspace<'_>) ->
    Result<(), RenderError> where S: CoverageSink<Error = Infallible> {
    render_path_to_region(path, transform, options, (width, height),
        (0, 0, width, height), sink, workspace)
}

fn render_path_to_region<S>(path: &Path, transform: Affine, options: RenderOptions,
    dimensions: (u32, u32), region: (u32, u32, u32, u32), sink: &mut S,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError>
    where S: CoverageSink<Error = Infallible> {
    let edge_count = build_edges(path, transform, options.flatten, workspace.edges)?;
    rasterize_region(&workspace.edges[..edge_count], dimensions, region,
        options.fill_rule, AnalyticWorkspace {
            intersections: workspace.intersections, cells: workspace.cells,
        }, AnalyticBinWorkspace {
            row_offsets: workspace.row_offsets, edge_indices: workspace.edge_indices,
        }, sink)
}

pub(crate) fn render_stroke_to<S>(path: &Path, transform: Affine,
    options: StrokePathOptions, width: u32, height: u32, sink: &mut S,
    workspace: &mut StrokeWorkspace<'_>) ->
    Result<(), RenderError> where S: CoverageSink<Error = Infallible> {
    render_stroke_to_region(path, transform, options, (width, height),
        (0, 0, width, height), sink, workspace)
}

fn render_stroke_to_region<S>(path: &Path, transform: Affine,
    options: StrokePathOptions, dimensions: (u32, u32),
    region: (u32, u32, u32, u32), sink: &mut S,
    workspace: &mut StrokeWorkspace<'_>) -> Result<(), RenderError>
    where S: CoverageSink<Error = Infallible> {
    let StrokeWorkspace {
        points, contours, edges, intersections, cells, row_offsets, edge_indices,
    } = workspace;
    let usage = build_stroke_edges(path, transform, options, points, contours, edges)?;
    rasterize_region(&edges[..usage.edges], dimensions, region, FillRule::NonZero,
        AnalyticWorkspace { intersections, cells },
        AnalyticBinWorkspace { row_offsets, edge_indices }, sink)
}

pub(crate) fn render_stroke_dashed_to<S>(path: &Path, transform: Affine,
    options: DashedStrokePathOptions<'_>, width: u32, height: u32, sink: &mut S,
    workspace: &mut DashedStrokeWorkspace<'_>) ->
    Result<(), RenderError> where S: CoverageSink<Error = Infallible> {
    render_stroke_dashed_to_region(path, transform, options, (width, height),
        (0, 0, width, height), sink, workspace)
}

fn render_stroke_dashed_to_region<S>(path: &Path, transform: Affine,
    options: DashedStrokePathOptions<'_>, dimensions: (u32, u32),
    region: (u32, u32, u32, u32), sink: &mut S,
    workspace: &mut DashedStrokeWorkspace<'_>) -> Result<(), RenderError>
    where S: CoverageSink<Error = Infallible> {
    let DashedStrokeWorkspace {
        stroke: StrokeWorkspace {
            points, contours, edges, intersections, cells, row_offsets, edge_indices,
        }, dash_points, dash_contours,
    } = workspace;
    let mut path_workspace = StrokePathWorkspace { points, contours };
    let mut dash_workspace = DashWorkspace {
        points: dash_points, contours: dash_contours,
    };
    let usage = build_dashed_stroke_edges(path, transform, options,
        &mut path_workspace, &mut dash_workspace, edges)?;
    rasterize_region(&edges[..usage.edges], dimensions, region, FillRule::NonZero,
        AnalyticWorkspace { intersections, cells },
        AnalyticBinWorkspace { row_offsets, edge_indices }, sink)
}

fn map_bin_error(error: AnalyticBinError) -> RenderError {
    match error {
        AnalyticBinError::DimensionsOverflow => RenderError::DimensionsOverflow,
        AnalyticBinError::OffsetCapacity { required } =>
            RenderError::AnalyticBinOffsetCapacity { required },
        AnalyticBinError::IndexCapacity { required } =>
            RenderError::AnalyticBinIndexCapacity { required },
    }
}

pub(crate) struct PaintCompositor<'a, 'b, S> {
    pub(crate) target: &'a mut Pixmap<'b>, pub(crate) sampler: &'a S,
}

impl<S: PaintSampler> CoverageSink for PaintCompositor<'_, '_, S> {
    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        self.target.blend_sampled_span(x, y, len, self.sampler, coverage);  Ok(())
    }   type Error = Infallible;
}

fn map_flatten_error(error: FlattenError<EdgeCapacity>) -> RenderError {
    match error {
        FlattenError::InvalidTolerance => RenderError::InvalidTolerance,
        FlattenError::InvalidDepth => RenderError::InvalidDepth,
        FlattenError::NonFiniteCoordinate => RenderError::NonFiniteCoordinate,
        FlattenError::DepthLimit => RenderError::FlattenDepthLimit,
        FlattenError::InvalidPath(error) => RenderError::InvalidPath(error),
        FlattenError::Sink(error) =>
            RenderError::EdgeCapacity { needed_at_least: error.needed_at_least },
    }
}

fn map_stroke_flatten_error(error: FlattenError<StrokeWorkspaceError>) -> RenderError {
    match error {
        FlattenError::InvalidTolerance => RenderError::InvalidTolerance,
        FlattenError::InvalidDepth => RenderError::InvalidDepth,
        FlattenError::NonFiniteCoordinate => RenderError::NonFiniteCoordinate,
        FlattenError::DepthLimit => RenderError::FlattenDepthLimit,
        FlattenError::InvalidPath(error) => RenderError::InvalidPath(error),
        FlattenError::Sink(StrokeWorkspaceError::PointCapacity { needed_at_least }) =>
            RenderError::StrokePointCapacity { needed_at_least },
        FlattenError::Sink(StrokeWorkspaceError::ContourCapacity { needed_at_least }) =>
            RenderError::StrokeContourCapacity { needed_at_least },
        FlattenError::Sink(StrokeWorkspaceError::IndexOverflow) =>
            RenderError::StrokeIndexOverflow,
    }
}

fn map_stroke_expand_error(error: StrokeExpandError<EdgeCapacity>) -> RenderError {
    match error {
        StrokeExpandError::NonFinitePoint => RenderError::NonFiniteCoordinate,
        StrokeExpandError::ArcSegmentLimit { needed, maximum } =>
            RenderError::StrokeArcSegmentLimit { needed, maximum },
        StrokeExpandError::Sink(error) =>
            RenderError::EdgeCapacity { needed_at_least: error.needed_at_least },
    }
}

fn map_raster_error(error: RasterError<Infallible>) -> RenderError {
    match error {
        RasterError::DimensionsOverflow => RenderError::DimensionsOverflow,
        RasterError::InvalidEdge => RenderError::InvalidEdge,
        RasterError::InvalidEdgeBins => RenderError::InvalidEdgeBins,
        RasterError::InvalidSampleCount => RenderError::InvalidSampleCount,
        RasterError::WorkspaceTooSmall { intersections, row_coverage } =>
            RenderError::RasterWorkspaceTooSmall { intersections, cells: row_coverage },
        RasterError::Sink(error) => match error {},
    }
}

#[cfg(test)] #[path = "tests.rs"] mod tests;
