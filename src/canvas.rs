//! Borrowed pixel targets and the first complete reference rendering path.

use core::convert::Infallible;
use crate::{color::{PremulRGBA, RGBA}, edge::{build_fill_edges, Edge, EdgeSink},
    analytic::{AnalyticIntersection, AnalyticWorkspace, rasterize_edges_analytic},
    flatten::{FlattenError, FlattenOptions}, geometry::{Affine, Path, PathError},
    raster::{CoverageSink, FillRule, Intersection, RasterError, RasterOptions,
        RasterWorkspace, rasterize_edges,
    }
};
#[cfg(feature = "fixed")] use crate::raster_fixed::{
    FixedLine, FixedRasterError, FixedRasterWorkspace, FixedRenderError, rasterize_lines,
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
        }
        Ok(Self { data, width, height, stride })
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
        let mul_div_255 = |a, b| (a as u16 * b as u16 + 127).div_euclid(255) as u8;
        let [r, g, b, a] = color.to_array();
        let source_alpha = mul_div_255(a, coverage);
        let inverse_alpha = u8::MAX - source_alpha;
        let source = [mul_div_255(r, coverage), mul_div_255(g, coverage),
                      mul_div_255(b, coverage)];
        let start = y as usize * self.stride as usize
            + x as usize * BYTES_PER_PIXEL as usize;
        let end = start + len as usize * BYTES_PER_PIXEL as usize;
        for pixel in self.data[start..end].chunks_exact_mut(BYTES_PER_PIXEL as _) {
            for (channel, source) in pixel[..3].iter_mut().zip(source) {
                *channel = source.saturating_add(mul_div_255(*channel, inverse_alpha));
            }
            pixel[3] = source_alpha.saturating_add(mul_div_255(pixel[3], inverse_alpha));
        }
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

#[derive(Clone, Copy, Debug, PartialEq)] pub enum RenderError {
    InvalidTolerance, InvalidDepth, NonFiniteCoordinate, FlattenDepthLimit,
    DimensionsOverflow, InvalidSampleCount, InvalidPath(PathError),
    EdgeCapacity { needed_at_least: usize },
    RasterWorkspaceTooSmall { intersections: usize, row_coverage: usize },
    #[cfg(feature = "fixed")] FixedRaster(FixedRasterError),
}

/// Renders a solid straight-alpha color through the reference rasterizer.
///
/// The destination is premultiplied RGBA8888. This function performs no
/// allocation; all geometry and raster storage comes from `workspace`.
pub fn render_solid(path: &Path, transform: Affine, color: RGBA<u8>, options: RenderOptions,
    target: &mut PixmapMut<'_>, workspace: &mut RenderWorkspace<'_>) ->
    Result<(), RenderError> {
    let edge_count = build_edges(path, transform, options.flatten, workspace.edges)?;
    let mut compositor = SolidCompositor { target, color: color.premul() };
    rasterize_edges(&workspace.edges[..edge_count], compositor.target.width,
        compositor.target.height, options.fill_rule, options.raster, &mut RasterWorkspace {
            intersections: workspace.intersections,
            row_coverage: workspace.row_coverage,
        }, &mut compositor,
    ).map_err(map_raster_error)
}

/// Renders a solid color through the exact-area `f32` rasterizer.
pub fn render_solid_analytic(path: &Path, transform: Affine, color: RGBA<u8>,
    options: RenderOptions, target: &mut PixmapMut<'_>,
    workspace: &mut AnalyticRenderWorkspace<'_>) -> Result<(), RenderError> {
    let edge_count = build_edges(path, transform, options.flatten, workspace.edges)?;
    let mut compositor = SolidCompositor { target, color: color.premul() };
    rasterize_edges_analytic(&workspace.edges[..edge_count], compositor.target.width,
        compositor.target.height, options.fill_rule, &mut AnalyticWorkspace {
            intersections: workspace.intersections,
             row_coverage: workspace.row_coverage,
        }, &mut compositor,
    ).map_err(map_raster_error)
}

/// Renders prepared Q24.8 lines through the allocation-free fixed backend.
#[cfg(feature = "fixed")]
pub fn render_solid_fixed(lines: &[FixedLine], color: RGBA<u8>, fill_rule: FillRule,
    target: &mut PixmapMut<'_>, workspace: &mut FixedRasterWorkspace<'_>) ->
    Result<(), RenderError> {
    let mut compositor = SolidCompositor { target, color: color.premul() };
    rasterize_lines(lines, compositor.target.width, compositor.target.height,
        fill_rule, workspace, &mut compositor).map_err(map_fixed_render_error)
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

struct SolidCompositor<'a, 'b> { target: &'a mut PixmapMut<'b>, color: PremulRGBA<u8> }

impl CoverageSink for SolidCompositor<'_, '_> {
    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        self.target.blend_solid_span(x, y, len, self.color, coverage);  Ok(())
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
        builder.move_to((1.0, 1.0)).line_to((3.0, 1.0)).unwrap()
            .line_to((3.0, 3.0)).unwrap().line_to((1.0, 3.0)).unwrap();
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
        builder.move_to((0.0, 0.0)).line_to((1.0, 1.0)).unwrap()
            .line_to((2.0, 0.0)).unwrap();
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
        builder.move_to((0.0, 0.0)).line_to((1.0, 0.0)).unwrap()
            .line_to((0.0, 1.0)).unwrap();
        let mut target = PixmapMut::new(&mut pixels, 1, 1, 4).unwrap();
        let (mut edges, mut intersections, mut row_coverage) = (
            [Edge::default(); 2], [AnalyticIntersection::default(); 2], [0.0]);
        render_solid_analytic(&builder.build(), Affine::identity(), RGBA::white(),
            RenderOptions::default(), &mut target, &mut AnalyticRenderWorkspace {
                edges: &mut edges, intersections: &mut intersections,
                row_coverage: &mut row_coverage,
            },
        ).unwrap();
        assert_eq!(target.pixel(0, 0), Some((128, 128, 128, 128).into()));
    }

    #[cfg(feature = "fixed")]
    #[test] fn fixed_solid_rendering_uses_the_shared_compositor() {
        use crate::{geometry::{FixedScalar, Point}, raster_fixed::{
            FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid, prepare_lines,
        }};

        let fixed = FixedScalar::from_num;
        let edges = [
            Edge { upper: Point::new(fixed(0.5), fixed(0.0)),
                   lower: Point::new(fixed(0.5), fixed(1.0)), winding:  1 },
            Edge { upper: Point::new(fixed(1.5), fixed(0.0)),
                   lower: Point::new(fixed(1.5), fixed(1.0)), winding: -1 },
        ];
        let (mut lines, mut segments, mut trapezoids, mut row_area) = (
            [FixedLine::default(); 2], [FixedSegment::default(); 2],
            [FixedTrapezoid::default(); 1], [0; 2],
        );
        prepare_lines(&edges, &mut lines).unwrap();
        let mut pixels = [0; 8];
        let mut target = PixmapMut::new(&mut pixels, 2, 1, 8).unwrap();
        render_solid_fixed(&lines, RGBA::white(), FillRule::NonZero, &mut target,
            &mut FixedRasterWorkspace { segments: &mut segments,
                trapezoids: &mut trapezoids, row_area: &mut row_area,
            }).unwrap();
        assert_eq!(target.pixel(0, 0), Some((128, 128, 128, 128).into()));
        assert_eq!(target.pixel(1, 0), Some((128, 128, 128, 128).into()));
    }
}
