//! Workspace and option types for fixed-point rendering.

use core::convert::Infallible;
use crate::{
    canvas::{EdgeSliceSink, FixedPaintCompositor, PaintCompositor, PixmapMut, RenderError,
        map_dash_error, map_fixed_flatten_error, map_fixed_render_error,
        map_fixed_stroke_expand_error, map_fixed_stroke_flatten_error,
        validate_coverage_dimensions},
    color::SRGBA, dash::{DashContour, DashWorkspace, FixedDashPattern,
        dash_polyline_fixed}, edge::{Edge, build_fill_edges_fixed},
    fixed::{flatten::FixedFlattenOptions,
        raster::{FixedCoverageStrips, FixedLine, FixedRasterWorkspace, prepare_lines,
            rasterize_lines}, stroke::{FixedStrokeOptions,
            stroke_polyline_fixed}, tile::{FixedCoverageTiles, FixedDirectTileWorkspace,
            FixedTileKind, rasterize_lines_to_tiles}},
    geometry::{Affine, FixedScalar, Path, Point, Rect},
    raster::{CoverageMask, CoverageSink, FillRule, MaskClipSink, RectClipSink},
    sampler::{PaintSampler, SolidPaint}, stroke::{StrokePathWorkspace,
        flatten_stroke_path_fixed},
};

pub struct FixedGeometryWorkspace<'a> {
    pub edges: &'a mut [Edge<FixedScalar>],
    pub lines: &'a mut [FixedLine],
}

pub struct FixedDashedStrokeWorkspace<'a> {
    pub path: StrokePathWorkspace<'a, FixedScalar>,
    pub dash_points: &'a mut [Point<FixedScalar>],
    pub dash_contours: &'a mut [DashContour],
    pub geometry: FixedGeometryWorkspace<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq)] pub struct FixedRenderOptions {
    pub transform: Affine<FixedScalar>,
    pub flatten: FixedFlattenOptions,
    pub fill_rule: FillRule,
}

impl Default for FixedRenderOptions { fn default() -> Self {
    Self { transform: Affine::identity(), flatten: FixedFlattenOptions::default(),
        fill_rule: FillRule::NonZero }
} }

#[derive(Clone, Copy, Debug, Default, PartialEq)] pub struct FixedStrokePathOptions {
    pub transform: Affine<FixedScalar>,
    pub flatten: FixedFlattenOptions,
    pub stroke: FixedStrokeOptions,
}

#[derive(Clone, Copy, Debug)] pub struct FixedDashedStrokePathOptions<'a> {
    pub path: FixedStrokePathOptions,
    pub dash: FixedDashPattern<'a>,
}

/// Renders prepared Q24.8 lines through the allocation-free fixed backend.
pub fn render_solid_fixed(lines: &[FixedLine],
    color: SRGBA<u8>, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    render_paint_fixed(lines, &SolidPaint::new(color), fill_rule, target, workspace)
}

/// Renders prepared Q24.8 lines through the shared encoded paint compositor.
///
/// Raster geometry and coverage are fixed-point; the supplied sampler retains
/// its own numeric contract and may use floating point.
pub fn render_paint_fixed<S: PaintSampler>(lines: &[FixedLine],
    sampler: &S, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let mut compositor = PaintCompositor { target, sampler };
    rasterize_lines(lines, compositor.target.width(), compositor.target.height(),
        fill_rule, workspace, &mut compositor).map_err(map_fixed_render_error)
}

/// Renders fixed coverage and solid paint through an antialiased rectangle clip.
pub fn render_solid_fixed_clipped(lines: &[FixedLine],
    color: SRGBA<u8>, clip: Rect, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    render_paint_fixed_clipped(
        lines, &SolidPaint::new(color), clip, fill_rule, target, workspace)
}

/// Renders fixed coverage and sampled paint through an antialiased rectangle clip.
pub fn render_paint_fixed_clipped<S: PaintSampler>(
    lines: &[FixedLine], sampler: &S, clip: Rect, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, workspace: &mut FixedRasterWorkspace<'_>) ->
    Result<(), RenderError> {
    let (width, height) = (target.width(), target.height());
    let mut compositor = PaintCompositor { target, sampler };
    rasterize_lines(lines, width, height, fill_rule, workspace,
        &mut RectClipSink::new(clip, &mut compositor)).map_err(map_fixed_render_error)
}

/// Renders fixed coverage and solid paint multiplied by a borrowed path mask.
pub fn render_solid_fixed_masked(lines: &[FixedLine],
    color: SRGBA<u8>, mask: CoverageMask<'_>, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, workspace: &mut FixedRasterWorkspace<'_>) ->
    Result<(), RenderError> {
    render_paint_fixed_masked(
        lines, &SolidPaint::new(color), mask, fill_rule, target, workspace)
}

/// Renders fixed coverage and sampled paint multiplied by a borrowed path mask.
pub fn render_paint_fixed_masked<S: PaintSampler>(
    lines: &[FixedLine], sampler: &S, mask: CoverageMask<'_>, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, workspace: &mut FixedRasterWorkspace<'_>) ->
    Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width(), target.height());
    let mut compositor = PaintCompositor { target, sampler };
    rasterize_lines(lines, width, height, fill_rule, workspace,
        &mut MaskClipSink::new(mask, &mut compositor)).map_err(map_fixed_render_error)
}

/// Renders prepared Q24.8 lines with a no-FPU fixed paint sampler.
pub fn render_native_paint_fixed<
    S: crate::sampler::FixedPaintSampler>(lines: &[FixedLine], sampler: &S,
    fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let mut compositor = FixedPaintCompositor { target, sampler };
    rasterize_lines(lines, compositor.target.width(), compositor.target.height(),
        fill_rule, workspace, &mut compositor).map_err(map_fixed_render_error)
}

/// Transforms, flattens, and fills a Q24.8 path without floating-point operations.
pub fn render_native_path_fixed<
    S: crate::sampler::FixedPaintSampler>(path: &Path<FixedScalar>,
    sampler: &S, options: FixedRenderOptions,
    target: &mut PixmapMut<'_>, geometry: &mut FixedGeometryWorkspace<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let line_count = prepare_fixed_path(path, options, geometry)?;
    render_native_paint_fixed(&geometry.lines[..line_count], sampler,
        options.fill_rule, target, raster_workspace)
}

/// Transforms, flattens, and fills a Q24.8 path through a rectangle clip.
pub fn render_native_path_fixed_clipped<
    S: crate::sampler::FixedPaintSampler>(path: &Path<FixedScalar>,
    sampler: &S, clip: Rect, options: FixedRenderOptions,
    target: &mut PixmapMut<'_>, geometry: &mut FixedGeometryWorkspace<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let line_count = prepare_fixed_path(path, options, geometry)?;
    render_native_paint_fixed_clipped(&geometry.lines[..line_count], sampler,
        clip, options.fill_rule, target, raster_workspace)
}

/// Transforms, flattens, and fills a Q24.8 path through a coverage mask.
pub fn render_native_path_fixed_masked<
    S: crate::sampler::FixedPaintSampler>(path: &Path<FixedScalar>,
    sampler: &S, mask: CoverageMask<'_>, options: FixedRenderOptions,
    target: &mut PixmapMut<'_>, geometry: &mut FixedGeometryWorkspace<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let line_count = prepare_fixed_path(path, options, geometry)?;
    render_native_paint_fixed_masked(&geometry.lines[..line_count], sampler,
        mask, options.fill_rule, target, raster_workspace)
}

/// Expands and renders a Q24.8 polyline with no floating-point operations.
pub fn render_native_stroke_polyline_fixed<
    S: crate::sampler::FixedPaintSampler>(points: &[Point<FixedScalar>], closed: bool,
    stroke: FixedStrokeOptions, sampler: &S, target: &mut PixmapMut<'_>,
    geometry: &mut FixedGeometryWorkspace<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let mut sink = EdgeSliceSink { edges: geometry.edges, len: 0 };
    stroke_polyline_fixed(points, closed, stroke, &mut sink)
        .map_err(map_fixed_stroke_expand_error)?;
    let line_count = prepare_lines(&sink.edges[..sink.len], geometry.lines)
        .map_err(RenderError::FixedRaster)?;
    render_native_paint_fixed(&geometry.lines[..line_count], sampler,
        FillRule::NonZero, target, raster_workspace)
}

/// Transforms, flattens, expands, and renders a Q24.8 stroked path without an FPU.
pub fn render_native_stroke_path_fixed<
    S: crate::sampler::FixedPaintSampler>(path: &Path<FixedScalar>, sampler: &S,
    options: FixedStrokePathOptions, target: &mut PixmapMut<'_>,
    path_workspace: &mut StrokePathWorkspace<'_, FixedScalar>,
    geometry: &mut FixedGeometryWorkspace<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let line_count = prepare_fixed_stroke_path(path, options, path_workspace, geometry)?;
    render_native_paint_fixed(&geometry.lines[..line_count], sampler,
        FillRule::NonZero, target, raster_workspace)
}

fn prepare_fixed_path(path: &Path<FixedScalar>, options: FixedRenderOptions,
    geometry: &mut FixedGeometryWorkspace<'_>) -> Result<usize, RenderError> {
    let mut sink = EdgeSliceSink { edges: geometry.edges, len: 0 };
    build_fill_edges_fixed(path, options.transform, options.flatten, &mut sink)
        .map_err(map_fixed_flatten_error)?;
    prepare_lines(&sink.edges[..sink.len], geometry.lines)
        .map_err(RenderError::FixedRaster)
}

pub(crate) fn prepare_fixed_stroke_path(
    path: &Path<FixedScalar>, options: FixedStrokePathOptions,
    path_workspace: &mut StrokePathWorkspace<'_, FixedScalar>,
    geometry: &mut FixedGeometryWorkspace<'_>) -> Result<usize, RenderError> {
    let flattened = flatten_stroke_path_fixed(
        path, options.transform, options.flatten, path_workspace)
        .map_err(map_fixed_stroke_flatten_error)?;
    let mut sink = EdgeSliceSink { edges: geometry.edges, len: 0 };
    for (points, closed) in flattened.contours() {
        stroke_polyline_fixed(points, closed, options.stroke, &mut sink)
            .map_err(map_fixed_stroke_expand_error)?;
    }
    prepare_lines(&sink.edges[..sink.len], geometry.lines)
        .map_err(RenderError::FixedRaster)
}

/// Renders a transformed, dashed Q24.8 path without floating-point operations.
pub fn render_native_stroke_path_dashed_fixed<
    S: crate::sampler::FixedPaintSampler>(path: &Path<FixedScalar>, sampler: &S,
    options: FixedDashedStrokePathOptions<'_>, target: &mut PixmapMut<'_>,
    workspace: &mut FixedDashedStrokeWorkspace<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let flattened = flatten_stroke_path_fixed(path, options.path.transform,
        options.path.flatten, &mut workspace.path)
        .map_err(map_fixed_stroke_flatten_error)?;
    let mut sink = EdgeSliceSink { edges: workspace.geometry.edges, len: 0 };
    for (points, closed) in flattened.contours() {
        let mut dash_workspace = DashWorkspace {
            points: workspace.dash_points, contours: workspace.dash_contours,
        };
        let dashed = dash_polyline_fixed(points, closed, options.dash, &mut dash_workspace)
            .map_err(map_dash_error)?;
        for (points, closed) in dashed.contours() {
            stroke_polyline_fixed(points, closed, options.path.stroke, &mut sink)
                .map_err(map_fixed_stroke_expand_error)?;
        }
    }
    let line_count = prepare_lines(&sink.edges[..sink.len], workspace.geometry.lines)
        .map_err(RenderError::FixedRaster)?;
    render_native_paint_fixed(&workspace.geometry.lines[..line_count], sampler,
        FillRule::NonZero, target, raster_workspace)
}

/// Renders fixed geometry and no-FPU paint through a rectangle clip.
pub fn render_native_paint_fixed_clipped<
    S: crate::sampler::FixedPaintSampler>(lines: &[FixedLine], sampler: &S,
    clip: Rect, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width(), target.height());
    let mut compositor = FixedPaintCompositor { target, sampler };
    rasterize_lines(lines, width, height, fill_rule, workspace,
        &mut RectClipSink::new(clip, &mut compositor)).map_err(map_fixed_render_error)
}

/// Renders fixed geometry and no-FPU paint through a borrowed path mask.
pub fn render_native_paint_fixed_masked<
    S: crate::sampler::FixedPaintSampler>(lines: &[FixedLine], sampler: &S,
    mask: CoverageMask<'_>, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width(), target.height());
    let mut compositor = FixedPaintCompositor { target, sampler };
    rasterize_lines(lines, width, height, fill_rule, workspace,
        &mut MaskClipSink::new(mask, &mut compositor)).map_err(map_fixed_render_error)
}

/// Renders prepared Q24.8 lines through direct sparse tiles.
pub fn render_solid_fixed_tiled(lines: &[FixedLine], color: SRGBA<u8>, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, raster_workspace: &mut FixedRasterWorkspace<'_>,
    tile_workspace: FixedDirectTileWorkspace<'_, '_>) -> Result<(), RenderError> {
    let tiled = rasterize_lines_to_tiles(lines, target.width(), target.height(), fill_rule,
        raster_workspace, tile_workspace).map_err(RenderError::FixedRaster)?;
    composite_solid_fixed_tiles(tiled, color, target)
}

/// Renders prepared fixed lines through direct sparse tiles and sampled paint.
pub fn render_paint_fixed_tiled<S: PaintSampler>(
    lines: &[FixedLine], sampler: &S, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>,
    tile_workspace: FixedDirectTileWorkspace<'_, '_>) -> Result<(), RenderError> {
    let tiled = rasterize_lines_to_tiles(lines, target.width(), target.height(), fill_rule,
        raster_workspace, tile_workspace).map_err(RenderError::FixedRaster)?;
    composite_paint_fixed_tiles(tiled, sampler, target)
}

/// Renders prepared fixed lines through direct sparse tiles and no-FPU paint.
pub fn render_native_paint_fixed_tiled<
    S: crate::sampler::FixedPaintSampler>(lines: &[FixedLine], sampler: &S,
    fill_rule: FillRule, target: &mut PixmapMut<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>,
    tile_workspace: FixedDirectTileWorkspace<'_, '_>) -> Result<(), RenderError> {
    let tiled = rasterize_lines_to_tiles(lines, target.width(), target.height(), fill_rule,
        raster_workspace, tile_workspace).map_err(RenderError::FixedRaster)?;
    composite_native_paint_fixed_tiles(tiled, sampler, target)
}

/// Composites retained fixed strips through the shared paint compositor.
pub fn composite_paint_fixed_strips<S: PaintSampler>(
    strips: FixedCoverageStrips<'_>, sampler: &S, target: &mut PixmapMut<'_>) ->
    Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    finish_infallible(strips.replay(&mut PaintCompositor { target, sampler }))
}

/// Composites retained fixed strips with a no-FPU fixed paint sampler.
pub fn composite_native_paint_fixed_strips<
    S: crate::sampler::FixedPaintSampler>(strips: FixedCoverageStrips<'_>,
    sampler: &S, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    finish_infallible(strips.replay(&mut FixedPaintCompositor { target, sampler }))
}

/// Composites retained fixed strips and no-FPU paint through a rectangle clip.
pub fn composite_native_paint_fixed_strips_clipped<
    S: crate::sampler::FixedPaintSampler>(strips: FixedCoverageStrips<'_>,
    sampler: &S, clip: Rect, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    let mut compositor = FixedPaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut RectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed strips and no-FPU paint through a path mask.
pub fn composite_native_paint_fixed_strips_masked<
    S: crate::sampler::FixedPaintSampler>(strips: FixedCoverageStrips<'_>,
    sampler: &S, mask: CoverageMask<'_>,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let mut compositor = FixedPaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut MaskClipSink::new(mask, &mut compositor)))
}

/// Composites retained fixed strips through an antialiased rectangle clip.
pub fn composite_paint_fixed_strips_clipped<S: PaintSampler>(
    strips: FixedCoverageStrips<'_>, sampler: &S, clip: Rect,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut RectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed strips multiplied by a borrowed path mask.
pub fn composite_paint_fixed_strips_masked<S: PaintSampler>(
    strips: FixedCoverageStrips<'_>, sampler: &S, mask: CoverageMask<'_>,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut MaskClipSink::new(mask, &mut compositor)))
}

/// Composites retained fixed coverage without rasterizing its geometry again.
pub fn composite_solid_fixed_tiles(tiled: FixedCoverageTiles<'_>,
    color: SRGBA<u8>, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let paint = SolidPaint::new(color);
    let compositor = PaintCompositor { target, sampler: &paint };
    for tile in tiled.tiles() {
        match tile.kind {
            FixedTileKind::Full => {
                let (width, height) = tiled.tile_extent(*tile);
                compositor.target.blend_solid_tile(
                    tile.x, tile.y, width, height, paint.color().into_legacy());
            }
            FixedTileKind::Boundary => {
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
pub fn composite_paint_fixed_tiles<S: PaintSampler>(
    tiled: FixedCoverageTiles<'_>, sampler: &S, target: &mut PixmapMut<'_>) ->
    Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(replay_fixed_tiles(tiled, &mut compositor))
}

/// Composites retained fixed tiles with a no-FPU fixed paint sampler.
pub fn composite_native_paint_fixed_tiles<
    S: crate::sampler::FixedPaintSampler>(tiled: FixedCoverageTiles<'_>,
    sampler: &S, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = FixedPaintCompositor { target, sampler };
    finish_infallible(replay_fixed_tiles(tiled, &mut compositor))
}

/// Composites retained fixed tiles and no-FPU paint through a rectangle clip.
pub fn composite_native_paint_fixed_tiles_clipped<
    S: crate::sampler::FixedPaintSampler>(tiled: FixedCoverageTiles<'_>,
    sampler: &S, clip: Rect, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = FixedPaintCompositor { target, sampler };
    finish_infallible(replay_fixed_tiles(
        tiled, &mut RectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed tiles and no-FPU paint through a path mask.
pub fn composite_native_paint_fixed_tiles_masked<
    S: crate::sampler::FixedPaintSampler>(tiled: FixedCoverageTiles<'_>,
    sampler: &S, mask: CoverageMask<'_>,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let mut compositor = FixedPaintCompositor { target, sampler };
    finish_infallible(replay_fixed_tiles(
        tiled, &mut MaskClipSink::new(mask, &mut compositor)))
}

/// Composites retained fixed tiles through an antialiased rectangle clip.
pub fn composite_paint_fixed_tiles_clipped<S: PaintSampler>(
    tiled: FixedCoverageTiles<'_>, sampler: &S, clip: Rect,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(replay_fixed_tiles(
        tiled, &mut RectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed tiles multiplied by a borrowed path mask.
pub fn composite_paint_fixed_tiles_masked<S: PaintSampler>(
    tiled: FixedCoverageTiles<'_>, sampler: &S, mask: CoverageMask<'_>,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(replay_fixed_tiles(
        tiled, &mut MaskClipSink::new(mask, &mut compositor)))
}

fn replay_fixed_tiles<S: CoverageSink>(
    tiled: FixedCoverageTiles<'_>, sink: &mut S) -> Result<(), S::Error> {
    for tile in tiled.tiles() {
        match tile.kind {
            FixedTileKind::Full => {
                let (width, height) = tiled.tile_extent(*tile);
                for row in 0..height {
                    sink.span(tile.x, tile.y + row, width, u8::MAX)?;
                }
            }
            FixedTileKind::Boundary => {
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
