//! Workspace and option types for fixed-point rendering.

use core::convert::Infallible;
use crate::{
    common::{color::{PremulSRGBA8, SRGBA}, dash::{DashContour, DashWorkspace},
        geometry::{Affine, Edge, Path, Point, Rect},
        raster::{ClipMask, CoverageMask, CoverageMaskMut, CoverageSink, FillRule,
            MaskClipSink},
        render::{BYTES_PER_PIXEL, EdgeCapacity, EdgeSliceSink, Pixmap, RenderError,
        blend_sampled_pixel, blend_sampled_quad, map_dash_error,
        validate_coverage_dimensions},
        stroke::{StrokePathWorkspace, StrokeWorkspaceError}, SolidPaint},
    fixed::{COORD_SCALE, DEVICE_RAW_LIMIT, Scalar,
        dash::{Pattern as DashPattern, dash_polyline},
        flatten::{Error as FlattenError, Options as FlattenOptions, build_fill_edges},
        math::round_div_u32,
        raster::{CoverageStrips, Error as RasterError, Line, STRIP_HEIGHT,
            RenderError as RasterRenderError, Workspace, prepare_lines, rasterize_lines,
            rasterize_lines_region, strip_requirements},
        sampler::PaintSampler,
        stroke::{ExpandError as StrokeExpandError, Options as StrokeOptions,
            flatten_path as flatten_stroke_path, stroke_polyline},
        tile::{CoverageTiles, DirectTileWorkspace, TileKind, rasterize_lines_to_tiles}},
};
fn blend_sampled_span<S: PaintSampler>(target: &mut Pixmap<'_>,
    x: u32, y: u32, len: u32, sampler: &S, coverage: u8) {
    if let Some(color) = sampler.solid_color() {
        target.blend_solid_span(x, y, len, color, coverage);
        return;
    }
    let start = y as usize * target.stride() as usize +
                x as usize * BYTES_PER_PIXEL as usize;
    let end = start + len as usize * BYTES_PER_PIXEL as usize;
    let bytes = &mut target.as_bytes_mut()[start..end];
    let mut quads = bytes.chunks_exact_mut(16);
    let mut pending = [PremulSRGBA8::zeroed(); 4];
    let mut pending_len = 0;
    sampler.sample_span(x, y, len, |color| {
        pending[pending_len] = color;
        pending_len += 1;
        if pending_len == 4 {
            blend_sampled_quad(quads.next().expect("sampler emitted too many span pixels"),
                pending, coverage);
            pending_len = 0;
        }
    });
    let remainder = quads.into_remainder();
    debug_assert_eq!(remainder.len(), pending_len * 4);
    for (pixel, color) in remainder.chunks_exact_mut(4).zip(pending[..pending_len].iter()) {
        blend_sampled_pixel(pixel, *color, coverage);
    }
}

const SUBPIXEL_SCALE: i64 = COORD_SCALE as _;

fn fixed_rect_is_integer(rect: Rect<Scalar>) -> bool {
    [rect.left(), rect.top(), rect.right(), rect.bottom()]
        .iter().all(|value| i64::from(value.to_bits()).rem_euclid(SUBPIXEL_SCALE) == 0)
}

fn fixed_clip_region(rect: Rect<Scalar>, width: u32, height: u32) ->
    (u32, u32, u32, u32) {
    let bound = |raw: i32, maximum: u32, upper: bool| {
        let raw = i64::from(raw);
        let pixel = if upper {
            (raw + SUBPIXEL_SCALE - 1).div_euclid(SUBPIXEL_SCALE)
        } else { raw.div_euclid(SUBPIXEL_SCALE) };
        pixel.clamp(0, i64::from(maximum)) as _
    };
    (bound(rect.left().to_bits(), width, false),
     bound(rect.top().to_bits(), height, false),
     bound(rect.right().to_bits(), width, true),
     bound(rect.bottom().to_bits(), height, true))
}

struct FixedRectClipSink<'a, S> { rect: Rect<Scalar>, sink: &'a mut S }

struct SparseClipSink<'a, 'mask, S> {
    mask: CoverageStrips<'mask>, sink: &'a mut S,
    strip_index: usize, row: Option<u32>, run_start: usize, run_end: usize,
}

impl<'a, 'mask, S> SparseClipSink<'a, 'mask, S> {
    fn new(mask: CoverageStrips<'mask>, sink: &'a mut S) -> Self {
        Self { mask, sink, strip_index: 0, row: None, run_start: 0, run_end: 0 }
    }

    fn select_row(&mut self, y: u32) {
        if self.row == Some(y) { return; }
        let strip_y = y / STRIP_HEIGHT * STRIP_HEIGHT;
        let strips = self.mask.strips();
        if self.row.is_none_or(|previous| y < previous) {
            self.strip_index = strips.partition_point(|strip| strip.y < strip_y);
        } else {
            while self.strip_index < strips.len() && strips[self.strip_index].y < strip_y {
                self.strip_index += 1;
            }
        }
        let Some(strip) = strips.get(self.strip_index).filter(|strip| strip.y == strip_y)
            else {
                (self.row, self.run_start, self.run_end) = (Some(y), 0, 0); return;
            };
        let start = strip.run_start as usize;
        let runs = &self.mask.runs()[start..start + strip.run_count as usize];
        let local_y = (y - strip_y) as u8;
        let first = runs.partition_point(|run| run.row < local_y);
        let count = runs[first..].partition_point(|run| run.row == local_y);
        (self.row, self.run_start, self.run_end) =
            (Some(y), start + first, start + first + count);
    }
}

impl<S: CoverageSink> CoverageSink for SparseClipSink<'_, '_, S> {
    type Error = S::Error;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        self.select_row(y);
        let incoming_end = x.saturating_add(len);
        let runs = self.mask.runs();
        while self.run_start < self.run_end &&
            runs[self.run_start].x + runs[self.run_start].len <= x {
            self.run_start += 1;
        }
        for run in &runs[self.run_start..self.run_end] {
            if run.x >= incoming_end { break; }
            let (left, right) = (x.max(run.x), incoming_end.min(run.x + run.len));
            if left >= right { continue; }
            let coverage = (u16::from(coverage) * u16::from(run.coverage) + 127)
                .div_euclid(255) as u8;
            if coverage != 0 { self.sink.span(left, y, right - left, coverage)?; }
        }
        Ok(())
    }
}

impl<'a, S> FixedRectClipSink<'a, S> {
    fn new(rect: Rect<Scalar>, sink: &'a mut S) -> Self { Self { rect, sink } }
}

impl<S: CoverageSink> CoverageSink for FixedRectClipSink<'_, S> {
    type Error = S::Error;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        let overlap = |from: Scalar, to: Scalar, pixel: u32| {
            let pixel = i64::from(pixel) * SUBPIXEL_SCALE;
            (i64::from(to.to_bits()).min(pixel + SUBPIXEL_SCALE) -
             i64::from(from.to_bits()).max(pixel)).clamp(0, SUBPIXEL_SCALE) as u32
        };
        let vertical = overlap(self.rect.top(), self.rect.bottom(), y);
        if vertical == 0 { return Ok(()); }
        let (left, _, right, _) = fixed_clip_region(self.rect, u32::MAX, u32::MAX);
        let (start, end) = (x.max(left), x.saturating_add(len).min(right));
        let quantize = |pixel| {
            let horizontal = overlap(self.rect.left(), self.rect.right(), pixel);
            round_div_u32(u32::from(coverage) * horizontal * vertical, 65_536) as _
        };
        let mut cursor = start;
        while cursor < end {
            let (clipped, run_start) = (quantize(cursor), cursor);
            cursor += 1;
            while cursor < end && quantize(cursor) == clipped { cursor += 1; }
            if clipped != 0 {
                self.sink.span(run_start, y, cursor - run_start, clipped)?;
            }
        }
        Ok(())
    }
}

pub struct GeometryWorkspace<'a> {
    pub edges: &'a mut [Edge<Scalar>],
    pub lines: &'a mut [Line],
}

pub struct DashedStrokeWorkspace<'a> {
    pub path: StrokePathWorkspace<'a, Scalar>,
    pub dash_points: &'a mut [Point<Scalar>],
    pub dash_contours: &'a mut [DashContour],
    pub geometry: GeometryWorkspace<'a>,
}

pub struct StrokePlanningWorkspace<'a> {
    pub path: StrokePathWorkspace<'a, Scalar>,
    pub geometry: GeometryWorkspace<'a>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderRequirements {
    pub edges: usize,
    pub lines: usize,
    pub segments: usize,
    pub trapezoids: usize,
    pub row_area: usize,
    pub strip_offsets: usize,
    pub strip_indices: usize,
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

#[derive(Clone, Copy, Debug, PartialEq)] pub struct RenderOptions {
    pub transform: Affine<Scalar>,
    pub flatten: FlattenOptions,
    pub fill_rule: FillRule,
}

impl Default for RenderOptions { fn default() -> Self {
    Self { transform: Affine::identity(), flatten: FlattenOptions::default(),
        fill_rule: FillRule::NonZero }
} }

#[derive(Clone, Copy, Debug, Default, PartialEq)] pub struct StrokePathOptions {
    pub transform: Affine<Scalar>,
    pub flatten: FlattenOptions,
    pub stroke: StrokeOptions,
}

#[derive(Clone, Copy, Debug)] pub struct DashedStrokePathOptions<'a> {
    pub path: StrokePathOptions,
    pub dash: DashPattern<'a>,
}

/// Computes exact fixed fill capacities without touching a render target.
pub fn render_requirements(path: &Path<Scalar>, options: RenderOptions,
    dimensions: (u32, u32), workspace: &mut GeometryWorkspace<'_>) ->
    Result<RenderRequirements, RenderError> {
    let usage = prepare_path(path, options, workspace)?;
    requirements_from_lines(
        usage.edges, &workspace.lines[..usage.lines], dimensions)
}

/// Computes the exact capacities for [`rasterize_path_clip`].
pub fn path_clip_requirements(path: &Path<Scalar>, options: RenderOptions,
    dimensions: (u32, u32), workspace: &mut GeometryWorkspace<'_>) ->
    Result<RenderRequirements, RenderError> {
    render_requirements(path, options, dimensions, workspace)
}

/// Computes exact fixed stroke capacities using caller-owned planning scratch.
pub fn stroke_requirements(path: &Path<Scalar>, options: StrokePathOptions,
    dimensions: (u32, u32), workspace: &mut StrokePlanningWorkspace<'_>) ->
    Result<StrokeRequirements, RenderError> {
    let usage = prepare_stroke_path(
        path, options, &mut workspace.path, &mut workspace.geometry)?;
    Ok(StrokeRequirements {
        render: requirements_from_lines(usage.edges,
            &workspace.geometry.lines[..usage.lines], dimensions)?,
        points: usage.points, contours: usage.contours,
    })
}

/// Computes exact fixed dashed-stroke capacities using caller-owned planning scratch.
pub fn dashed_stroke_requirements(path: &Path<Scalar>,
    options: DashedStrokePathOptions<'_>, dimensions: (u32, u32),
    workspace: &mut DashedStrokeWorkspace<'_>) ->
    Result<DashedStrokeRequirements, RenderError> {
    let usage = prepare_dashed_stroke_path(path, options, workspace)?;
    Ok(DashedStrokeRequirements {
        stroke: StrokeRequirements {
            render: requirements_from_lines(usage.edges,
                &workspace.geometry.lines[..usage.lines], dimensions)?,
            points: usage.points, contours: usage.contours,
        },
        dash_points: usage.dash_points, dash_contours: usage.dash_contours,
    })
}

/// Renders prepared Q24.8 lines through the allocation-free fixed backend.
pub fn render_solid(lines: &[Line],
    color: SRGBA<u8>, fill_rule: FillRule, target: &mut Pixmap<'_>,
    workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    render_paint(lines, &SolidPaint::new(color), fill_rule, target, workspace)
}

/// Renders fixed coverage and solid paint through an antialiased rectangle clip.
pub fn render_solid_clipped(lines: &[Line],
    color: SRGBA<u8>, clip: Rect<Scalar>, fill_rule: FillRule, target: &mut Pixmap<'_>,
    workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    render_paint_clipped(
        lines, &SolidPaint::new(color), clip, fill_rule, target, workspace)
}

/// Renders fixed coverage and solid paint multiplied by a borrowed path mask.
pub fn render_solid_masked(lines: &[Line],
    color: SRGBA<u8>, mask: CoverageMask<'_>, fill_rule: FillRule,
    target: &mut Pixmap<'_>, workspace: &mut Workspace<'_>) ->
    Result<(), RenderError> {
    render_paint_masked(
        lines, &SolidPaint::new(color), mask, fill_rule, target, workspace)
}

/// Renders prepared Q24.8 lines with a no-FPU fixed paint sampler.
pub fn render_paint<
    S: PaintSampler>(lines: &[Line], sampler: &S,
    fill_rule: FillRule, target: &mut Pixmap<'_>,
    workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let mut compositor = PaintCompositor { target, sampler };
    rasterize_lines(lines, compositor.target.width(), compositor.target.height(),
        fill_rule, workspace, &mut compositor).map_err(map_render_error)
}

/// Transforms, flattens, and fills a Q24.8 path without floating-point operations.
pub fn render_path<
    S: PaintSampler>(path: &Path<Scalar>,
    sampler: &S, options: RenderOptions,
    target: &mut Pixmap<'_>, geometry: &mut GeometryWorkspace<'_>,
    raster_workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let usage = prepare_path(path, options, geometry)?;
    render_paint(&geometry.lines[..usage.lines], sampler,
        options.fill_rule, target, raster_workspace)
}

/// Transforms, flattens, and fills a Q24.8 path through a rectangle clip.
pub fn render_path_clipped<
    S: PaintSampler>(path: &Path<Scalar>,
    sampler: &S, clip: Rect<Scalar>, options: RenderOptions,
    target: &mut Pixmap<'_>, geometry: &mut GeometryWorkspace<'_>,
    raster_workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let usage = prepare_path(path, options, geometry)?;
    render_paint_clipped(&geometry.lines[..usage.lines], sampler,
        clip, options.fill_rule, target, raster_workspace)
}

/// Transforms, flattens, and fills a Q24.8 path through a coverage mask.
pub fn render_path_masked<
    S: PaintSampler>(path: &Path<Scalar>,
    sampler: &S, mask: CoverageMask<'_>, options: RenderOptions,
    target: &mut Pixmap<'_>, geometry: &mut GeometryWorkspace<'_>,
    raster_workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let usage = prepare_path(path, options, geometry)?;
    render_paint_masked(&geometry.lines[..usage.lines], sampler,
        mask, options.fill_rule, target, raster_workspace)
}

pub(crate) fn render_path_sparse_masked<
    S: PaintSampler>(path: &Path<Scalar>, sampler: &S, mask: CoverageStrips<'_>,
    options: RenderOptions, target: &mut Pixmap<'_>,
    geometry: &mut GeometryWorkspace<'_>, raster_workspace: &mut Workspace<'_>) ->
    Result<(), RenderError> {
    let usage = prepare_path(path, options, geometry)?;
    render_paint_sparse_masked(&geometry.lines[..usage.lines], sampler,
        mask, options.fill_rule, target, raster_workspace)
}

/// Expands and renders a Q24.8 polyline with no floating-point operations.
pub fn render_stroke_polyline<
    S: PaintSampler>(points: &[Point<Scalar>], closed: bool,
    stroke: StrokeOptions, sampler: &S, target: &mut Pixmap<'_>,
    geometry: &mut GeometryWorkspace<'_>,
    raster_workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let mut sink = EdgeSliceSink { edges: geometry.edges, len: 0 };
    stroke_polyline(points, closed, stroke, &mut sink)
        .map_err(map_stroke_expand_error)?;
    let line_count = prepare_lines(&sink.edges[..sink.len], geometry.lines)
        .map_err(RenderError::FixedRaster)?;
    render_paint(&geometry.lines[..line_count], sampler,
        FillRule::NonZero, target, raster_workspace)
}

/// Transforms, flattens, expands, and renders a Q24.8 stroked path without an FPU.
pub fn render_stroke_path<
    S: PaintSampler>(path: &Path<Scalar>, sampler: &S,
    options: StrokePathOptions, target: &mut Pixmap<'_>,
    path_workspace: &mut StrokePathWorkspace<'_, Scalar>,
    geometry: &mut GeometryWorkspace<'_>,
    raster_workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let usage = prepare_stroke_path(path, options, path_workspace, geometry)?;
    render_paint(&geometry.lines[..usage.lines], sampler,
        FillRule::NonZero, target, raster_workspace)
}

pub(crate) struct PreparedUsage {
    pub(crate) points: usize, pub(crate) contours: usize,
    pub(crate) edges: usize, pub(crate) lines: usize,
}

pub(crate) struct DashedPreparedUsage {
    pub(crate) points: usize, pub(crate) contours: usize,
    pub(crate) edges: usize, pub(crate) lines: usize,
    pub(crate) dash_points: usize, pub(crate) dash_contours: usize,
}

fn requirements_from_lines(edges: usize, lines: &[Line], dimensions: (u32, u32)) ->
    Result<RenderRequirements, RenderError> {
    let (width, height) = dimensions;
    let limit = DEVICE_RAW_LIMIT as u32 / COORD_SCALE as u32;
    if width > limit || height > limit {
        return Err(RenderError::FixedRaster(RasterError::CoordinateOutOfRange));
    }
    let bins = strip_requirements(lines, height).map_err(RenderError::FixedRaster)?;
    Ok(RenderRequirements {
        edges, lines: lines.len(), segments: lines.len(),
        trapezoids: lines.len().div_ceil(2),
        row_area: usize::try_from(width).map_err(|_| RenderError::DimensionsOverflow)?,
        strip_offsets: bins.offsets, strip_indices: bins.indices,
    })
}

fn prepare_path(path: &Path<Scalar>, options: RenderOptions,
    geometry: &mut GeometryWorkspace<'_>) -> Result<PreparedUsage, RenderError> {
    let mut sink = EdgeSliceSink { edges: geometry.edges, len: 0 };
    build_fill_edges(path, options.transform, options.flatten, &mut sink)
        .map_err(map_flatten_error)?;
    let lines = prepare_lines(&sink.edges[..sink.len], geometry.lines)
        .map_err(RenderError::FixedRaster)?;
    Ok(PreparedUsage { points: 0, contours: 0, edges: sink.len, lines })
}

pub(crate) fn prepare_stroke_path(
    path: &Path<Scalar>, options: StrokePathOptions,
    path_workspace: &mut StrokePathWorkspace<'_, Scalar>,
    geometry: &mut GeometryWorkspace<'_>) -> Result<PreparedUsage, RenderError> {
    let flattened = flatten_stroke_path(
        path, options.transform, options.flatten, path_workspace)
        .map_err(map_stroke_flatten_error)?;
    let (points, contours) = (flattened.point_count(), flattened.contour_count());
    let mut sink = EdgeSliceSink { edges: geometry.edges, len: 0 };
    for (points, closed) in flattened.contours() {
        stroke_polyline(points, closed, options.stroke, &mut sink)
            .map_err(map_stroke_expand_error)?;
    }
    let lines = prepare_lines(&sink.edges[..sink.len], geometry.lines)
        .map_err(RenderError::FixedRaster)?;
    Ok(PreparedUsage { points, contours, edges: sink.len, lines })
}

/// Renders a transformed, dashed Q24.8 path without floating-point operations.
pub fn render_dashed_stroke_path<
    S: PaintSampler>(path: &Path<Scalar>, sampler: &S,
    options: DashedStrokePathOptions<'_>, target: &mut Pixmap<'_>,
    workspace: &mut DashedStrokeWorkspace<'_>,
    raster_workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let usage = prepare_dashed_stroke_path(path, options, workspace)?;
    render_paint(&workspace.geometry.lines[..usage.lines], sampler,
        FillRule::NonZero, target, raster_workspace)
}

pub(crate) fn prepare_dashed_stroke_path(path: &Path<Scalar>,
    options: DashedStrokePathOptions<'_>, workspace: &mut DashedStrokeWorkspace<'_>) ->
    Result<DashedPreparedUsage, RenderError> {
    let flattened = flatten_stroke_path(path, options.path.transform,
        options.path.flatten, &mut workspace.path)
        .map_err(map_stroke_flatten_error)?;
    let (points, contours) = (flattened.point_count(), flattened.contour_count());
    let (mut dash_points, mut dash_contours) = (0, 0);
    let mut sink = EdgeSliceSink { edges: workspace.geometry.edges, len: 0 };
    for (points, closed) in flattened.contours() {
        let mut dash_workspace = DashWorkspace {
            points: workspace.dash_points, contours: workspace.dash_contours,
        };
        let dashed = dash_polyline(points, closed, options.dash, &mut dash_workspace)
            .map_err(map_dash_error)?;
        dash_points = dash_points.max(dashed.point_count());
        dash_contours = dash_contours.max(dashed.contour_count());
        for (points, closed) in dashed.contours() {
            stroke_polyline(points, closed, options.path.stroke, &mut sink)
                .map_err(map_stroke_expand_error)?;
        }
    }
    let lines = prepare_lines(&sink.edges[..sink.len], workspace.geometry.lines)
        .map_err(RenderError::FixedRaster)?;
    Ok(DashedPreparedUsage {
        points, contours, edges: sink.len, lines, dash_points, dash_contours,
    })
}

/// Renders fixed geometry and no-FPU paint through a rectangle clip.
pub fn render_paint_clipped<
    S: PaintSampler>(lines: &[Line], sampler: &S,
    clip: Rect<Scalar>, fill_rule: FillRule, target: &mut Pixmap<'_>,
    workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width(), target.height());
    let mut compositor = PaintCompositor { target, sampler };
    let region = fixed_clip_region(clip, width, height);
    if fixed_rect_is_integer(clip) {
        rasterize_lines_region(lines, width, height, region, fill_rule, workspace,
            &mut compositor).map_err(map_render_error)
    } else {
        rasterize_lines_region(lines, width, height, region, fill_rule, workspace,
            &mut FixedRectClipSink::new(clip, &mut compositor)).map_err(map_render_error)
    }
}

/// Renders fixed geometry and no-FPU paint through a borrowed path mask.
pub fn render_paint_masked<
    S: PaintSampler>(lines: &[Line], sampler: &S,
    mask: CoverageMask<'_>, fill_rule: FillRule, target: &mut Pixmap<'_>,
    workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    render_paint_with_mask(lines, sampler, mask, fill_rule, target, workspace)
}

pub(crate) fn render_paint_with_mask<M: ClipMask,
    S: PaintSampler>(lines: &[Line], sampler: &S,
    mask: M, fill_rule: FillRule, target: &mut Pixmap<'_>,
    workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let (mask_width, mask_height) = mask.dimensions();
    validate_coverage_dimensions(mask_width, mask_height, target)?;
    let (width, height) = (target.width(), target.height());
    let mut compositor = PaintCompositor { target, sampler };
    let region = mask.bounds().unwrap_or_default();
    rasterize_lines_region(lines, width, height, region, fill_rule, workspace,
        &mut MaskClipSink::new(mask, &mut compositor)).map_err(map_render_error)
}

pub(crate) fn render_paint_sparse_masked<S: PaintSampler>(lines: &[Line], sampler: &S,
    mask: CoverageStrips<'_>, fill_rule: FillRule, target: &mut Pixmap<'_>,
    workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width(), target.height());
    let mut compositor = PaintCompositor { target, sampler };
    let region = mask.bounds().unwrap_or_default();
    rasterize_lines_region(lines, width, height, region, fill_rule, workspace,
        &mut SparseClipSink::new(mask, &mut compositor)).map_err(map_render_error)
}

/// Renders prepared Q24.8 lines through direct sparse tiles.
pub fn render_solid_tiled(lines: &[Line], color: SRGBA<u8>, fill_rule: FillRule,
    target: &mut Pixmap<'_>, raster_workspace: &mut Workspace<'_>,
    tile_workspace: DirectTileWorkspace<'_, '_>) -> Result<(), RenderError> {
    let tiled = rasterize_lines_to_tiles(lines, target.width(), target.height(), fill_rule,
        raster_workspace, tile_workspace).map_err(RenderError::FixedRaster)?;
    composite_solid_tiles(tiled, color, target)
}

/// Renders prepared fixed lines through direct sparse tiles and no-FPU paint.
pub fn render_paint_tiled<
    S: PaintSampler>(lines: &[Line], sampler: &S,
    fill_rule: FillRule, target: &mut Pixmap<'_>,
    raster_workspace: &mut Workspace<'_>,
    tile_workspace: DirectTileWorkspace<'_, '_>) -> Result<(), RenderError> {
    let tiled = rasterize_lines_to_tiles(lines, target.width(), target.height(), fill_rule,
        raster_workspace, tile_workspace).map_err(RenderError::FixedRaster)?;
    composite_paint_tiles(tiled, sampler, target)
}

/// Composites retained fixed strips with a no-FPU fixed paint sampler.
pub fn composite_paint_strips<
    S: PaintSampler>(strips: CoverageStrips<'_>,
    sampler: &S, target: &mut Pixmap<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    finish_infallible(strips.replay(&mut PaintCompositor { target, sampler }))
}

/// Composites retained fixed strips and no-FPU paint through a rectangle clip.
pub fn composite_paint_strips_clipped<
    S: PaintSampler>(strips: CoverageStrips<'_>,
    sampler: &S, clip: Rect<Scalar>, target: &mut Pixmap<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut FixedRectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed strips and no-FPU paint through a path mask.
pub fn composite_paint_strips_masked<
    S: PaintSampler>(strips: CoverageStrips<'_>,
    sampler: &S, mask: CoverageMask<'_>,
    target: &mut Pixmap<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut MaskClipSink::new(mask, &mut compositor)))
}

/// Composites retained fixed coverage without rasterizing its geometry again.
pub fn composite_solid_tiles(tiled: CoverageTiles<'_>,
    color: SRGBA<u8>, target: &mut Pixmap<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let paint = SolidPaint::new(color);
    let compositor = PaintCompositor { target, sampler: &paint };
    for tile in tiled.tiles() {
        match tile.kind {
            TileKind::Full => {
                let (width, height) = tiled.tile_extent(*tile);
                compositor.target.blend_solid_tile(
                    tile.x, tile.y, width, height, paint.color());
            }
            TileKind::Boundary => {
                let start = tile.run_start as usize;
                for run in &tiled.runs()[start..start + tile.run_count as usize] {
                    compositor.target.blend_solid_span(tile.x + run.x as u32,
                        tile.y + run.row as u32, run.len as _,
                        paint.color(), run.coverage);
                }
            }
        }
    }   Ok(())
}

/// Composites retained fixed tiles with a no-FPU fixed paint sampler.
pub fn composite_paint_tiles<
    S: PaintSampler>(tiled: CoverageTiles<'_>,
    sampler: &S, target: &mut Pixmap<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(replay_tiles(tiled, &mut compositor))
}

/// Composites retained fixed tiles and no-FPU paint through a rectangle clip.
pub fn composite_paint_tiles_clipped<
    S: PaintSampler>(tiled: CoverageTiles<'_>,
    sampler: &S, clip: Rect<Scalar>, target: &mut Pixmap<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(replay_tiles(
        tiled, &mut FixedRectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed tiles and no-FPU paint through a path mask.
pub fn composite_paint_tiles_masked<
    S: PaintSampler>(tiled: CoverageTiles<'_>,
    sampler: &S, mask: CoverageMask<'_>,
    target: &mut Pixmap<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(replay_tiles(
        tiled, &mut MaskClipSink::new(mask, &mut compositor)))
}

fn replay_tiles<S: CoverageSink>(
    tiled: CoverageTiles<'_>, sink: &mut S) -> Result<(), S::Error> {
    for tile in tiled.tiles() {
        match tile.kind {
            TileKind::Full => {
                let (width, height) = tiled.tile_extent(*tile);
                for row in 0..height {
                    sink.span(tile.x, tile.y + row, width, u8::MAX)?;
                }
            }
            TileKind::Boundary => {
                let start = tile.run_start as usize;
                for run in &tiled.runs()[start..start + tile.run_count as usize] {
                    sink.span(tile.x + run.x as u32, tile.y + run.row as u32,
                        run.len as _, run.coverage)?;
                }
            }
        }
    }   Ok(())
}

fn finish_infallible(result: Result<(), Infallible>) ->
    Result<(), RenderError> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => match error {},
    }
}

/// Rasterizes a Q24.8 path clip into caller-owned 8-bit coverage without an FPU.
///
/// The valid mask area is cleared after path flattening and line preparation
/// succeed. Callers must discard the mask if this function returns an error.
pub fn rasterize_path_clip(
    path: &Path<Scalar>, options: RenderOptions,
    mask: &mut CoverageMaskMut<'_>, geometry: &mut GeometryWorkspace<'_>,
    raster_workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let mut sink = EdgeSliceSink { edges: geometry.edges, len: 0 };
    build_fill_edges(path, options.transform, options.flatten, &mut sink)
        .map_err(map_flatten_error)?;
    let line_count = prepare_lines(&sink.edges[..sink.len], geometry.lines)
        .map_err(RenderError::FixedRaster)?;
    mask.clear();
    rasterize_lines(&geometry.lines[..line_count], mask.width(), mask.height(),
        options.fill_rule, raster_workspace, mask).map_err(map_render_error)
}


pub(crate) fn map_stroke_expand_error(
    error: StrokeExpandError<EdgeCapacity>) -> RenderError {
    match error {
        StrokeExpandError::CoordinateOutOfRange =>
            RenderError::FixedRaster(RasterError::CoordinateOutOfRange),
        StrokeExpandError::Sink(error) =>
            RenderError::EdgeCapacity { needed_at_least: error.needed_at_least },
    }
}

pub(crate) fn map_flatten_error(
    error: FlattenError<EdgeCapacity>) -> RenderError {
    match error {
        FlattenError::NonPositiveTolerance => RenderError::InvalidTolerance,
        FlattenError::InvalidDepth => RenderError::InvalidDepth,
        FlattenError::CoordinateOutOfRange =>
            RenderError::FixedRaster(RasterError::CoordinateOutOfRange),
        FlattenError::DepthLimit => RenderError::FlattenDepthLimit,
        FlattenError::InvalidPath(error) => RenderError::InvalidPath(error),
        FlattenError::Sink(error) =>
            RenderError::EdgeCapacity { needed_at_least: error.needed_at_least },
    }
}

pub(crate) fn map_stroke_flatten_error(
    error: FlattenError<StrokeWorkspaceError>) -> RenderError {
    match error {
        FlattenError::NonPositiveTolerance => RenderError::InvalidTolerance,
        FlattenError::InvalidDepth => RenderError::InvalidDepth,
        FlattenError::CoordinateOutOfRange =>
            RenderError::FixedRaster(RasterError::CoordinateOutOfRange),
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


pub(crate) struct PaintCompositor<'a, 'b, S> {
    pub(crate) target: &'a mut Pixmap<'b>, pub(crate) sampler: &'a S,
}

impl<S: PaintSampler> CoverageSink for PaintCompositor<'_, '_, S> {
    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        blend_sampled_span(self.target, x, y, len, self.sampler, coverage);
        Ok(())
    }   type Error = Infallible;
}


pub(crate) fn map_render_error(
    error: RasterRenderError<Infallible>) -> RenderError {
    match error {
        RasterRenderError::Raster(error) => RenderError::FixedRaster(error),
        RasterRenderError::Sink(error) => match error {},
    }
}

#[cfg(test)] #[path = "canvas_tests.rs"] mod tests;
