//! Borrowed pixel targets and the first complete reference rendering path.

use core::convert::Infallible;
use crate::{color::RGBA, edge::{build_fill_edges, Edge, EdgeSink},
    flatten::{FlattenError, FlattenOptions}, geometry::{Affine, Path, PathError},
    raster::{CoverageSink, FillRule, Intersection, RasterError, RasterOptions,
        RasterWorkspace, rasterize_edges,
    }
};

const BYTES_PER_PIXEL: u32 = 4;

#[derive(Debug)] pub struct PixmapMut<'a> {
    width: u32, height: u32, stride: u32,
    data: &'a mut [u8],
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

    pub fn pixel(&self, x: u32, y: u32) -> Option<RGBA<u8>> {
        if x >= self.width || y >= self.height { return None; }
        let offset = y as usize * self.stride as usize
            + x as usize * BYTES_PER_PIXEL as usize;
        Some(RGBA::new(self.data[offset], self.data[offset + 1],
                       self.data[offset + 2], self.data[offset + 3]))
    }

    fn blend_solid_span(&mut self, x: u32, y: u32, len: u32, color: RGBA<u8>, coverage: u8) {
        let source_alpha = mul_div_255(color.a, coverage);
        let inverse_alpha = u8::MAX - source_alpha;
        let source = [mul_div_255(color.r, source_alpha),
                      mul_div_255(color.g, source_alpha),
                      mul_div_255(color.b, source_alpha)];
        let start = y as usize * self.stride as usize
            + x as usize * BYTES_PER_PIXEL as usize;
        let end = start + len as usize * BYTES_PER_PIXEL as usize;
        for pixel in self.data[start..end].chunks_exact_mut(BYTES_PER_PIXEL as usize) {
            for (channel, source) in pixel[..3].iter_mut().zip(source) {
                *channel = source.saturating_add(mul_div_255(*channel, inverse_alpha));
            }
            pixel[3] = source_alpha.saturating_add(mul_div_255(pixel[3], inverse_alpha));
        }
    }
}

fn mul_div_255(a: u8, b: u8) -> u8 {
    (u16::from(a) * u16::from(b) + 127).div_euclid(255) as u8
}

pub struct RenderWorkspace<'a> {
    pub edges: &'a mut [Edge],
    pub intersections: &'a mut [Intersection],
    pub row_coverage: &'a mut [f32],
}

#[derive(Clone, Copy, Debug, PartialEq)] pub struct RenderOptions {
    pub fill_rule: FillRule,
    pub flatten: FlattenOptions,
    pub raster: RasterOptions,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            fill_rule: FillRule::NonZero,
            flatten: FlattenOptions::default(),
            raster: RasterOptions::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)] pub enum RenderError {
    InvalidTolerance, InvalidDepth, NonFiniteCoordinate, FlattenDepthLimit,
    InvalidSampleCount, InvalidPath(PathError),
    EdgeCapacity { needed_at_least: usize },
    RasterWorkspaceTooSmall { intersections: usize, row_coverage: usize },
}

/// Renders a solid straight-alpha color through the reference rasterizer.
///
/// The destination is premultiplied RGBA8888. This function performs no
/// allocation; all geometry and raster storage comes from `workspace`.
pub fn render_solid(path: &Path, transform: Affine, color: RGBA<u8>, options: RenderOptions,
    target: &mut PixmapMut<'_>, workspace: &mut RenderWorkspace<'_>) ->
    Result<(), RenderError> {
    let mut edge_sink = EdgeSliceSink { edges: workspace.edges, len: 0 };
    build_fill_edges(path, transform, options.flatten, &mut edge_sink)
        .map_err(map_flatten_error)?;
    let edge_count = edge_sink.len;

    let mut compositor = SolidCompositor { target, color };
    rasterize_edges(&workspace.edges[..edge_count], compositor.target.width,
        compositor.target.height, options.fill_rule, options.raster, &mut RasterWorkspace {
            intersections: workspace.intersections,
            row_coverage: workspace.row_coverage,
        }, &mut compositor,
    ).map_err(map_raster_error)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] struct EdgeCapacity { needed_at_least: usize }

struct EdgeSliceSink<'a> { edges: &'a mut [Edge], len: usize }

impl EdgeSink for EdgeSliceSink<'_> {
    type Error = EdgeCapacity;
    fn edge(&mut self, edge: Edge) -> Result<(), Self::Error> {
        let slot = self.edges.get_mut(self.len)
            .ok_or(EdgeCapacity { needed_at_least: self.len + 1 })?;
        *slot = edge;
        self.len += 1;
        Ok(())
    }
}

struct SolidCompositor<'a, 'b> { target: &'a mut PixmapMut<'b>, color: RGBA<u8> }

impl CoverageSink for SolidCompositor<'_, '_> {
    type Error = Infallible;
    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        self.target.blend_solid_span(x, y, len, self.color, coverage);
        Ok(())
    }
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
        RasterError::InvalidSampleCount => RenderError::InvalidSampleCount,
        RasterError::WorkspaceTooSmall { intersections, row_coverage } =>
            RenderError::RasterWorkspaceTooSmall { intersections, row_coverage },
        RasterError::Sink(error) => match error {},
    }
}

#[cfg(test)] mod tests {
    use alloc::vec;
    use super::{PixmapError, PixmapMut, RenderError, RenderOptions,
        RenderWorkspace, render_solid };
    use crate::{color::RGBA, edge::Edge, raster::Intersection,
        geometry::{Affine, PathBuilder}};

    #[test] fn pixmap_validates_stride_and_preserves_padding() {
        let mut data = [0_u8; 11];
        assert_eq!(PixmapMut::new(&mut data, 2, 1, 7).unwrap_err(),
            PixmapError::StrideTooSmall { minimum: 8, actual: 7 });
        let mut target = PixmapMut::new(&mut data, 2, 1, 11).unwrap();
        target.blend_solid_span(0, 0, 2, RGBA::red(), 255);
        assert_eq!(target.pixel(1, 0), Some(RGBA::red()));
        assert_eq!(&target.data[8..], &[0, 0, 0]);
    }

    #[test] fn source_over_combines_coverage_alpha_and_premultiplied_destination() {
        let mut data = [0, 0, 255, 255];
        let mut target = PixmapMut::new(&mut data, 1, 1, 4).unwrap();
        target.blend_solid_span(0, 0, 1, RGBA::new(255, 0, 0, 128), 255);
        assert_eq!(target.pixel(0, 0), Some(RGBA::new(128, 0, 127, 255)));
        let before = target.pixel(0, 0);
        target.blend_solid_span(0, 0, 1, RGBA::new(1, 2, 3, 0), 255);
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
        assert_eq!(target.pixel(0, 0), Some(RGBA::zeroed()));
        assert_eq!(target.pixel(1, 1), Some(RGBA::new(128, 0, 0, 128)));
        assert_eq!(target.pixel(2, 2), Some(RGBA::new(128, 0, 0, 128)));
        assert_eq!(target.pixel(3, 3), Some(RGBA::zeroed()));
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
}
