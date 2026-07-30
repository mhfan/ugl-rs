//! Borrowed pixel targets and the first complete reference rendering path.

use core::convert::Infallible;
use crate::{color::{PremulRGBA, RGBA}, edge::{build_fill_edges, Edge, EdgeSink},
    analytic::{AnalyticIntersection, AnalyticWorkspace, rasterize_edges_analytic},
    flatten::{FlattenError, FlattenOptions}, geometry::{Affine, Path, PathError, Rect},
    sampler::{PaintSampler, SolidPaint},
    raster::{CoverageMask, CoverageMaskMut, CoverageSink, FillRule, Intersection,
        MaskClipSink, RasterError, RasterOptions, RasterWorkspace, RectClipSink,
        rasterize_edges,
    }
};
#[cfg(feature = "fixed")] use crate::raster_fixed::{
    FixedLine, FixedRasterError, FixedRasterWorkspace, FixedRenderError, rasterize_lines,
};
#[cfg(feature = "fixed")] use crate::tile_fixed::{
    FixedCoverageTiles, FixedDirectTileWorkspace, FixedTileKind, rasterize_lines_to_tiles,
};

const BYTES_PER_PIXEL: u32 = 4;

#[derive(Debug)] pub struct PixmapMut<'a> {
    data: &'a mut [u8], width: u32, height: u32, stride: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum PixmapError {
    StrideTooSmall { minimum: u32,   actual: u32 },
    BufferTooSmall { minimum: usize, actual: usize },
    DimensionsOverflow,
}

impl<'a> PixmapMut<'a> {
    /// Creates a premultiplied RGBA8888 target with explicit row stride.
    pub fn new(data: &'a mut [u8], width: u32, height: u32, stride: u32) ->
        Result<Self, PixmapError> {
        let row_bytes = width.checked_mul(BYTES_PER_PIXEL)
            .ok_or(PixmapError::DimensionsOverflow)?;
        if stride < row_bytes {
            return Err(PixmapError::StrideTooSmall { minimum: row_bytes, actual: stride });
        }
        let (height_usize, stride_usize, row_bytes_usize) = (
            usize::try_from(height).map_err(|_| PixmapError::DimensionsOverflow)?,
            usize::try_from(stride).map_err(|_| PixmapError::DimensionsOverflow)?,
            usize::try_from(row_bytes).map_err(|_| PixmapError::DimensionsOverflow)?,
        );
        let minimum = if height_usize == 0 { 0 } else {
            stride_usize.checked_mul(height_usize - 1)
                .and_then(|offset| offset.checked_add(row_bytes_usize))
                .ok_or(PixmapError::DimensionsOverflow)?
        };
        if data.len() < minimum {
            return Err(PixmapError::BufferTooSmall { minimum, actual: data.len() });
        }   Ok(Self { data, width, height, stride })
    }

    pub fn  width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn stride(&self) -> u32 { self.stride }

    pub fn pixel(&self, x: u32, y: u32) -> Option<PremulRGBA<u8>> {
        if x >= self.width || y >= self.height { return None; }
        let offset = y as usize * self.stride as usize +
                     x as usize * BYTES_PER_PIXEL as usize;
        Some((self.data[offset], self.data[offset + 1],
              self.data[offset + 2], self.data[offset + 3]).into())
    }

    fn blend_solid_span(&mut self, x: u32, y: u32, len: u32,
        color: PremulRGBA<u8>, coverage: u8) {
        let terms = solid_blend_terms(color, coverage);
        let start = y as usize * self.stride as usize
            + x as usize * BYTES_PER_PIXEL as usize;
        let end = start + len as usize * BYTES_PER_PIXEL as usize;
        blend_solid_bytes(&mut self.data[start..end], terms);
    }

    fn blend_sampled_span<S: PaintSampler>(&mut self, x: u32, y: u32, len: u32,
        sampler: &S, coverage: u8) {
        if let Some(color) = sampler.solid_color() {
            self.blend_solid_span(x, y, len, color, coverage);
            return;
        }
        for pixel_x in x..x + len {
            let color = sampler.sample(pixel_x as f32 + 0.5, y as f32 + 0.5);
            self.blend_solid_span(pixel_x, y, 1, color, coverage);
        }
    }

    #[cfg(feature = "fixed")] fn blend_solid_tile(&mut self, x: u32, y: u32,
        width: u32, height: u32, color: PremulRGBA<u8>) {
        let terms = solid_blend_terms(color, u8::MAX);
        for row in y..y + height {
            let start = row as usize * self.stride as usize
                + x as usize * BYTES_PER_PIXEL as usize;
            let end = start + width as usize * BYTES_PER_PIXEL as usize;
            blend_solid_bytes(&mut self.data[start..end], terms);
        }
    }
}

fn solid_blend_terms(color: PremulRGBA<u8>, coverage: u8) -> ([u8; 3], u8, u8) {
    let mul_div_255 = |a, b| (a as u16 * b as u16 + 127).div_euclid(255) as u8;
    let [r, g, b, a] = color.to_array();
    let alpha = mul_div_255(a, coverage);
    ([mul_div_255(r, coverage), mul_div_255(g, coverage),
      mul_div_255(b, coverage)], alpha, u8::MAX - alpha)
}

fn blend_solid_bytes(bytes: &mut [u8], (source, alpha, inverse): ([u8; 3], u8, u8)) {
    let mul_div_255 = |a, b| (a as u16 * b as u16 + 127).div_euclid(255) as u8;
    for pixel in bytes.chunks_exact_mut(BYTES_PER_PIXEL as _) {
        for (channel, source) in pixel[..3].iter_mut().zip(source) {
            *channel = source.saturating_add(mul_div_255(*channel, inverse));
        }
        pixel[3] = alpha.saturating_add(mul_div_255(pixel[3], inverse));
    }
}

pub struct RenderWorkspace<'a> {
    pub edges: &'a mut [Edge],
    pub intersections: &'a mut [Intersection],
    pub row_coverage: &'a mut [f32],
}

pub struct AnalyticRenderWorkspace<'a> {
    pub intersections: &'a mut [AnalyticIntersection],
    pub  row_coverage: &'a mut [f32],
    pub edges: &'a mut [Edge],
}

#[derive(Clone, Copy, Debug, PartialEq)] pub struct RenderOptions {
    pub fill_rule: FillRule,
    pub flatten: FlattenOptions,
    pub  raster:  RasterOptions,
}

impl Default for RenderOptions { fn default() -> Self {
        Self { fill_rule: FillRule::NonZero,
            flatten: FlattenOptions::default(),
             raster:  RasterOptions::default(),
        }
} }

#[derive(Clone, Copy, Debug, PartialEq)] pub struct AnalyticRenderOptions {
    pub fill_rule: FillRule,
    pub flatten: FlattenOptions,
}

impl Default for AnalyticRenderOptions { fn default() -> Self {
    Self { fill_rule: FillRule::NonZero, flatten: FlattenOptions::default() }
} }

#[derive(Clone, Copy, Debug, PartialEq)] pub enum RenderError {
    InvalidTolerance, InvalidDepth, NonFiniteCoordinate, FlattenDepthLimit,
    DimensionsOverflow, InvalidEdge, InvalidSampleCount, InvalidPath(PathError),
    EdgeCapacity { needed_at_least: usize },
    #[cfg(feature = "fixed")] FixedRaster(FixedRasterError),
    RasterWorkspaceTooSmall { intersections: usize, row_coverage: usize },
    CoverageDimensionsMismatch { coverage: (u32, u32), target: (u32, u32), },
}

/// Renders a solid straight-alpha color through the reference rasterizer.
///
/// The destination is premultiplied RGBA8888. This function performs no
/// allocation; all geometry and raster storage comes from `workspace`.
pub fn render_solid(path: &Path, transform: Affine, color: RGBA<u8>, options: RenderOptions,
    target: &mut PixmapMut<'_>, workspace: &mut RenderWorkspace<'_>) ->
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
pub fn render_solid_clipped(path: &Path, transform: Affine, color: RGBA<u8>,
    clip: Rect, options: RenderOptions, target: &mut PixmapMut<'_>,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError> {
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
pub fn render_solid_analytic(path: &Path, transform: Affine, color: RGBA<u8>,
    options: AnalyticRenderOptions, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticRenderWorkspace<'_>) -> Result<(), RenderError> {
    render_paint_analytic(path, transform, &SolidPaint::new(color), options, target, workspace)
}

/// Renders a statically dispatched paint sampler through the exact-area `f32` rasterizer.
///
/// Samples are evaluated at device-space pixel centers. Coverage and sampled
/// premultiplied colors are then composed source-over the target.
pub fn render_paint_analytic<S: PaintSampler>(path: &Path, transform: Affine, sampler: &S,
    options: AnalyticRenderOptions, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticRenderWorkspace<'_>) -> Result<(), RenderError> {
    let edge_count = build_edges(path, transform, options.flatten, workspace.edges)?;
    let mut compositor = PaintCompositor { target, sampler };
    rasterize_edges_analytic(&workspace.edges[..edge_count], compositor.target.width,
        compositor.target.height, options.fill_rule, &mut AnalyticWorkspace {
            intersections: workspace.intersections,
             row_coverage: workspace.row_coverage,
        }, &mut compositor,
    ).map_err(map_raster_error)
}

/// Renders through the analytic reference rasterizer and an antialiased rectangle clip.
pub fn render_solid_analytic_clipped(path: &Path, transform: Affine, color: RGBA<u8>,
    clip: Rect, options: AnalyticRenderOptions, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticRenderWorkspace<'_>) -> Result<(), RenderError> {
    let edge_count = build_edges(path, transform, options.flatten, workspace.edges)?;
    let paint = SolidPaint::new(color);
    let mut compositor = PaintCompositor { target, sampler: &paint };
    rasterize_edges_analytic(&workspace.edges[..edge_count], compositor.target.width,
        compositor.target.height, options.fill_rule, &mut AnalyticWorkspace {
            intersections: workspace.intersections,
             row_coverage: workspace.row_coverage,
        }, &mut RectClipSink::new(clip, &mut compositor),
    ).map_err(map_raster_error)
}

/// Rasterizes an analytic path clip into caller-owned 8-bit coverage.
///
/// The valid mask area is cleared after flattening succeeds. Callers must
/// discard the mask if this function returns an error.
pub fn rasterize_path_clip_analytic(path: &Path, transform: Affine,
    options: AnalyticRenderOptions, mask: &mut CoverageMaskMut<'_>,
    workspace: &mut AnalyticRenderWorkspace<'_>) -> Result<(), RenderError> {
    let edge_count = build_edges(path, transform, options.flatten, workspace.edges)?;
    mask.clear();
    rasterize_edges_analytic(&workspace.edges[..edge_count], mask.width(), mask.height(),
        options.fill_rule, &mut AnalyticWorkspace {
            intersections: workspace.intersections,
             row_coverage: workspace.row_coverage,
        }, mask,
    ).map_err(map_raster_error)
}

/// Renders analytic solid coverage multiplied by a borrowed path clip mask.
pub fn render_solid_analytic_masked(path: &Path, transform: Affine, color: RGBA<u8>,
    mask: CoverageMask<'_>, options: AnalyticRenderOptions, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticRenderWorkspace<'_>) -> Result<(), RenderError> {
    if (mask.width(), mask.height()) != (target.width, target.height) {
        return Err(RenderError::CoverageDimensionsMismatch {
            coverage: (mask.width(), mask.height()), target: (target.width, target.height),
        });
    }
    let edge_count = build_edges(path, transform, options.flatten, workspace.edges)?;
    let paint = SolidPaint::new(color);
    let mut compositor = PaintCompositor { target, sampler: &paint };
    rasterize_edges_analytic(&workspace.edges[..edge_count], compositor.target.width,
        compositor.target.height, options.fill_rule, &mut AnalyticWorkspace {
            intersections: workspace.intersections,
             row_coverage: workspace.row_coverage,
        }, &mut MaskClipSink::new(mask, &mut compositor),
    ).map_err(map_raster_error)
}

/// Renders prepared Q24.8 lines through the allocation-free fixed backend.
#[cfg(feature = "fixed")] pub fn render_solid_fixed(lines: &[FixedLine],
    color: RGBA<u8>, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let paint = SolidPaint::new(color);
    let mut compositor = PaintCompositor { target, sampler: &paint };
    rasterize_lines(lines, compositor.target.width, compositor.target.height,
        fill_rule, workspace, &mut compositor).map_err(map_fixed_render_error)
}

#[cfg(feature = "fixed")]
/// Renders prepared Q24.8 lines through direct sparse tiles.
pub fn render_solid_fixed_tiled(lines: &[FixedLine], color: RGBA<u8>, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, raster_workspace: &mut FixedRasterWorkspace<'_>,
    tile_workspace: FixedDirectTileWorkspace<'_, '_>) -> Result<(), RenderError> {
    let tiled = rasterize_lines_to_tiles(lines, target.width, target.height, fill_rule,
        raster_workspace, tile_workspace).map_err(RenderError::FixedRaster)?;
    composite_solid_fixed_tiles(tiled, color, target)
}

/// Composites retained fixed coverage without rasterizing its geometry again.
#[cfg(feature = "fixed")] pub fn composite_solid_fixed_tiles(tiled: FixedCoverageTiles<'_>,
    color: RGBA<u8>, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    if (tiled.width(), tiled.height()) != (target.width, target.height) {
        return Err(RenderError::CoverageDimensionsMismatch {
            coverage: (tiled.width(), tiled.height()),
            target: (target.width, target.height),
        });
    }
    let paint = SolidPaint::new(color);
    let compositor = PaintCompositor { target, sampler: &paint };
    for tile in tiled.tiles() {
        match tile.kind {
            FixedTileKind::Full => {
                let (width, height) = tiled.tile_extent(*tile);
                compositor.target.blend_solid_tile(
                    tile.x, tile.y, width, height, paint.color());
            }
            FixedTileKind::Boundary => {
                let start = tile.run_start as usize;
                for run in &tiled.runs()[start..start + tile.run_count as usize] {
                    compositor.target.blend_solid_span(tile.x + run.x as u32,
                        tile.y + run.row as u32, run.len as _, paint.color(), run.coverage);
                }
            }
        }
    }   Ok(())
}

fn build_edges(path: &Path, transform: Affine, options: FlattenOptions, edges: &mut [Edge]) ->
    Result<usize, RenderError> {
    let mut sink = EdgeSliceSink { edges, len: 0 };
    build_fill_edges(path, transform, options, &mut sink).map_err(map_flatten_error)?;
    Ok(sink.len)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] struct EdgeCapacity { needed_at_least: usize }

struct EdgeSliceSink<'a> { edges: &'a mut [Edge], len: usize }

impl EdgeSink for EdgeSliceSink<'_> {
    fn edge(&mut self, edge: Edge) -> Result<(), Self::Error> {
        let slot = self.edges.get_mut(self.len)
            .ok_or(EdgeCapacity { needed_at_least: self.len + 1 })?;
        *slot = edge;   self.len += 1;  Ok(())
    }   type Error = EdgeCapacity;
}

struct PaintCompositor<'a, 'b, S> {
    target: &'a mut PixmapMut<'b>, sampler: &'a S,
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

fn map_raster_error(error: RasterError<Infallible>) -> RenderError {
    match error {
        RasterError::DimensionsOverflow => RenderError::DimensionsOverflow,
        RasterError::InvalidEdge => RenderError::InvalidEdge,
        RasterError::InvalidSampleCount => RenderError::InvalidSampleCount,
        RasterError::WorkspaceTooSmall { intersections, row_coverage } =>
            RenderError::RasterWorkspaceTooSmall { intersections, row_coverage },
        RasterError::Sink(error) => match error {},
    }
}

#[cfg(feature = "fixed")]
fn map_fixed_render_error(error: FixedRenderError<Infallible>) -> RenderError {
    match error {
        FixedRenderError::Raster(error) => RenderError::FixedRaster(error),
        FixedRenderError::Sink(error) => match error {},
    }
}

#[cfg(test)] mod tests { use super::*;
    use crate::{color::RGBA, edge::Edge, raster::Intersection,
        analytic::AnalyticIntersection, geometry::{Affine, PathBuilder}};
    use alloc::vec;

    #[test] fn pixmap_validates_stride_and_preserves_padding() {
        let mut data = [0_u8; 11];
        assert_eq!(PixmapMut::new(&mut data, 2, 1, 7).unwrap_err(),
            PixmapError::StrideTooSmall { minimum: 8, actual: 7 });
        let mut target = PixmapMut::new(&mut data, 2, 1, 11).unwrap();
        target.blend_solid_span(0, 0, 2, RGBA::<u8>::red().premul(), 255);
        assert_eq!(target.pixel(1, 0), Some(RGBA::<u8>::red().premul()));
        assert_eq!(&target.data[8..], &[0, 0, 0]);
    }

    #[test] fn source_over_combines_coverage_alpha_and_premultiplied_destination() {
        let mut data = [0, 0, 255, 255];
        let mut target = PixmapMut::new(&mut data, 1, 1, 4).unwrap();
        target.blend_solid_span(0, 0, 1, RGBA::<u8>::new(255, 0, 0, 128).premul(), 255);
        assert_eq!(target.pixel(0, 0), Some((128, 0, 127, 255).into()));
        let before = target.pixel(0, 0);
        target.blend_solid_span(0, 0, 1, RGBA::<u8>::new(1, 2, 3, 0).premul(), 255);
        assert_eq!(target.pixel(0, 0), before);
    }

    #[test] fn solid_rectangle_renders_end_to_end_without_allocation() {
        let mut builder = PathBuilder::new();
        builder.move_to((1.0, 1.0)).line_to((3.0, 1.0))
               .line_to((3.0, 3.0)).line_to((1.0, 3.0));
        let mut pixels = vec![0; 4 * 4 * 4];
        let mut target = PixmapMut::new(&mut pixels, 4, 4, 16).unwrap();
        let (mut edges, mut intersections, mut row_coverage) = (
            [Edge::default(); 4], [Intersection::default(); 4], [0.0; 4]);
        render_solid(&builder.build(), Affine::identity(), RGBA::new(255, 0, 0, 128),
            RenderOptions::default(), &mut target, &mut RenderWorkspace {
                edges: &mut edges, intersections: &mut intersections,
                row_coverage: &mut row_coverage,
            },
        ).unwrap();
        assert_eq!(target.pixel(0, 0), Some(PremulRGBA::zeroed()));
        assert_eq!(target.pixel(1, 1), Some((128, 0, 0, 128).into()));
        assert_eq!(target.pixel(2, 2), Some((128, 0, 0, 128).into()));
        assert_eq!(target.pixel(3, 3), Some(PremulRGBA::zeroed()));
    }

    #[test] fn edge_capacity_failure_reports_required_lower_bound() {
        let (mut builder, mut pixels) = (PathBuilder::new(), [0; 16]);
        builder.move_to((0.0, 0.0)).line_to((1.0, 1.0)).line_to((2.0, 0.0));
        let mut target = PixmapMut::new(&mut pixels, 2, 2, 8).unwrap();
        let (mut edges, mut intersections, mut row_coverage) = (
            [Edge::default(); 1], [Intersection::default(); 2], [0.0; 2]);
        let result = render_solid(&builder.build(), Affine::identity(), RGBA::white(), RenderOptions::default(), &mut target, &mut RenderWorkspace {
                edges: &mut edges, intersections: &mut intersections,
                row_coverage: &mut row_coverage,
            },
        );
        assert_eq!(result, Err(RenderError::EdgeCapacity { needed_at_least: 2 }));
    }

    #[test] fn analytic_solid_rendering_uses_the_shared_compositor() {
        let (mut builder, mut pixels) = (PathBuilder::new(), [0; 4]);
        builder.move_to((0.0, 0.0)).line_to((1.0, 0.0)).line_to((0.0, 1.0));
        let mut target = PixmapMut::new(&mut pixels, 1, 1, 4).unwrap();
        let (mut edges, mut intersections, mut row_coverage) = (
            [Edge::default(); 2], [AnalyticIntersection::default(); 2], [0.0]);
        render_solid_analytic(&builder.build(), Affine::identity(), RGBA::white(),
            AnalyticRenderOptions::default(), &mut target, &mut AnalyticRenderWorkspace {
                edges: &mut edges, intersections: &mut intersections,
                row_coverage: &mut row_coverage,
            },
        ).unwrap();
        assert_eq!(target.pixel(0, 0), Some((128, 128, 128, 128).into()));
    }

    #[test] fn analytic_sampled_paint_uses_device_pixel_centers_and_coverage() {
        struct CoordinatePaint;
        impl PaintSampler for CoordinatePaint {
            fn sample(&self, x: f32, y: f32) -> PremulRGBA<u8> {
                ((x * 40.0) as u8, (y * 40.0) as u8, 0, u8::MAX).into()
            }
        }

        let mut builder = PathBuilder::new();
        builder.move_to((0.5, 0.0)).line_to((2.0, 0.0))
               .line_to((2.0, 1.0)).line_to((0.5, 1.0));
        let mut pixels = [0; 8];
        let mut target = PixmapMut::new(&mut pixels, 2, 1, 8).unwrap();
        let (mut edges, mut intersections, mut row_coverage) = (
            [Edge::default(); 4], [AnalyticIntersection::default(); 4], [0.0; 2]);
        render_paint_analytic(&builder.build(), Affine::identity(), &CoordinatePaint,
            AnalyticRenderOptions::default(), &mut target, &mut AnalyticRenderWorkspace {
                edges: &mut edges, intersections: &mut intersections,
                row_coverage: &mut row_coverage,
            },
        ).unwrap();
        assert_eq!(target.pixel(0, 0), Some((10, 10, 0, 128).into()));
        assert_eq!(target.pixel(1, 0), Some((60, 20, 0, 255).into()));
    }

    #[test] fn analytic_rectangle_clip_multiplies_coverage_end_to_end() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((3.0, 0.0))
               .line_to((3.0, 2.0)).line_to((0.0, 2.0));
        let mut pixels = [0; 3 * 2 * 4];
        let mut target = PixmapMut::new(&mut pixels, 3, 2, 12).unwrap();
        let (mut edges, mut intersections, mut row_coverage) = (
            [Edge::default(); 4], [AnalyticIntersection::default(); 4], [0.0; 3]);
        render_solid_analytic_clipped(&builder.build(), Affine::identity(), RGBA::white(),
            Rect::from_ltrb(0.5, 0.25, 2.5, 1.0).unwrap(),
            AnalyticRenderOptions::default(), &mut target, &mut AnalyticRenderWorkspace {
                edges: &mut edges, intersections: &mut intersections,
                row_coverage: &mut row_coverage,
            }).unwrap();
        assert_eq!((target.pixel(0, 0), target.pixel(1, 0), target.pixel(2, 0)), (
            Some((96, 96, 96, 96).into()), Some((191, 191, 191, 191).into()),
            Some((96, 96, 96, 96).into()),
        ));
        assert_eq!(target.pixel(1, 1), Some(PremulRGBA::zeroed()));
    }

    #[test] fn analytic_path_clip_uses_reusable_caller_owned_coverage() {
        let mut clip_builder = PathBuilder::new();
        clip_builder.move_to((0.5, 0.0)).line_to((1.5, 0.0))
                    .line_to((1.5, 1.0)).line_to((0.5, 1.0));
        let mut shape_builder = PathBuilder::new();
        shape_builder.move_to((0.0, 0.0)).line_to((2.0, 0.0))
                     .line_to((2.0, 1.0)).line_to((0.0, 1.0));
        let (clip, shape) = (clip_builder.build(), shape_builder.build());
        let (mut mask_data, mut pixels) = ([17; 4], [0; 8]);
        let (mut edges, mut intersections, mut row_coverage) = (
            [Edge::default(); 4], [AnalyticIntersection::default(); 4], [0.0; 2]);
        let mut workspace = AnalyticRenderWorkspace {
            edges: &mut edges, intersections: &mut intersections,
            row_coverage: &mut row_coverage,
        };
        {
            let mut mask = CoverageMaskMut::new(&mut mask_data, 2, 1, 4).unwrap();
            rasterize_path_clip_analytic(&clip, Affine::identity(),
                AnalyticRenderOptions::default(), &mut mask, &mut workspace).unwrap();
        }
        assert_eq!(mask_data, [128, 128, 17, 17]);

        let mask = CoverageMask::new(&mask_data, 2, 1, 4).unwrap();
        render_solid_analytic_masked(&shape, Affine::identity(), RGBA::white(), mask,
            AnalyticRenderOptions::default(),
            &mut PixmapMut::new(&mut pixels, 2, 1, 8).unwrap(), &mut workspace).unwrap();
        assert_eq!(pixels, [128; 8]);
    }

    #[cfg(feature = "fixed")]
    #[test] fn fixed_solid_rendering_uses_the_shared_compositor() {
        use crate::{geometry::FixedScalar, raster_fixed::{
            FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid, prepare_lines,
        }, tile_fixed::{ FixedCoverageTile, FixedCoverageTileRun, FixedDirectTilePiece,
            FixedDirectTileWorkspace, rasterize_lines_to_tiles,
        }};

        let fixed = FixedScalar::from_num;
        let edges = [
            Edge { upper: (fixed(0.5), fixed(0.0)).into(),
                   lower: (fixed(0.5), fixed(1.0)).into(), winding:  1 },
            Edge { upper: (fixed(1.5), fixed(0.0)).into(),
                   lower: (fixed(1.5), fixed(1.0)).into(), winding: -1 },
        ];
        let (mut lines, mut segments, mut trapezoids, mut row_area) = (
            [FixedLine::default(); 2], [FixedSegment::default(); 2],
            [FixedTrapezoid::default(); 1], [0; 2],
        );
        let (mut strip_offsets, mut strip_indices) = ([0; 2], [0; 2]);
        prepare_lines(&edges, &mut lines).unwrap();
        let mut pixels = [0; 8];
        let mut target = PixmapMut::new(&mut pixels, 2, 1, 8).unwrap();
        render_solid_fixed(&lines, RGBA::white(), FillRule::NonZero, &mut target,
            &mut FixedRasterWorkspace { segments: &mut segments,
                trapezoids: &mut trapezoids, row_area: &mut row_area,
                strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
            }).unwrap();
        assert_eq!(target.pixel(0, 0), Some((128, 128, 128, 128).into()));
        assert_eq!(target.pixel(1, 0), Some((128, 128, 128, 128).into()));

        let mut tiled_pixels = [0; 8];
        let mut tiled_target = PixmapMut::new(&mut tiled_pixels, 2, 1, 8).unwrap();
        let (mut tiles, mut runs, mut pieces) = (
            [FixedCoverageTile::default(); 1], [FixedCoverageTileRun::default(); 2],
            [FixedDirectTilePiece::default(); 2],
        );
        render_solid_fixed_tiled(&lines, RGBA::white(), FillRule::NonZero,
            &mut tiled_target, &mut FixedRasterWorkspace {
                segments: &mut segments, trapezoids: &mut trapezoids, row_area: &mut row_area,
                strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
            }, FixedDirectTileWorkspace {
                tiles: &mut tiles, runs: &mut runs, pieces: &mut pieces,
                column_heads: &mut [0], column_tails: &mut [0], touched_columns: &mut [0],
            },
        ).unwrap();
        assert_eq!(tiled_pixels, pixels);

        let tiled = rasterize_lines_to_tiles(&lines, 2, 1, FillRule::NonZero,
            &mut FixedRasterWorkspace {
                segments: &mut segments, trapezoids: &mut trapezoids, row_area: &mut row_area,
                strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
            }, FixedDirectTileWorkspace {
                tiles: &mut tiles, runs: &mut runs, pieces: &mut pieces,
                column_heads: &mut [0], column_tails: &mut [0], touched_columns: &mut [0],
            }).unwrap();
        let mut cached_pixels = [0; 8];
        composite_solid_fixed_tiles(tiled, RGBA::white(),
            &mut PixmapMut::new(&mut cached_pixels, 2, 1, 8).unwrap()).unwrap();
        assert_eq!(cached_pixels, pixels);

        let mut mismatched_pixels = [17; 4];
        let error = composite_solid_fixed_tiles(tiled, RGBA::white(),
            &mut PixmapMut::new(&mut mismatched_pixels, 1, 1, 4).unwrap());
        assert_eq!(error, Err(RenderError::CoverageDimensionsMismatch {
            coverage: (2, 1), target: (1, 1),
        }));
        assert_eq!(mismatched_pixels, [17; 4]);
    }

    #[cfg(feature = "fixed")] #[test] fn full_tile_blending_matches_row_spans() {
        let (mut tiled, mut spanned) = ([17; 16 * 16 * 4], [17; 16 * 16 * 4]);
        let color = RGBA::<u8>::new(40, 120, 220, 192).premul();
        PixmapMut::new(&mut tiled, 16, 16, 64).unwrap()
            .blend_solid_tile(0, 0, 16, 16, color);
        let mut target = PixmapMut::new(&mut spanned, 16, 16, 64).unwrap();
        for y in 0..16 { target.blend_solid_span(0, y, 16, color, u8::MAX); }
        assert_eq!(tiled, spanned);
    }
}
