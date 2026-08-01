//! Shared target and error support used when the f32 backend is disabled.

use alloc::vec::Vec;
use crate::{color::PremulSRGBA8, dash::DashError, edge::{Edge, EdgeSink},
    fixed::raster::Error as FixedRasterError, geometry::PathError};

const BYTES_PER_PIXEL: u32 = 4;

#[derive(Debug)] enum PixmapData<'a> { Owned(Vec<u8>), Borrowed(&'a mut [u8]) }

#[derive(Debug)] pub struct Pixmap<'a> {
    data: PixmapData<'a>, width: u32, height: u32, stride: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum PixmapError {
    StrideTooSmall { minimum: u32, actual: u32 },
    BufferTooSmall { minimum: usize, actual: usize }, DimensionsOverflow,
}

impl Pixmap<'static> {
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
    pub fn from_buffer(data: &'a mut [u8], width: u32, height: u32, stride: u32) ->
        Result<Self, PixmapError> {
        let row_bytes = width.checked_mul(BYTES_PER_PIXEL)
            .ok_or(PixmapError::DimensionsOverflow)?;
        if stride < row_bytes {
            return Err(PixmapError::StrideTooSmall { minimum: row_bytes, actual: stride });
        }
        let (height, stride_size, row_bytes) = (
            usize::try_from(height).map_err(|_| PixmapError::DimensionsOverflow)?,
            usize::try_from(stride).map_err(|_| PixmapError::DimensionsOverflow)?,
            usize::try_from(row_bytes).map_err(|_| PixmapError::DimensionsOverflow)?,
        );
        let minimum = if height == 0 { 0 } else {
            stride_size.checked_mul(height - 1).and_then(|offset| offset.checked_add(row_bytes))
                .ok_or(PixmapError::DimensionsOverflow)?
        };
        if data.len() < minimum {
            return Err(PixmapError::BufferTooSmall { minimum, actual: data.len() });
        }
        Ok(Self { data: PixmapData::Borrowed(data), width, height: height as _, stride })
    }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn stride(&self) -> u32 { self.stride }
    pub fn as_bytes(&self) -> &[u8] { match &self.data {
        PixmapData::Owned(data) => data, PixmapData::Borrowed(data) => data,
    } }
    pub fn as_bytes_mut(&mut self) -> &mut [u8] { match &mut self.data {
        PixmapData::Owned(data) => data, PixmapData::Borrowed(data) => data,
    } }
    pub fn pixel_bytes(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height { return None; }
        let offset = y as usize * self.stride as usize + x as usize * 4;
        let data = self.as_bytes();
        Some([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
    }
    pub fn pixel(&self, x: u32, y: u32) -> Option<PremulSRGBA8> {
        PremulSRGBA8::from_array(self.pixel_bytes(x, y)?)
    }
    pub(crate) fn blend_solid_span(&mut self, x: u32, y: u32, len: u32,
        color: PremulSRGBA8, coverage: u8) {
        let start = y as usize * self.stride as usize + x as usize * 4;
        for pixel in self.as_bytes_mut()[start..start + len as usize * 4].chunks_exact_mut(4) {
            blend_pixel(pixel, color, coverage);
        }
    }
    pub(crate) fn blend_solid_tile(&mut self, x: u32, y: u32,
        width: u32, height: u32, color: PremulSRGBA8) {
        for row in y..y + height { self.blend_solid_span(x, row, width, color, u8::MAX); }
    }
}

fn blend_pixel(pixel: &mut [u8], color: PremulSRGBA8, coverage: u8) {
    let mul = |a, b| (a as u16 * b as u16 + 127).div_euclid(255) as u8;
    let [r, g, b, a] = color.to_array();
    let source = [mul(r, coverage), mul(g, coverage), mul(b, coverage)];
    let alpha = mul(a, coverage);
    let inverse = u8::MAX - alpha;
    for (channel, source) in pixel[..3].iter_mut().zip(source) {
        *channel = source.saturating_add(mul(*channel, inverse));
    }
    pixel[3] = alpha.saturating_add(mul(pixel[3], inverse));
}

#[derive(Clone, Copy, Debug, PartialEq)] pub enum RenderError {
    InvalidTolerance, InvalidDepth, NonFiniteCoordinate, FlattenDepthLimit,
    DimensionsOverflow, InvalidEdge, InvalidEdgeBins, InvalidSampleCount,
    InvalidPath(PathError), StrokeIndexOverflow,
    EdgeCapacity { needed_at_least: usize },
    StrokePointCapacity { needed_at_least: usize },
    StrokeContourCapacity { needed_at_least: usize },
    DashPointCapacity { needed_at_least: usize },
    DashContourCapacity { needed_at_least: usize }, DashPrecisionExhausted,
    StrokeArcSegmentLimit { needed: usize, maximum: u16 },
    AnalyticBinOffsetCapacity { required: usize }, AnalyticBinIndexCapacity { required: usize },
    FixedRaster(FixedRasterError),
    RasterWorkspaceTooSmall { intersections: usize, cells: usize },
    CoverageDimensionsMismatch { coverage: (u32, u32), target: (u32, u32) },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EdgeCapacity { pub(crate) needed_at_least: usize }

pub(crate) struct EdgeSliceSink<'a, T = crate::geometry::Scalar> {
    pub(crate) edges: &'a mut [Edge<T>], pub(crate) len: usize,
}

impl<T> EdgeSink<T> for EdgeSliceSink<'_, T> {
    type Error = EdgeCapacity;
    fn edge(&mut self, edge: Edge<T>) -> Result<(), Self::Error> {
        let slot = self.edges.get_mut(self.len)
            .ok_or(EdgeCapacity { needed_at_least: self.len + 1 })?;
        *slot = edge; self.len += 1; Ok(())
    }
}

pub(crate) fn map_dash_error(error: DashError) -> RenderError { match error {
    DashError::NonFinitePoint => RenderError::NonFiniteCoordinate,
    DashError::PrecisionExhausted => RenderError::DashPrecisionExhausted,
    DashError::CoordinateOutOfRange =>
        RenderError::FixedRaster(FixedRasterError::CoordinateOutOfRange),
    DashError::PointCapacity { needed_at_least } =>
        RenderError::DashPointCapacity { needed_at_least },
    DashError::ContourCapacity { needed_at_least } =>
        RenderError::DashContourCapacity { needed_at_least },
    DashError::IndexOverflow => RenderError::StrokeIndexOverflow,
} }

pub(crate) fn validate_coverage_dimensions(width: u32, height: u32,
    target: &Pixmap<'_>) -> Result<(), RenderError> {
    if (width, height) != (target.width, target.height) {
        return Err(RenderError::CoverageDimensionsMismatch {
            coverage: (width, height), target: (target.width, target.height),
        });
    }
    Ok(())
}
