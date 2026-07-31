//! Borrowed pixel targets and the first complete reference rendering path.

use core::convert::Infallible;
use crate::{color::{PremulSRGBA8, PremulRGBA, SRGBA},
    dash::{dash_polyline, DashContour, DashError, DashPattern, DashWorkspace},
    edge::{build_fill_edges, Edge, EdgeSink},
    analytic::{AnalyticBinError, AnalyticBinWorkspace, AnalyticIntersection,
        AnalyticWorkspace, build_analytic_row_bins, rasterize_edges_analytic_binned},
    flatten::{FlattenError, FlattenOptions}, sampler::{PaintSampler, SolidPaint},
    raster::{CoverageMask, CoverageMaskMut, CoverageSink, FillRule, Intersection,
        MaskClipSink, RasterError, RasterOptions, RasterWorkspace, RectClipSink,
        rasterize_edges,
    }, geometry::{Affine, Path, PathError, Point, Rect},
    stroke::{flatten_stroke_path, stroke_polyline, StrokeContour, StrokeExpandError,
        StrokeOptions, StrokePathWorkspace, StrokeWorkspaceError},
};
#[cfg(feature = "fixed")] use crate::raster_fixed::{
    FixedCoverageStrips, FixedLine, FixedRasterError, FixedRasterWorkspace,
    FixedRenderError, prepare_lines, rasterize_lines,
};
#[cfg(feature = "fixed")] use crate::{
    dash::{FixedDashPattern, dash_polyline_fixed},
    edge::build_fill_edges_fixed,
    flatten_fixed::{FixedFlattenError, FixedFlattenOptions},
    geometry::FixedScalar,
    stroke::flatten_stroke_path_fixed,
    stroke_fixed::{FixedStrokeExpandError, FixedStrokeOptions, stroke_polyline_fixed},
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
    /// Creates an encoded-premultiplied sRGBA8 target with explicit row stride.
    ///
    /// Construction validates only layout and capacity; it does not scan pixel
    /// contents. Before compositing over existing contents, callers must ensure
    /// every destination pixel satisfies `RGB <= alpha`. [`Self::pixel`] can
    /// validate individual pixels without changing their bytes.
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

    /// Returns the physical RGBA bytes without interpreting their invariants.
    pub fn pixel_bytes(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height { return None; }
        let offset = y as usize * self.stride as usize +
                     x as usize * BYTES_PER_PIXEL as usize;
        Some([self.data[offset], self.data[offset + 1],
              self.data[offset + 2], self.data[offset + 3]])
    }

    /// Returns a validated encoded-premultiplied sRGB pixel.
    ///
    /// `None` indicates either an out-of-bounds coordinate or raw target bytes
    /// that violate the premultiplied `RGB <= alpha` invariant.
    pub fn pixel(&self, x: u32, y: u32) -> Option<PremulSRGBA8> {
        PremulSRGBA8::from_array(self.pixel_bytes(x, y)?)
    }

    pub(crate) fn write_encoded_pixel(&mut self, x: u32, y: u32,
        color: PremulSRGBA8) {
        let offset = y as usize * self.stride as usize +
                     x as usize * BYTES_PER_PIXEL as usize;
        self.data[offset..offset + BYTES_PER_PIXEL as usize]
            .copy_from_slice(&color.to_array());
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
            self.blend_solid_span(x, y, len, color.into_legacy(), coverage);
            return;
        }
        for pixel_x in x..x + len {
            let color = sampler.sample(pixel_x as f32 + 0.5, y as f32 + 0.5);
            self.blend_solid_span(pixel_x, y, 1, color.into_legacy(), coverage);
        }
    }

    #[cfg(feature = "fixed")]
    fn blend_fixed_sampled_span<S: crate::sampler::FixedPaintSampler>(
        &mut self, x: u32, y: u32, len: u32, sampler: &S, coverage: u8) {
        if let Some(color) = sampler.solid_color_fixed() {
            self.blend_solid_span(x, y, len, color.into_legacy(), coverage);
            return;
        }
        for pixel_x in x..x + len {
            let color = sampler.sample_fixed(pixel_x, y);
            self.blend_solid_span(pixel_x, y, 1, color.into_legacy(), coverage);
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
    pub  row_coverage: &'a mut [f32],
}

pub struct AnalyticRenderWorkspace<'a> {
    pub intersections: &'a mut [AnalyticIntersection],
    pub  row_coverage: &'a mut [f32],
    pub edges: &'a mut [Edge],
    pub row_offsets: &'a mut [u32],
    pub edge_indices: &'a mut [u32],
}

/// Caller-owned storage for the complete analytic stroke pipeline.
pub struct AnalyticStrokeWorkspace<'a> {
    pub points: &'a mut [Point],
    pub  edges: &'a mut [Edge],
    pub contours: &'a mut [StrokeContour],
    pub intersections: &'a mut [AnalyticIntersection],
    pub  row_coverage: &'a mut [f32],
    pub row_offsets: &'a mut [u32],
    pub edge_indices: &'a mut [u32],
}

/// Caller-owned storage for flattened, dashed analytic strokes.
pub struct AnalyticDashedStrokeWorkspace<'a> {
    pub stroke: AnalyticStrokeWorkspace<'a>,
    pub dash_points: &'a mut [Point],
    pub dash_contours: &'a mut [DashContour],
}

#[cfg(feature = "fixed")]
pub struct FixedGeometryWorkspace<'a> {
    pub edges: &'a mut [Edge<FixedScalar>],
    pub lines: &'a mut [FixedLine],
}

#[cfg(feature = "fixed")]
pub struct FixedDashedStrokeWorkspace<'a> {
    pub path: StrokePathWorkspace<'a, FixedScalar>,
    pub dash_points: &'a mut [Point<FixedScalar>],
    pub dash_contours: &'a mut [DashContour],
    pub geometry: FixedGeometryWorkspace<'a>,
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

#[cfg(feature = "fixed")]
#[derive(Clone, Copy, Debug, PartialEq)] pub struct FixedRenderOptions {
    pub transform: Affine<FixedScalar>,
    pub flatten: FixedFlattenOptions,
    pub fill_rule: FillRule,
}

#[cfg(feature = "fixed")] impl Default for FixedRenderOptions { fn default() -> Self {
    Self { transform: Affine::identity(), flatten: FixedFlattenOptions::default(),
        fill_rule: FillRule::NonZero }
} }

#[cfg(feature = "fixed")]
#[derive(Clone, Copy, Debug, Default, PartialEq)] pub struct FixedStrokePathOptions {
    pub transform: Affine<FixedScalar>,
    pub flatten: FixedFlattenOptions,
    pub stroke: FixedStrokeOptions,
}

#[cfg(feature = "fixed")]
#[derive(Clone, Copy, Debug)] pub struct FixedDashedStrokePathOptions<'a> {
    pub path: FixedStrokePathOptions,
    pub dash: FixedDashPattern<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq)] pub struct AnalyticRenderOptions {
    pub fill_rule: FillRule, pub flatten: FlattenOptions,
}

impl Default for AnalyticRenderOptions { fn default() -> Self {
    Self { fill_rule: FillRule::NonZero, flatten: FlattenOptions::default() }
} }

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AnalyticStrokeOptions {
    pub flatten: FlattenOptions,
    pub stroke: StrokeOptions,
}

#[derive(Clone, Copy, Debug)] pub struct AnalyticDashedStrokeOptions<'a> {
    pub flatten: FlattenOptions,
    pub stroke: StrokeOptions,
    pub dash: DashPattern<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq)] pub enum RenderError {
    InvalidTolerance, InvalidDepth, NonFiniteCoordinate, FlattenDepthLimit,
    DimensionsOverflow, InvalidEdge, InvalidEdgeBins, InvalidSampleCount, InvalidPath(PathError),
    StrokeIndexOverflow, EdgeCapacity { needed_at_least: usize },
    StrokePointCapacity { needed_at_least: usize },
    StrokeContourCapacity { needed_at_least: usize },
    DashPointCapacity { needed_at_least: usize },
    DashContourCapacity { needed_at_least: usize },
    DashPrecisionExhausted,
    StrokeArcSegmentLimit { needed: usize, maximum: u16 },
    AnalyticBinOffsetCapacity { required: usize },
    AnalyticBinIndexCapacity { required: usize },
    #[cfg(feature = "fixed")] FixedRaster(FixedRasterError),
    RasterWorkspaceTooSmall { intersections: usize, row_coverage: usize },
    CoverageDimensionsMismatch { coverage: (u32, u32), target: (u32, u32), },
}

/// Renders a solid straight-alpha color through the reference rasterizer.
///
/// The destination is premultiplied RGBA8888. This function performs no
/// allocation; all geometry and raster storage comes from `workspace`.
pub fn render_solid(path: &Path, transform: Affine, color: SRGBA<u8>, options: RenderOptions,
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
pub fn render_solid_clipped(path: &Path, transform: Affine, color: SRGBA<u8>,
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
pub fn render_solid_analytic(path: &Path, transform: Affine, color: SRGBA<u8>,
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
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_path_analytic_to(path, transform, options, width, height,
        &mut compositor, workspace)
}

/// Renders a solid analytic stroke without allocating intermediate geometry.
pub fn render_stroke_solid_analytic(path: &Path, transform: Affine, color: SRGBA<u8>,
    options: AnalyticStrokeOptions, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticStrokeWorkspace<'_>) -> Result<(), RenderError> {
    render_stroke_paint_analytic(
        path, transform, &SolidPaint::new(color), options, target, workspace)
}

/// Renders a sampled analytic stroke through the shared paint compositor.
pub fn render_stroke_paint_analytic<S: PaintSampler>(path: &Path, transform: Affine,
    sampler: &S, options: AnalyticStrokeOptions, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticStrokeWorkspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_stroke_analytic_to(path, transform, options, width, height,
        &mut compositor, workspace)
}

/// Renders a dashed analytic stroke without allocating intermediate geometry.
pub fn render_stroke_solid_analytic_dashed(path: &Path, transform: Affine,
    color: SRGBA<u8>, options: AnalyticDashedStrokeOptions<'_>,
    target: &mut PixmapMut<'_>, workspace: &mut AnalyticDashedStrokeWorkspace<'_>) ->
    Result<(), RenderError> {
    render_stroke_paint_analytic_dashed(path, transform, &SolidPaint::new(color),
        options, target, workspace)
}

pub fn render_stroke_paint_analytic_dashed<S: PaintSampler>(path: &Path,
    transform: Affine, sampler: &S, options: AnalyticDashedStrokeOptions<'_>,
    target: &mut PixmapMut<'_>, workspace: &mut AnalyticDashedStrokeWorkspace<'_>) ->
    Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_stroke_analytic_dashed_to(path, transform, options, width, height,
        &mut compositor, workspace)
}

pub fn render_stroke_paint_analytic_dashed_clipped<S: PaintSampler>(path: &Path,
    transform: Affine, sampler: &S, clip: Rect, options: AnalyticDashedStrokeOptions<'_>,
    target: &mut PixmapMut<'_>, workspace: &mut AnalyticDashedStrokeWorkspace<'_>) ->
    Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_stroke_analytic_dashed_to(path, transform, options, width, height,
        &mut RectClipSink::new(clip, &mut compositor), workspace)
}

pub fn render_stroke_paint_analytic_dashed_masked<S: PaintSampler>(path: &Path,
    transform: Affine, sampler: &S, mask: CoverageMask<'_>,
    options: AnalyticDashedStrokeOptions<'_>, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticDashedStrokeWorkspace<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_stroke_analytic_dashed_to(path, transform, options, width, height,
        &mut MaskClipSink::new(mask, &mut compositor), workspace)
}

/// Renders a solid analytic stroke through an antialiased rectangle clip.
pub fn render_stroke_solid_analytic_clipped(path: &Path, transform: Affine, color: SRGBA<u8>,
    clip: Rect, options: AnalyticStrokeOptions, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticStrokeWorkspace<'_>) -> Result<(), RenderError> {
    render_stroke_paint_analytic_clipped(path, transform,
        &SolidPaint::new(color), clip, options, target, workspace)
}

/// Renders analytic stroke paint through an antialiased rectangle clip.
pub fn render_stroke_paint_analytic_clipped<S: PaintSampler>(path: &Path, transform: Affine,
    sampler: &S, clip: Rect, options: AnalyticStrokeOptions, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticStrokeWorkspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_stroke_analytic_to(path, transform, options, width, height,
        &mut RectClipSink::new(clip, &mut compositor), workspace)
}

/// Renders a solid analytic stroke multiplied by a borrowed path clip mask.
pub fn render_stroke_solid_analytic_masked(path: &Path, transform: Affine, color: SRGBA<u8>,
    mask: CoverageMask<'_>, options: AnalyticStrokeOptions, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticStrokeWorkspace<'_>) -> Result<(), RenderError> {
    render_stroke_paint_analytic_masked(
        path, transform, &SolidPaint::new(color), mask, options, target, workspace)
}

/// Renders analytic stroke paint multiplied by a borrowed path clip mask.
pub fn render_stroke_paint_analytic_masked<S: PaintSampler>(path: &Path, transform: Affine,
    sampler: &S, mask: CoverageMask<'_>, options: AnalyticStrokeOptions,
    target: &mut PixmapMut<'_>, workspace: &mut AnalyticStrokeWorkspace<'_>) ->
    Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_stroke_analytic_to(path, transform, options, width, height,
        &mut MaskClipSink::new(mask, &mut compositor), workspace)
}

/// Renders through the analytic reference rasterizer and an antialiased rectangle clip.
pub fn render_solid_analytic_clipped(path: &Path, transform: Affine, color: SRGBA<u8>,
    clip: Rect, options: AnalyticRenderOptions, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticRenderWorkspace<'_>) -> Result<(), RenderError> {
    render_paint_analytic_clipped(path, transform,
        &SolidPaint::new(color), clip, options, target, workspace)
}

/// Renders an analytic paint through an antialiased rectangle clip.
pub fn render_paint_analytic_clipped<S: PaintSampler>(path: &Path, transform: Affine,
    sampler: &S, clip: Rect, options: AnalyticRenderOptions, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticRenderWorkspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_path_analytic_to(path, transform, options, width, height,
        &mut RectClipSink::new(clip, &mut compositor), workspace)
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
    rasterize_analytic(&workspace.edges[..edge_count], mask.width(), mask.height(),
        options.fill_rule, AnalyticWorkspace {
            intersections: workspace.intersections, row_coverage: workspace.row_coverage,
        }, AnalyticBinWorkspace {
            row_offsets: workspace.row_offsets, edge_indices: workspace.edge_indices,
        }, mask)
}

/// Rasterizes a Q24.8 path clip into caller-owned 8-bit coverage without an FPU.
///
/// The valid mask area is cleared after path flattening and line preparation
/// succeed. Callers must discard the mask if this function returns an error.
#[cfg(feature = "fixed")] pub fn rasterize_path_clip_fixed(
    path: &Path<FixedScalar>, options: FixedRenderOptions,
    mask: &mut CoverageMaskMut<'_>, geometry: &mut FixedGeometryWorkspace<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let mut sink = EdgeSliceSink { edges: geometry.edges, len: 0 };
    build_fill_edges_fixed(path, options.transform, options.flatten, &mut sink)
        .map_err(map_fixed_flatten_error)?;
    let line_count = prepare_lines(&sink.edges[..sink.len], geometry.lines)
        .map_err(RenderError::FixedRaster)?;
    mask.clear();
    rasterize_lines(&geometry.lines[..line_count], mask.width(), mask.height(),
        options.fill_rule, raster_workspace, mask).map_err(map_fixed_render_error)
}

/// Renders analytic solid coverage multiplied by a borrowed path clip mask.
pub fn render_solid_analytic_masked(path: &Path, transform: Affine, color: SRGBA<u8>,
    mask: CoverageMask<'_>, options: AnalyticRenderOptions, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticRenderWorkspace<'_>) -> Result<(), RenderError> {
    render_paint_analytic_masked(path, transform, &SolidPaint::new(color), mask,
        options, target, workspace)
}

/// Renders analytic paint coverage multiplied by a borrowed path clip mask.
pub fn render_paint_analytic_masked<S: PaintSampler>(path: &Path, transform: Affine,
    sampler: &S, mask: CoverageMask<'_>, options: AnalyticRenderOptions,
    target: &mut PixmapMut<'_>, workspace: &mut AnalyticRenderWorkspace<'_>) ->
    Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_path_analytic_to(path, transform, options, width, height,
        &mut MaskClipSink::new(mask, &mut compositor), workspace)
}

/// Renders prepared Q24.8 lines through the allocation-free fixed backend.
#[cfg(feature = "fixed")] pub fn render_solid_fixed(lines: &[FixedLine],
    color: SRGBA<u8>, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    render_paint_fixed(lines, &SolidPaint::new(color), fill_rule, target, workspace)
}

/// Renders prepared Q24.8 lines through the shared encoded paint compositor.
///
/// Raster geometry and coverage are fixed-point; the supplied sampler retains
/// its own numeric contract and may use floating point.
#[cfg(feature = "fixed")] pub fn render_paint_fixed<S: PaintSampler>(lines: &[FixedLine],
    sampler: &S, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let mut compositor = PaintCompositor { target, sampler };
    rasterize_lines(lines, compositor.target.width, compositor.target.height,
        fill_rule, workspace, &mut compositor).map_err(map_fixed_render_error)
}

/// Renders fixed coverage and solid paint through an antialiased rectangle clip.
#[cfg(feature = "fixed")] pub fn render_solid_fixed_clipped(lines: &[FixedLine],
    color: SRGBA<u8>, clip: Rect, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    render_paint_fixed_clipped(
        lines, &SolidPaint::new(color), clip, fill_rule, target, workspace)
}

/// Renders fixed coverage and sampled paint through an antialiased rectangle clip.
#[cfg(feature = "fixed")] pub fn render_paint_fixed_clipped<S: PaintSampler>(
    lines: &[FixedLine], sampler: &S, clip: Rect, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, workspace: &mut FixedRasterWorkspace<'_>) ->
    Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    rasterize_lines(lines, width, height, fill_rule, workspace,
        &mut RectClipSink::new(clip, &mut compositor)).map_err(map_fixed_render_error)
}

/// Renders fixed coverage and solid paint multiplied by a borrowed path mask.
#[cfg(feature = "fixed")] pub fn render_solid_fixed_masked(lines: &[FixedLine],
    color: SRGBA<u8>, mask: CoverageMask<'_>, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, workspace: &mut FixedRasterWorkspace<'_>) ->
    Result<(), RenderError> {
    render_paint_fixed_masked(
        lines, &SolidPaint::new(color), mask, fill_rule, target, workspace)
}

/// Renders fixed coverage and sampled paint multiplied by a borrowed path mask.
#[cfg(feature = "fixed")] pub fn render_paint_fixed_masked<S: PaintSampler>(
    lines: &[FixedLine], sampler: &S, mask: CoverageMask<'_>, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, workspace: &mut FixedRasterWorkspace<'_>) ->
    Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    rasterize_lines(lines, width, height, fill_rule, workspace,
        &mut MaskClipSink::new(mask, &mut compositor)).map_err(map_fixed_render_error)
}

/// Renders prepared Q24.8 lines with a no-FPU fixed paint sampler.
#[cfg(feature = "fixed")] pub fn render_native_paint_fixed<
    S: crate::sampler::FixedPaintSampler>(lines: &[FixedLine], sampler: &S,
    fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let mut compositor = FixedPaintCompositor { target, sampler };
    rasterize_lines(lines, compositor.target.width, compositor.target.height,
        fill_rule, workspace, &mut compositor).map_err(map_fixed_render_error)
}

/// Transforms, flattens, and fills a Q24.8 path without floating-point operations.
#[cfg(feature = "fixed")] pub fn render_native_path_fixed<
    S: crate::sampler::FixedPaintSampler>(path: &Path<FixedScalar>,
    sampler: &S, options: FixedRenderOptions,
    target: &mut PixmapMut<'_>, geometry: &mut FixedGeometryWorkspace<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let line_count = prepare_fixed_path(path, options, geometry)?;
    render_native_paint_fixed(&geometry.lines[..line_count], sampler,
        options.fill_rule, target, raster_workspace)
}

/// Transforms, flattens, and fills a Q24.8 path through a rectangle clip.
#[cfg(feature = "fixed")] pub fn render_native_path_fixed_clipped<
    S: crate::sampler::FixedPaintSampler>(path: &Path<FixedScalar>,
    sampler: &S, clip: Rect, options: FixedRenderOptions,
    target: &mut PixmapMut<'_>, geometry: &mut FixedGeometryWorkspace<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let line_count = prepare_fixed_path(path, options, geometry)?;
    render_native_paint_fixed_clipped(&geometry.lines[..line_count], sampler,
        clip, options.fill_rule, target, raster_workspace)
}

/// Transforms, flattens, and fills a Q24.8 path through a coverage mask.
#[cfg(feature = "fixed")] pub fn render_native_path_fixed_masked<
    S: crate::sampler::FixedPaintSampler>(path: &Path<FixedScalar>,
    sampler: &S, mask: CoverageMask<'_>, options: FixedRenderOptions,
    target: &mut PixmapMut<'_>, geometry: &mut FixedGeometryWorkspace<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let line_count = prepare_fixed_path(path, options, geometry)?;
    render_native_paint_fixed_masked(&geometry.lines[..line_count], sampler,
        mask, options.fill_rule, target, raster_workspace)
}

/// Expands and renders a Q24.8 polyline with no floating-point operations.
#[cfg(feature = "fixed")] pub fn render_native_stroke_polyline_fixed<
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
#[cfg(feature = "fixed")] pub fn render_native_stroke_path_fixed<
    S: crate::sampler::FixedPaintSampler>(path: &Path<FixedScalar>, sampler: &S,
    options: FixedStrokePathOptions, target: &mut PixmapMut<'_>,
    path_workspace: &mut StrokePathWorkspace<'_, FixedScalar>,
    geometry: &mut FixedGeometryWorkspace<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let line_count = prepare_fixed_stroke_path(path, options, path_workspace, geometry)?;
    render_native_paint_fixed(&geometry.lines[..line_count], sampler,
        FillRule::NonZero, target, raster_workspace)
}

#[cfg(feature = "fixed")]
fn prepare_fixed_path(path: &Path<FixedScalar>, options: FixedRenderOptions,
    geometry: &mut FixedGeometryWorkspace<'_>) -> Result<usize, RenderError> {
    let mut sink = EdgeSliceSink { edges: geometry.edges, len: 0 };
    build_fill_edges_fixed(path, options.transform, options.flatten, &mut sink)
        .map_err(map_fixed_flatten_error)?;
    prepare_lines(&sink.edges[..sink.len], geometry.lines)
        .map_err(RenderError::FixedRaster)
}

#[cfg(feature = "fixed")]
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
#[cfg(feature = "fixed")] pub fn render_native_stroke_path_dashed_fixed<
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
#[cfg(feature = "fixed")] pub fn render_native_paint_fixed_clipped<
    S: crate::sampler::FixedPaintSampler>(lines: &[FixedLine], sampler: &S,
    clip: Rect, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = FixedPaintCompositor { target, sampler };
    rasterize_lines(lines, width, height, fill_rule, workspace,
        &mut RectClipSink::new(clip, &mut compositor)).map_err(map_fixed_render_error)
}

/// Renders fixed geometry and no-FPU paint through a borrowed path mask.
#[cfg(feature = "fixed")] pub fn render_native_paint_fixed_masked<
    S: crate::sampler::FixedPaintSampler>(lines: &[FixedLine], sampler: &S,
    mask: CoverageMask<'_>, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    workspace: &mut FixedRasterWorkspace<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width, target.height);
    let mut compositor = FixedPaintCompositor { target, sampler };
    rasterize_lines(lines, width, height, fill_rule, workspace,
        &mut MaskClipSink::new(mask, &mut compositor)).map_err(map_fixed_render_error)
}

#[cfg(feature = "fixed")]
/// Renders prepared Q24.8 lines through direct sparse tiles.
pub fn render_solid_fixed_tiled(lines: &[FixedLine], color: SRGBA<u8>, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, raster_workspace: &mut FixedRasterWorkspace<'_>,
    tile_workspace: FixedDirectTileWorkspace<'_, '_>) -> Result<(), RenderError> {
    let tiled = rasterize_lines_to_tiles(lines, target.width, target.height, fill_rule,
        raster_workspace, tile_workspace).map_err(RenderError::FixedRaster)?;
    composite_solid_fixed_tiles(tiled, color, target)
}

/// Renders prepared fixed lines through direct sparse tiles and sampled paint.
#[cfg(feature = "fixed")] pub fn render_paint_fixed_tiled<S: PaintSampler>(
    lines: &[FixedLine], sampler: &S, fill_rule: FillRule, target: &mut PixmapMut<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>,
    tile_workspace: FixedDirectTileWorkspace<'_, '_>) -> Result<(), RenderError> {
    let tiled = rasterize_lines_to_tiles(lines, target.width, target.height, fill_rule,
        raster_workspace, tile_workspace).map_err(RenderError::FixedRaster)?;
    composite_paint_fixed_tiles(tiled, sampler, target)
}

/// Renders prepared fixed lines through direct sparse tiles and no-FPU paint.
#[cfg(feature = "fixed")] pub fn render_native_paint_fixed_tiled<
    S: crate::sampler::FixedPaintSampler>(lines: &[FixedLine], sampler: &S,
    fill_rule: FillRule, target: &mut PixmapMut<'_>,
    raster_workspace: &mut FixedRasterWorkspace<'_>,
    tile_workspace: FixedDirectTileWorkspace<'_, '_>) -> Result<(), RenderError> {
    let tiled = rasterize_lines_to_tiles(lines, target.width, target.height, fill_rule,
        raster_workspace, tile_workspace).map_err(RenderError::FixedRaster)?;
    composite_native_paint_fixed_tiles(tiled, sampler, target)
}

/// Composites retained fixed strips through the shared paint compositor.
#[cfg(feature = "fixed")] pub fn composite_paint_fixed_strips<S: PaintSampler>(
    strips: FixedCoverageStrips<'_>, sampler: &S, target: &mut PixmapMut<'_>) ->
    Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    finish_infallible(strips.replay(&mut PaintCompositor { target, sampler }))
}

/// Composites retained fixed strips with a no-FPU fixed paint sampler.
#[cfg(feature = "fixed")] pub fn composite_native_paint_fixed_strips<
    S: crate::sampler::FixedPaintSampler>(strips: FixedCoverageStrips<'_>,
    sampler: &S, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    finish_infallible(strips.replay(&mut FixedPaintCompositor { target, sampler }))
}

/// Composites retained fixed strips and no-FPU paint through a rectangle clip.
#[cfg(feature = "fixed")] pub fn composite_native_paint_fixed_strips_clipped<
    S: crate::sampler::FixedPaintSampler>(strips: FixedCoverageStrips<'_>,
    sampler: &S, clip: Rect, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    let mut compositor = FixedPaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut RectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed strips and no-FPU paint through a path mask.
#[cfg(feature = "fixed")] pub fn composite_native_paint_fixed_strips_masked<
    S: crate::sampler::FixedPaintSampler>(strips: FixedCoverageStrips<'_>,
    sampler: &S, mask: CoverageMask<'_>,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let mut compositor = FixedPaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut MaskClipSink::new(mask, &mut compositor)))
}

/// Composites retained fixed strips through an antialiased rectangle clip.
#[cfg(feature = "fixed")] pub fn composite_paint_fixed_strips_clipped<S: PaintSampler>(
    strips: FixedCoverageStrips<'_>, sampler: &S, clip: Rect,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut RectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed strips multiplied by a borrowed path mask.
#[cfg(feature = "fixed")] pub fn composite_paint_fixed_strips_masked<S: PaintSampler>(
    strips: FixedCoverageStrips<'_>, sampler: &S, mask: CoverageMask<'_>,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(strips.width(), strips.height(), target)?;
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(strips.replay(&mut MaskClipSink::new(mask, &mut compositor)))
}

/// Composites retained fixed coverage without rasterizing its geometry again.
#[cfg(feature = "fixed")] pub fn composite_solid_fixed_tiles(tiled: FixedCoverageTiles<'_>,
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
#[cfg(feature = "fixed")] pub fn composite_paint_fixed_tiles<S: PaintSampler>(
    tiled: FixedCoverageTiles<'_>, sampler: &S, target: &mut PixmapMut<'_>) ->
    Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(replay_fixed_tiles(tiled, &mut compositor))
}

/// Composites retained fixed tiles with a no-FPU fixed paint sampler.
#[cfg(feature = "fixed")] pub fn composite_native_paint_fixed_tiles<
    S: crate::sampler::FixedPaintSampler>(tiled: FixedCoverageTiles<'_>,
    sampler: &S, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = FixedPaintCompositor { target, sampler };
    finish_infallible(replay_fixed_tiles(tiled, &mut compositor))
}

/// Composites retained fixed tiles and no-FPU paint through a rectangle clip.
#[cfg(feature = "fixed")] pub fn composite_native_paint_fixed_tiles_clipped<
    S: crate::sampler::FixedPaintSampler>(tiled: FixedCoverageTiles<'_>,
    sampler: &S, clip: Rect, target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = FixedPaintCompositor { target, sampler };
    finish_infallible(replay_fixed_tiles(
        tiled, &mut RectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed tiles and no-FPU paint through a path mask.
#[cfg(feature = "fixed")] pub fn composite_native_paint_fixed_tiles_masked<
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
#[cfg(feature = "fixed")] pub fn composite_paint_fixed_tiles_clipped<S: PaintSampler>(
    tiled: FixedCoverageTiles<'_>, sampler: &S, clip: Rect,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(replay_fixed_tiles(
        tiled, &mut RectClipSink::new(clip, &mut compositor)))
}

/// Composites retained fixed tiles multiplied by a borrowed path mask.
#[cfg(feature = "fixed")] pub fn composite_paint_fixed_tiles_masked<S: PaintSampler>(
    tiled: FixedCoverageTiles<'_>, sampler: &S, mask: CoverageMask<'_>,
    target: &mut PixmapMut<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(tiled.width(), tiled.height(), target)?;
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let mut compositor = PaintCompositor { target, sampler };
    finish_infallible(replay_fixed_tiles(
        tiled, &mut MaskClipSink::new(mask, &mut compositor)))
}

#[cfg(feature = "fixed")] fn replay_fixed_tiles<S: CoverageSink>(
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

#[cfg(feature = "fixed")] fn finish_infallible(result: Result<(), Infallible>) ->
    Result<(), RenderError> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => match error {},
    }
}

fn validate_coverage_dimensions(width: u32, height: u32, target: &PixmapMut<'_>) ->
    Result<(), RenderError> {
    if (width, height) != (target.width, target.height) {
        return Err(RenderError::CoverageDimensionsMismatch {
            coverage: (width, height), target: (target.width, target.height),
        });
    }   Ok(())
}

pub(crate) fn build_edges(path: &Path, transform: Affine, options: FlattenOptions,
    edges: &mut [Edge]) ->
    Result<usize, RenderError> {
    let mut sink = EdgeSliceSink { edges, len: 0 };
    build_fill_edges(path, transform, options, &mut sink).map_err(map_flatten_error)?;
    Ok(sink.len)
}

pub(crate) fn build_stroke_edges(path: &Path, transform: Affine,
    options: AnalyticStrokeOptions,
    points: &mut [Point], contours: &mut [StrokeContour], edges: &mut [Edge]) ->
    Result<usize, RenderError> {
    let mut path_workspace = StrokePathWorkspace { points, contours };
    let flattened = flatten_stroke_path(path, transform, options.flatten,
        &mut path_workspace).map_err(map_stroke_flatten_error)?;
    let mut sink = EdgeSliceSink { edges, len: 0 };
    for (points, closed) in flattened.contours() {
        stroke_polyline(points, closed, options.stroke, &mut sink)
            .map_err(map_stroke_expand_error)?;
    }   Ok(sink.len)
}

fn build_dashed_stroke_edges(path: &Path, transform: Affine,
    options: AnalyticDashedStrokeOptions<'_>,
    path_workspace: &mut StrokePathWorkspace<'_>,
    dash_workspace: &mut DashWorkspace<'_>, edges: &mut [Edge]) ->
    Result<usize, RenderError> {
    let flattened = flatten_stroke_path(path, transform, options.flatten,
        path_workspace).map_err(map_stroke_flatten_error)?;
    let mut sink = EdgeSliceSink { edges, len: 0 };
    for (points, closed) in flattened.contours() {
        let dashed = dash_polyline(points, closed, options.dash, dash_workspace)
            .map_err(map_dash_error)?;
        for (points, closed) in dashed.contours() {
            stroke_polyline(points, closed, options.stroke, &mut sink)
                .map_err(map_stroke_expand_error)?;
        }
    }
    Ok(sink.len)
}

pub(crate) fn rasterize_analytic<S>(edges: &[Edge], width: u32, height: u32,
    fill_rule: FillRule,
    mut workspace: AnalyticWorkspace<'_>, bin_workspace: AnalyticBinWorkspace<'_>,
    sink: &mut S) ->
    Result<(), RenderError> where S: CoverageSink<Error = Infallible> {
    let bins = build_analytic_row_bins(edges, height, bin_workspace)
        .map_err(map_analytic_bin_error)?;
    rasterize_edges_analytic_binned(edges, bins, width, height, fill_rule,
        &mut workspace, sink).map_err(map_raster_error)
}

pub(crate) fn render_path_analytic_to<S>(path: &Path, transform: Affine,
    options: AnalyticRenderOptions, width: u32, height: u32, sink: &mut S,
    workspace: &mut AnalyticRenderWorkspace<'_>) ->
    Result<(), RenderError> where S: CoverageSink<Error = Infallible> {
    let edge_count = build_edges(path, transform, options.flatten, workspace.edges)?;
    rasterize_analytic(&workspace.edges[..edge_count], width, height, options.fill_rule,
        AnalyticWorkspace {
            intersections: workspace.intersections, row_coverage: workspace.row_coverage,
        }, AnalyticBinWorkspace {
            row_offsets: workspace.row_offsets, edge_indices: workspace.edge_indices,
        }, sink)
}

pub(crate) fn render_stroke_analytic_to<S>(path: &Path, transform: Affine,
    options: AnalyticStrokeOptions, width: u32, height: u32, sink: &mut S,
    workspace: &mut AnalyticStrokeWorkspace<'_>) ->
    Result<(), RenderError> where S: CoverageSink<Error = Infallible> {
    let AnalyticStrokeWorkspace {
        points, contours, edges, intersections, row_coverage, row_offsets, edge_indices,
    } = workspace;
    let edge_count = build_stroke_edges(path, transform, options, points, contours, edges)?;
    rasterize_analytic(&edges[..edge_count], width, height, FillRule::NonZero,
        AnalyticWorkspace { intersections, row_coverage },
        AnalyticBinWorkspace { row_offsets, edge_indices }, sink)
}

pub(crate) fn render_stroke_analytic_dashed_to<S>(path: &Path, transform: Affine,
    options: AnalyticDashedStrokeOptions<'_>, width: u32, height: u32, sink: &mut S,
    workspace: &mut AnalyticDashedStrokeWorkspace<'_>) ->
    Result<(), RenderError> where S: CoverageSink<Error = Infallible> {
    let AnalyticDashedStrokeWorkspace {
        stroke: AnalyticStrokeWorkspace {
            points, contours, edges, intersections, row_coverage, row_offsets, edge_indices,
        }, dash_points, dash_contours,
    } = workspace;
    let mut path_workspace = StrokePathWorkspace { points, contours };
    let mut dash_workspace = DashWorkspace {
        points: dash_points, contours: dash_contours,
    };
    let edge_count = build_dashed_stroke_edges(path, transform, options,
        &mut path_workspace, &mut dash_workspace, edges)?;
    rasterize_analytic(&edges[..edge_count], width, height, FillRule::NonZero,
        AnalyticWorkspace { intersections, row_coverage },
        AnalyticBinWorkspace { row_offsets, edge_indices }, sink)
}

fn map_analytic_bin_error(error: AnalyticBinError) -> RenderError {
    match error {
        AnalyticBinError::DimensionsOverflow => RenderError::DimensionsOverflow,
        AnalyticBinError::OffsetCapacity { required } =>
            RenderError::AnalyticBinOffsetCapacity { required },
        AnalyticBinError::IndexCapacity { required } =>
            RenderError::AnalyticBinIndexCapacity { required },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] struct EdgeCapacity { needed_at_least: usize }

struct EdgeSliceSink<'a, T = crate::geometry::Scalar> {
    edges: &'a mut [Edge<T>], len: usize,
}

impl<T> EdgeSink<T> for EdgeSliceSink<'_, T> {
    fn edge(&mut self, edge: Edge<T>) -> Result<(), Self::Error> {
        let slot = self.edges.get_mut(self.len)
            .ok_or(EdgeCapacity { needed_at_least: self.len + 1 })?;
        *slot = edge;   self.len += 1;  Ok(())
    }   type Error = EdgeCapacity;
}

#[cfg(feature = "fixed")]
fn map_fixed_stroke_expand_error(error: FixedStrokeExpandError<EdgeCapacity>) -> RenderError {
    match error {
        FixedStrokeExpandError::CoordinateOutOfRange =>
            RenderError::FixedRaster(FixedRasterError::CoordinateOutOfRange),
        FixedStrokeExpandError::Sink(error) =>
            RenderError::EdgeCapacity { needed_at_least: error.needed_at_least },
    }
}

#[cfg(feature = "fixed")]
fn map_fixed_flatten_error(error: FixedFlattenError<EdgeCapacity>) -> RenderError {
    match error {
        FixedFlattenError::NonPositiveTolerance => RenderError::InvalidTolerance,
        FixedFlattenError::InvalidDepth => RenderError::InvalidDepth,
        FixedFlattenError::CoordinateOutOfRange =>
            RenderError::FixedRaster(FixedRasterError::CoordinateOutOfRange),
        FixedFlattenError::DepthLimit => RenderError::FlattenDepthLimit,
        FixedFlattenError::InvalidPath(error) => RenderError::InvalidPath(error),
        FixedFlattenError::Sink(error) =>
            RenderError::EdgeCapacity { needed_at_least: error.needed_at_least },
    }
}

#[cfg(feature = "fixed")]
fn map_fixed_stroke_flatten_error(
    error: FixedFlattenError<StrokeWorkspaceError>) -> RenderError {
    match error {
        FixedFlattenError::NonPositiveTolerance => RenderError::InvalidTolerance,
        FixedFlattenError::InvalidDepth => RenderError::InvalidDepth,
        FixedFlattenError::CoordinateOutOfRange =>
            RenderError::FixedRaster(FixedRasterError::CoordinateOutOfRange),
        FixedFlattenError::DepthLimit => RenderError::FlattenDepthLimit,
        FixedFlattenError::InvalidPath(error) => RenderError::InvalidPath(error),
        FixedFlattenError::Sink(StrokeWorkspaceError::PointCapacity { needed_at_least }) =>
            RenderError::StrokePointCapacity { needed_at_least },
        FixedFlattenError::Sink(StrokeWorkspaceError::ContourCapacity { needed_at_least }) =>
            RenderError::StrokeContourCapacity { needed_at_least },
        FixedFlattenError::Sink(StrokeWorkspaceError::IndexOverflow) =>
            RenderError::StrokeIndexOverflow,
    }
}

struct PaintCompositor<'a, 'b, S> {
    target: &'a mut PixmapMut<'b>, sampler: &'a S,
}

#[cfg(feature = "fixed")] struct FixedPaintCompositor<'a, 'b, S> {
    target: &'a mut PixmapMut<'b>, sampler: &'a S,
}

#[cfg(feature = "fixed")] impl<S: crate::sampler::FixedPaintSampler> CoverageSink
    for FixedPaintCompositor<'_, '_, S> {
    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        self.target.blend_fixed_sampled_span(x, y, len, self.sampler, coverage);
        Ok(())
    }   type Error = Infallible;
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

fn map_dash_error(error: DashError) -> RenderError {
    match error {
        DashError::NonFinitePoint => RenderError::NonFiniteCoordinate,
        DashError::PrecisionExhausted => RenderError::DashPrecisionExhausted,
        #[cfg(feature = "fixed")]
        DashError::CoordinateOutOfRange =>
            RenderError::FixedRaster(FixedRasterError::CoordinateOutOfRange),
        DashError::PointCapacity { needed_at_least } =>
            RenderError::DashPointCapacity { needed_at_least },
        DashError::ContourCapacity { needed_at_least } =>
            RenderError::DashContourCapacity { needed_at_least },
        DashError::IndexOverflow => RenderError::StrokeIndexOverflow,
    }
}

fn map_raster_error(error: RasterError<Infallible>) -> RenderError {
    match error {
        RasterError::DimensionsOverflow => RenderError::DimensionsOverflow,
        RasterError::InvalidEdge => RenderError::InvalidEdge,
        RasterError::InvalidEdgeBins => RenderError::InvalidEdgeBins,
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

#[cfg(test)] #[path = "canvas_tests.rs"] mod tests;
