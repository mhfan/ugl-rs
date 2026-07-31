//! Workspace and option types for fixed-point rendering.

use core::convert::Infallible;
use crate::{
    canvas::{EdgeCapacity, EdgeSliceSink, PaintCompositor as CompatPaintCompositor,
        PixmapMut, RenderError, map_dash_error, validate_coverage_dimensions},
    color::SRGBA, dash::{DashContour, DashWorkspace}, edge::Edge,
    fixed::{dash::{Pattern as DashPattern, dash_polyline},
        flatten::{Error as FlattenError, Options as FlattenOptions, build_fill_edges},
        raster::{CoverageStrips, Error as RasterError, Line,
            RenderError as RasterRenderError, Workspace, prepare_lines, rasterize_lines},
        sampler::FixedPaintSampler,
        stroke::{ExpandError as StrokeExpandError, Options as StrokeOptions,
            flatten_path as flatten_stroke_path, stroke_polyline},
        tile::{CoverageTiles, DirectTileWorkspace, TileKind, rasterize_lines_to_tiles}},
    geometry::{Affine, FixedScalar, Path, Point, Rect},
    raster::{CoverageMask, CoverageMaskMut, CoverageSink, FillRule, MaskClipSink,
        RectClipSink},
    sampler::{PaintSampler, SolidPaint},
    stroke::{StrokePathWorkspace, StrokeWorkspaceError},
};

impl PixmapMut<'_> {
    fn blend_fixed_sampled_span<S: FixedPaintSampler>(
        &mut self, x: u32, y: u32, len: u32, sampler: &S, coverage: u8) {
        if let Some(color) = sampler.solid_color() {
            self.blend_solid_span(x, y, len, color.into_legacy(), coverage);
            return;
        }
        for pixel_x in x..x + len {
            let color = sampler.sample(pixel_x, y);
            self.blend_solid_span(pixel_x, y, 1, color.into_legacy(), coverage);
        }
    }
}

pub struct GeometryWorkspace<'a> {
    pub edges: &'a mut [Edge<FixedScalar>],
    pub lines: &'a mut [Line],
}

pub struct DashedStrokeWorkspace<'a> {
    pub path: StrokePathWorkspace<'a, FixedScalar>,
    pub dash_points: &'a mut [Point<FixedScalar>],
    pub dash_contours: &'a mut [DashContour],
    pub geometry: GeometryWorkspace<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq)] pub struct RenderOptions {
    pub transform: Affine<FixedScalar>,
    pub flatten: FlattenOptions,
    pub fill_rule: FillRule,
}

impl Default for RenderOptions { fn default() -> Self {
    Self { transform: Affine::identity(), flatten: FlattenOptions::default(),
        fill_rule: FillRule::NonZero }
} }

#[derive(Clone, Copy, Debug, Default, PartialEq)] pub struct StrokePathOptions {
    pub transform: Affine<FixedScalar>,
    pub flatten: FlattenOptions,
    pub stroke: StrokeOptions,
}

#[derive(Clone, Copy, Debug)] pub struct DashedStrokePathOptions<'a> {
    pub path: StrokePathOptions,
    pub dash: DashPattern<'a>,
}

/// Renders prepared Q24.8 lines through the allocation-free fixed backend.
pub fn render_solid(lines: &[Line],
    color: SRGBA<u8>, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    render_compat_paint(lines, &SolidPaint::new(color), fill_rule, target, workspace)
}

/// Renders prepared Q24.8 lines through the shared encoded paint compositor.
///
/// Raster geometry and coverage are fixed-point; the supplied sampler retains
/// its own numeric contract and may use floating point.
pub fn render_compat_paint<S: PaintSampler>(lines: &[Line],
    sampler: &S, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let mut compositor = CompatPaintCompositor { target, sampler };
    rasterize_lines(lines, compositor.target.width(), compositor.target.height(),
        fill_rule, workspace, &mut compositor).map_err(map_render_error)
}

/// Renders fixed coverage and solid paint through an antialiased rectangle clip.
pub fn render_solid_clipped(lines: &[Line],
    color: SRGBA<u8>, clip: Rect, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    render_compat_paint_clipped(
        lines, &SolidPaint::new(color), clip, fill_rule, target, workspace)
}

/// Renders fixed coverage and sampled paint through an antialiased rectangle clip.
pub fn render_compat_paint_clipped<S: PaintSampler>(
    lines: &[Line], sampler: &S, clip: Rect, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, workspace: &mut Workspace<'_>) ->
    Result<(), RenderError> {
    let (width, height) = (target.width(), target.height());
    let mut compositor = CompatPaintCompositor { target, sampler };
    rasterize_lines(lines, width, height, fill_rule, workspace,
        &mut RectClipSink::new(clip, &mut compositor)).map_err(map_render_error)
}

/// Renders fixed coverage and solid paint multiplied by a borrowed path mask.
pub fn render_solid_masked(lines: &[Line],
    color: SRGBA<u8>, mask: CoverageMask<'_>, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, workspace: &mut Workspace<'_>) ->
    Result<(), RenderError> {
    render_compat_paint_masked(
        lines, &SolidPaint::new(color), mask, fill_rule, target, workspace)
}

/// Renders fixed coverage and sampled paint multiplied by a borrowed path mask.
pub fn render_compat_paint_masked<S: PaintSampler>(
    lines: &[Line], sampler: &S, mask: CoverageMask<'_>, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, workspace: &mut Workspace<'_>) ->
    Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width(), target.height());
    let mut compositor = CompatPaintCompositor { target, sampler };
    rasterize_lines(lines, width, height, fill_rule, workspace,
        &mut MaskClipSink::new(mask, &mut compositor)).map_err(map_render_error)
}

/// Renders prepared Q24.8 lines with a no-FPU fixed paint sampler.
pub fn render_paint<
    S: FixedPaintSampler>(lines: &[Line], sampler: &S,
    fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let mut compositor = PaintCompositor { target, sampler };
    rasterize_lines(lines, compositor.target.width(), compositor.target.height(),
        fill_rule, workspace, &mut compositor).map_err(map_render_error)
}

/// Transforms, flattens, and fills a Q24.8 path without floating-point operations.
pub fn render_path<
    S: FixedPaintSampler>(path: &Path<FixedScalar>,
    sampler: &S, options: RenderOptions,
    target: &mut PixmapMut<'_>, geometry: &mut GeometryWorkspace<'_>,
    raster_workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let line_count = prepare_path(path, options, geometry)?;
    render_paint(&geometry.lines[..line_count], sampler,
        options.fill_rule, target, raster_workspace)
}

/// Transforms, flattens, and fills a Q24.8 path through a rectangle clip.
pub fn render_path_clipped<
    S: FixedPaintSampler>(path: &Path<FixedScalar>,
    sampler: &S, clip: Rect, options: RenderOptions,
    target: &mut PixmapMut<'_>, geometry: &mut GeometryWorkspace<'_>,
    raster_workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let line_count = prepare_path(path, options, geometry)?;
    render_paint_clipped(&geometry.lines[..line_count], sampler,
        clip, options.fill_rule, target, raster_workspace)
}

/// Transforms, flattens, and fills a Q24.8 path through a coverage mask.
pub fn render_path_masked<
    S: FixedPaintSampler>(path: &Path<FixedScalar>,
    sampler: &S, mask: CoverageMask<'_>, options: RenderOptions,
    target: &mut PixmapMut<'_>, geometry: &mut GeometryWorkspace<'_>,
    raster_workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let line_count = prepare_path(path, options, geometry)?;
    render_paint_masked(&geometry.lines[..line_count], sampler,
        mask, options.fill_rule, target, raster_workspace)
}

/// Expands and renders a Q24.8 polyline with no floating-point operations.
pub fn render_stroke_polyline<
    S: FixedPaintSampler>(points: &[Point<FixedScalar>], closed: bool,
    stroke: StrokeOptions, sampler: &S, target: &mut PixmapMut<'_>,
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
    S: FixedPaintSampler>(path: &Path<FixedScalar>, sampler: &S,
    options: StrokePathOptions, target: &mut PixmapMut<'_>,
    path_workspace: &mut StrokePathWorkspace<'_, FixedScalar>,
    geometry: &mut GeometryWorkspace<'_>,
    raster_workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let line_count = prepare_stroke_path(path, options, path_workspace, geometry)?;
    render_paint(&geometry.lines[..line_count], sampler,
        FillRule::NonZero, target, raster_workspace)
}

fn prepare_path(path: &Path<FixedScalar>, options: RenderOptions,
    geometry: &mut GeometryWorkspace<'_>) -> Result<usize, RenderError> {
    let mut sink = EdgeSliceSink { edges: geometry.edges, len: 0 };
    build_fill_edges(path, options.transform, options.flatten, &mut sink)
        .map_err(map_flatten_error)?;
    prepare_lines(&sink.edges[..sink.len], geometry.lines)
        .map_err(RenderError::FixedRaster)
}

pub(crate) fn prepare_stroke_path(
    path: &Path<FixedScalar>, options: StrokePathOptions,
    path_workspace: &mut StrokePathWorkspace<'_, FixedScalar>,
    geometry: &mut GeometryWorkspace<'_>) -> Result<usize, RenderError> {
    let flattened = flatten_stroke_path(
        path, options.transform, options.flatten, path_workspace)
        .map_err(map_stroke_flatten_error)?;
    let mut sink = EdgeSliceSink { edges: geometry.edges, len: 0 };
    for (points, closed) in flattened.contours() {
        stroke_polyline(points, closed, options.stroke, &mut sink)
            .map_err(map_stroke_expand_error)?;
    }
    prepare_lines(&sink.edges[..sink.len], geometry.lines)
        .map_err(RenderError::FixedRaster)
}

/// Renders a transformed, dashed Q24.8 path without floating-point operations.
pub fn render_dashed_stroke_path<
    S: FixedPaintSampler>(path: &Path<FixedScalar>, sampler: &S,
    options: DashedStrokePathOptions<'_>, target: &mut PixmapMut<'_>,
    workspace: &mut DashedStrokeWorkspace<'_>,
    raster_workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let flattened = flatten_stroke_path(path, options.path.transform,
        options.path.flatten, &mut workspace.path)
        .map_err(map_stroke_flatten_error)?;
    let mut sink = EdgeSliceSink { edges: workspace.geometry.edges, len: 0 };
    for (points, closed) in flattened.contours() {
        let mut dash_workspace = DashWorkspace {
            points: workspace.dash_points, contours: workspace.dash_contours,
        };
        let dashed = dash_polyline(points, closed, options.dash, &mut dash_workspace)
            .map_err(map_dash_error)?;
        for (points, closed) in dashed.contours() {
            stroke_polyline(points, closed, options.path.stroke, &mut sink)
                .map_err(map_stroke_expand_error)?;
        }
    }
    let line_count = prepare_lines(&sink.edges[..sink.len], workspace.geometry.lines)
        .map_err(RenderError::FixedRaster)?;
    render_paint(&workspace.geometry.lines[..line_count], sampler,
        FillRule::NonZero, target, raster_workspace)
}

/// Renders fixed geometry and no-FPU paint through a rectangle clip.
pub fn render_paint_clipped<
    S: FixedPaintSampler>(lines: &[Line], sampler: &S,
    clip: Rect, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width(), target.height());
    let mut compositor = PaintCompositor { target, sampler };
    rasterize_lines(lines, width, height, fill_rule, workspace,
        &mut RectClipSink::new(clip, &mut compositor)).map_err(map_render_error)
}

/// Renders fixed geometry and no-FPU paint through a borrowed path mask.
pub fn render_paint_masked<
    S: FixedPaintSampler>(lines: &[Line], sampler: &S,
    mask: CoverageMask<'_>, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut Workspace<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width(), target.height());
    let mut compositor = PaintCompositor { target, sampler };
    rasterize_lines(lines, width, height, fill_rule, workspace,
        &mut MaskClipSink::new(mask, &mut compositor)).map_err(map_render_error)
}

/// Renders prepared Q24.8 lines through direct sparse tiles.
pub fn render_solid_tiled(lines: &[Line], color: SRGBA<u8>, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, raster_workspace: &mut Workspace<'_>,
    tile_workspace: DirectTileWorkspace<'_, '_>) -> Result<(), RenderError> {
    let tiled = rasterize_lines_to_tiles(lines, target.width(), target.height(), fill_rule,
        raster_workspace, tile_workspace).map_err(RenderError::FixedRaster)?;
    composite_solid_tiles(tiled, color, target)
}

/// Renders prepared fixed lines through direct sparse tiles and sampled paint.
pub fn render_compat_paint_tiled<S: PaintSampler>(
    lines: &[Line], sampler: &S, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    raster_workspace: &mut Workspace<'_>,
    tile_workspace: DirectTileWorkspace<'_, '_>) -> Result<(), RenderError> {
    let tiled = rasterize_lines_to_tiles(lines, target.width(), target.height(), fill_rule,
        raster_workspace, tile_workspace).map_err(RenderError::FixedRaster)?;
    composite_compat_paint_tiles(tiled, sampler, target)
}

/// Renders prepared fixed lines through direct sparse tiles and no-FPU paint.
pub fn render_paint_tiled<
    S: FixedPaintSampler>(lines: &[Line], sampler: &S,
    fill_rule: FillRule, target: &mut PixmapMut<'_>,
    raster_workspace: &mut Workspace<'_>,
    tile_workspace: DirectTileWorkspace<'_, '_>) -> Result<(), RenderError> {
    let tiled = rasterize_lines_to_tiles(lines, target.width(), target.height(), fill_rule,
        raster_workspace, tile_workspace).map_err(RenderError::FixedRaster)?;
    composite_paint_tiles(tiled, sampler, target)
}

/// Composites retained fixed strips through the shared paint compositor.
pub fn composite_compat_paint_strips<S: PaintSampler>(
    strips: CoverageStrips<'_>, sampler: &S, target: &mut PixmapMut<'_>) ->
    Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    finish_infallible(strips.replay(&mut CompatPaintCompositor { target, sampler }))
}

/// Composites retained fixed strips with a no-FPU fixed paint sampler.
pub fn composite_paint_strips<
    S: FixedPaintSampler>(strips: CoverageStrips<'_>,
    sampler: &S, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    finish_infallible(strips.replay(&mut PaintCompositor { target, sampler }))
}

/// Composites retained fixed strips and no-FPU paint through a rectangle clip.
pub fn composite_paint_strips_clipped<
    S: FixedPaintSampler>(strips: CoverageStrips<'_>,
    sampler: &S, clip: Rect, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut RectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed strips and no-FPU paint through a path mask.
pub fn composite_paint_strips_masked<
    S: FixedPaintSampler>(strips: CoverageStrips<'_>,
    sampler: &S, mask: CoverageMask<'_>,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut MaskClipSink::new(mask, &mut compositor)))
}

/// Composites retained fixed strips through an antialiased rectangle clip.
pub fn composite_compat_paint_strips_clipped<S: PaintSampler>(
    strips: CoverageStrips<'_>, sampler: &S, clip: Rect,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    let mut compositor = CompatPaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut RectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed strips multiplied by a borrowed path mask.
pub fn composite_compat_paint_strips_masked<S: PaintSampler>(
    strips: CoverageStrips<'_>, sampler: &S, mask: CoverageMask<'_>,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let mut compositor = CompatPaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut MaskClipSink::new(mask, &mut compositor)))
}

/// Composites retained fixed coverage without rasterizing its geometry again.
pub fn composite_solid_tiles(tiled: CoverageTiles<'_>,
    color: SRGBA<u8>, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let paint = SolidPaint::new(color);
    let compositor = CompatPaintCompositor { target, sampler: &paint };
    for tile in tiled.tiles() {
        match tile.kind {
            TileKind::Full => {
                let (width, height) = tiled.tile_extent(*tile);
                compositor.target.blend_solid_tile(
                    tile.x, tile.y, width, height, paint.color().into_legacy());
            }
            TileKind::Boundary => {
                let start = tile.run_start as usize;
                for run in &tiled.runs()[start..start + tile.run_count as usize] {
                    compositor.target.blend_solid_span(tile.x + run.x as u32,
                        tile.y + run.row as u32, run.len as _,
                        paint.color().into_legacy(), run.coverage);
                }
            }
        }
    }   Ok(())
}

/// Composites retained fixed tiles through the shared paint compositor.
pub fn composite_compat_paint_tiles<S: PaintSampler>(
    tiled: CoverageTiles<'_>, sampler: &S, target: &mut PixmapMut<'_>) ->
    Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = CompatPaintCompositor { target, sampler };
    finish_infallible(replay_tiles(tiled, &mut compositor))
}

/// Composites retained fixed tiles with a no-FPU fixed paint sampler.
pub fn composite_paint_tiles<
    S: FixedPaintSampler>(tiled: CoverageTiles<'_>,
    sampler: &S, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(replay_tiles(tiled, &mut compositor))
}

/// Composites retained fixed tiles and no-FPU paint through a rectangle clip.
pub fn composite_paint_tiles_clipped<
    S: FixedPaintSampler>(tiled: CoverageTiles<'_>,
    sampler: &S, clip: Rect, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(replay_tiles(
        tiled, &mut RectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed tiles and no-FPU paint through a path mask.
pub fn composite_paint_tiles_masked<
    S: FixedPaintSampler>(tiled: CoverageTiles<'_>,
    sampler: &S, mask: CoverageMask<'_>,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(replay_tiles(
        tiled, &mut MaskClipSink::new(mask, &mut compositor)))
}

/// Composites retained fixed tiles through an antialiased rectangle clip.
pub fn composite_compat_paint_tiles_clipped<S: PaintSampler>(
    tiled: CoverageTiles<'_>, sampler: &S, clip: Rect,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = CompatPaintCompositor { target, sampler };
    finish_infallible(replay_tiles(
        tiled, &mut RectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed tiles multiplied by a borrowed path mask.
pub fn composite_compat_paint_tiles_masked<S: PaintSampler>(
    tiled: CoverageTiles<'_>, sampler: &S, mask: CoverageMask<'_>,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let mut compositor = CompatPaintCompositor { target, sampler };
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
    path: &Path<FixedScalar>, options: RenderOptions,
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
    pub(crate) target: &'a mut PixmapMut<'b>, pub(crate) sampler: &'a S,
}

impl<S: FixedPaintSampler> CoverageSink for PaintCompositor<'_, '_, S> {
    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        self.target.blend_fixed_sampled_span(x, y, len, self.sampler, coverage);
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
