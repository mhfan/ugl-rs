//! Pixel targets and allocation-free `f32` rendering paths.
//!
//! The exact-area rasterizer is the production path exposed by the unqualified
//! `render_*` API. The supersampled reference path is explicitly named
//! `render_*_sampled`.

use alloc::vec::Vec;
use core::convert::Infallible;
use crate::{color::{PremulSRGBA8, PremulRGBA, SRGBA},
    dash::{dash_polyline, DashContour, DashError, DashPattern, DashWorkspace},
    edge::{build_fill_edges, Edge, EdgeSink},
    analytic::{BinError as AnalyticBinError, BinWorkspace as AnalyticBinWorkspace,
        Cell as AnalyticCell, CellWorkspace as AnalyticWorkspace,
        Intersection as AnalyticIntersection, build_row_bins, rasterize_edges_cells,
        rasterize_edges_cells_region},
    float::{ceil, floor},
    flatten::{FlattenError, FlattenOptions}, sampler::{PaintSampler, SolidPaint},
    raster::{CoverageMask, CoverageMaskMut, CoverageSink, FillRule, Intersection,
        MaskClipSink, RasterError, RasterOptions, RasterWorkspace, RectClipSink,
        rasterize_edges,
    }, geometry::{Affine, Path, PathError, Point, Rect},
    stroke::{flatten_stroke_path, stroke_polyline, StrokeContour, StrokeExpandError,
        StrokeOptions, StrokePathWorkspace, StrokeWorkspaceError},
};
#[cfg(feature = "fixed")] use crate::fixed::raster::Error as FixedRasterError;

const BYTES_PER_PIXEL: u32 = 4;

#[derive(Debug)] enum PixmapData<'a> { Owned(Vec<u8>), Borrowed(&'a mut [u8]) }

/// Owned or borrowed premultiplied sRGBA8 pixel storage.
///
/// ```
/// use ugl_rs::Pixmap;
///
/// let owned = Pixmap::new(2, 1).unwrap();
/// assert_eq!((owned.stride(), owned.as_bytes().len()), (8, 8));
///
/// let mut bytes = [0; 12];
/// let borrowed = Pixmap::from_buffer(&mut bytes, 2, 1, 12).unwrap();
/// assert_eq!((borrowed.width(), borrowed.height()), (2, 1));
/// ```
#[derive(Debug)] pub struct Pixmap<'a> {
    data: PixmapData<'a>, width: u32, height: u32, stride: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum PixmapError {
    StrideTooSmall { minimum: u32,   actual: u32 },
    BufferTooSmall { minimum: usize, actual: usize },
    DimensionsOverflow,
}

impl Pixmap<'static> {
    /// Creates zero-initialized, tightly packed owned storage.
    pub fn new(width: u32, height: u32) -> Result<Self, PixmapError> {
        let stride = width.checked_mul(BYTES_PER_PIXEL)
            .ok_or(PixmapError::DimensionsOverflow)?;
        let length = usize::try_from(stride).ok().and_then(|stride|
            usize::try_from(height).ok().and_then(|height| stride.checked_mul(height)))
            .ok_or(PixmapError::DimensionsOverflow)?;
        Ok(Self { data: PixmapData::Owned(alloc::vec![0; length]), width, height, stride })
    }
}

impl<'a> Pixmap<'a> {
    /// Borrows an encoded-premultiplied sRGBA8 target with explicit row stride.
    ///
    /// Construction validates only layout and capacity; it does not scan pixel
    /// contents. Before compositing over existing contents, callers must ensure
    /// every destination pixel satisfies `RGB <= alpha`. [`Self::pixel`] can
    /// validate individual pixels without changing their bytes.
    pub fn from_buffer(data: &'a mut [u8], width: u32, height: u32, stride: u32) ->
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
        }   Ok(Self { data: PixmapData::Borrowed(data), width, height, stride })
    }

    pub fn  width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn stride(&self) -> u32 { self.stride }
    pub fn as_bytes(&self) -> &[u8] { match &self.data {
        PixmapData::Owned(data) => data, PixmapData::Borrowed(data) => data,
    } }

    /// Returns mutable physical RGBA bytes.
    ///
    /// Before subsequent compositing, callers must restore the premultiplied
    /// invariant `R, G, B <= A` for every modified pixel.
    pub fn as_bytes_mut(&mut self) -> &mut [u8] { match &mut self.data {
        PixmapData::Owned(data) => data, PixmapData::Borrowed(data) => data,
    } }

    /// Returns the physical RGBA bytes without interpreting their invariants.
    pub fn pixel_bytes(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height { return None; }
        let offset = y as usize * self.stride as usize +
                     x as usize * BYTES_PER_PIXEL as usize;
        let data = self.as_bytes();
        Some([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
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
        self.as_bytes_mut()[offset..offset + BYTES_PER_PIXEL as usize]
            .copy_from_slice(&color.to_array());
    }

    pub(crate) fn blend_solid_span(&mut self, x: u32, y: u32, len: u32,
        color: PremulRGBA<u8>, coverage: u8) {
        let terms = solid_blend_terms(color, coverage);
        let start = y as usize * self.stride as usize
            + x as usize * BYTES_PER_PIXEL as usize;
        let end = start + len as usize * BYTES_PER_PIXEL as usize;
        blend_solid_bytes(&mut self.as_bytes_mut()[start..end], terms);
    }

    fn blend_sampled_span<S: PaintSampler>(&mut self, x: u32, y: u32, len: u32,
        sampler: &S, coverage: u8) {
        if let Some(color) = sampler.solid_color() {
            self.blend_solid_span(x, y, len, color.into_legacy(), coverage);
            return;
        }
        let start = y as usize * self.stride as usize +
                    x as usize * BYTES_PER_PIXEL as usize;
        let end = start + len as usize * BYTES_PER_PIXEL as usize;
        let mut pixels = self.as_bytes_mut()[start..end]
            .chunks_exact_mut(BYTES_PER_PIXEL as _);
        sampler.sample_span(x as f32 + 0.5, y as f32 + 0.5, 1.0, 0.0, len, |color| {
            let pixel = pixels.next().expect("sampler emitted too many span pixels");
            blend_sampled_pixel(pixel, color, coverage);
        });
        debug_assert!(pixels.next().is_none());
    }

    #[cfg(feature = "fixed")] pub(crate) fn blend_solid_tile(&mut self, x: u32, y: u32,
        width: u32, height: u32, color: PremulRGBA<u8>) {
        let terms = solid_blend_terms(color, u8::MAX);
        for row in y..y + height {
            let start = row as usize * self.stride as usize
                + x as usize * BYTES_PER_PIXEL as usize;
            let end = start + width as usize * BYTES_PER_PIXEL as usize;
            blend_solid_bytes(&mut self.as_bytes_mut()[start..end], terms);
        }
    }

}

pub(crate) fn blend_sampled_pixel(pixel: &mut [u8], color: PremulSRGBA8,
    coverage: u8) {
    if coverage == u8::MAX && pixel[3] == 0 {
        pixel.copy_from_slice(&color.to_array());
        return;
    }
    blend_solid_pixel(pixel, solid_blend_terms(color.into_legacy(), coverage));
}

fn blend_solid_pixel(pixel: &mut [u8], (source, alpha, inverse): ([u8; 3], u8, u8)) {
    if pixel[3] == 0 {
        pixel.copy_from_slice(&[source[0], source[1], source[2], alpha]);
        return;
    }
    let mul_div_255 = |a, b| (a as u16 * b as u16 + 127).div_euclid(255) as u8;
    for (channel, source) in pixel[..3].iter_mut().zip(source) {
        *channel = source.saturating_add(mul_div_255(*channel, inverse));
    }
    pixel[3] = alpha.saturating_add(mul_div_255(pixel[3], inverse));
}

fn solid_blend_terms(color: PremulRGBA<u8>, coverage: u8) -> ([u8; 3], u8, u8) {
    let mul_div_255 = |a, b| (a as u16 * b as u16 + 127).div_euclid(255) as u8;
    let [r, g, b, a] = color.to_array();
    let alpha = mul_div_255(a, coverage);
    ([mul_div_255(r, coverage), mul_div_255(g, coverage),
      mul_div_255(b, coverage)], alpha, u8::MAX - alpha)
}

fn blend_solid_bytes(bytes: &mut [u8], (source, alpha, inverse): ([u8; 3], u8, u8)) {
    let source = u32::from_le_bytes([source[0], source[1], source[2], alpha]) as u64;
    let source_pair = source | source << 32;
    let scale_lanes = |lanes: u64| {
        let product = lanes * inverse as u64 + 0x0080_0080_0080_0080;
        (product + ((product >> 8) & 0x00ff_00ff_00ff_00ff)) >> 8 &
            0x00ff_00ff_00ff_00ff
    };
    let mut pairs = bytes.chunks_exact_mut(8);
    for pair in &mut pairs {
        let destination = u64::from_le_bytes(pair.try_into().unwrap());
        let result = if destination == 0 { source_pair } else {
            let rb = scale_lanes(destination & 0x00ff_00ff_00ff_00ff);
            let ag = scale_lanes((destination >> 8) & 0x00ff_00ff_00ff_00ff) << 8;
            source_pair + rb + ag
        };
        pair.copy_from_slice(&result.to_le_bytes());
    }
    let remainder = pairs.into_remainder();
    if remainder.len() == 4 {
        let destination = u32::from_le_bytes(remainder.try_into().unwrap()) as u64;
        let rb = scale_lanes(destination & 0x00ff_00ff);
        let ag = scale_lanes((destination >> 8) & 0x00ff_00ff) << 8;
        remainder.copy_from_slice(&(source + rb + ag).to_le_bytes()[..4]);
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
/// use ugl_rs::{canvas::{RenderOptions, render_requirements},
///     edge::Edge, geometry::{Affine, PathBuilder}};
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
    RasterWorkspaceTooSmall { intersections: usize, cells: usize },
    CoverageDimensionsMismatch { coverage: (u32, u32), target: (u32, u32), },
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
    render_stroke_dashed_to_region(path, transform, options, (width, height),
        clip_region(clip, width, height),
        &mut RectClipSink::new(clip, &mut compositor), workspace)
}

pub fn render_stroke_paint_dashed_masked<S: PaintSampler>(path: &Path,
    transform: Affine, sampler: &S, mask: CoverageMask<'_>,
    options: DashedStrokePathOptions<'_>, target: &mut Pixmap<'_>,
    workspace: &mut DashedStrokeWorkspace<'_>) -> Result<(), RenderError> {
    validate_coverage_dimensions(mask.width(), mask.height(), target)?;
    let (width, height) = (target.width, target.height);
    let mut compositor = PaintCompositor { target, sampler };
    render_stroke_dashed_to(path, transform, options, width, height,
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
    render_stroke_to_region(path, transform, options, (width, height),
        clip_region(clip, width, height),
        &mut RectClipSink::new(clip, &mut compositor), workspace)
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
    render_stroke_to(path, transform, options, width, height,
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
    render_path_to_region(path, transform, options, (width, height),
        clip_region(clip, width, height),
        &mut RectClipSink::new(clip, &mut compositor), workspace)
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
    render_path_to(path, transform, options, width, height,
        &mut MaskClipSink::new(mask, &mut compositor), workspace)
}

pub(crate) fn validate_coverage_dimensions(width: u32, height: u32, target: &Pixmap<'_>) ->
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
    let bins = crate::analytic::bin_requirements(edges, height).map_err(map_bin_error)?;
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

fn clip_region(clip: Rect, width: u32, height: u32) -> (u32, u32, u32, u32) {
    (floor(clip.left()).clamp(0.0, width as _) as _,
     floor(clip.top()).clamp(0.0, height as _) as _,
      ceil(clip.right()).clamp(0.0, width as _) as _,
      ceil(clip.bottom()).clamp(0.0, height as _) as _)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EdgeCapacity { pub(crate) needed_at_least: usize }

pub(crate) struct EdgeSliceSink<'a, T = crate::geometry::Scalar> {
    pub(crate) edges: &'a mut [Edge<T>], pub(crate) len: usize,
}

impl<T> EdgeSink<T> for EdgeSliceSink<'_, T> {
    fn edge(&mut self, edge: Edge<T>) -> Result<(), Self::Error> {
        let slot = self.edges.get_mut(self.len)
            .ok_or(EdgeCapacity { needed_at_least: self.len + 1 })?;
        *slot = edge;   self.len += 1;  Ok(())
    }   type Error = EdgeCapacity;
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

pub(crate) fn map_dash_error(error: DashError) -> RenderError {
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
            RenderError::RasterWorkspaceTooSmall { intersections, cells: row_coverage },
        RasterError::Sink(error) => match error {},
    }
}

#[cfg(test)] #[path = "canvas_tests.rs"] mod tests;
