//! Rendering state, target storage, and backend-neutral pipeline support.

use alloc::vec::Vec;
use crate::common::{color::{PremulSRGBA8, SRGBA}, dash::DashError,
    edge::{Edge, EdgeSink}, geometry::{Affine, PathError, Rect},
    raster::{CoverageMask, FillRule}};
#[cfg(feature = "fixed")] use crate::fixed::raster::Error as FixedRasterError;

#[derive(Clone, Copy, Debug, PartialEq)] pub struct SolidPaint {
    encoded: PremulSRGBA8,
    #[cfg(feature = "f32")]
    linear: crate::common::color::LinearPremulRGBA<f32>,
}

impl SolidPaint {
    pub fn new(color: SRGBA<u8>) -> Self { Self {
        encoded: color.premul_encoded(),
        #[cfg(feature = "f32")]
        linear: color.to_linear().premul(),
    } }
    pub fn premultiplied(color: PremulSRGBA8) -> Self { Self {
        encoded: color,
        #[cfg(feature = "f32")]
        linear: color.to_linear(),
    } }
    pub fn color(&self) -> PremulSRGBA8 { self.encoded }
    #[cfg(feature = "f32")]
    pub fn linear_color(&self) -> crate::common::color::LinearPremulRGBA<f32> { self.linear }
}

impl From<SRGBA<u8>> for SolidPaint { fn from(color: SRGBA<u8>) -> Self { Self::new(color) } }

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum GradientError {
    EmptyStops, NonFiniteOffset, OffsetOutOfRange, UnorderedStops,
    RampTooSmall, RampTooLarge, NonFiniteGeometry, CoordinateOutOfRange,
    NegativeRadius, DegenerateGeometry,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SpreadMode { #[default] Pad, Repeat, Reflect }

#[derive(Clone, Copy, Debug)] pub(crate) enum Clip<'a, T = f32> {
    None, Rect(Rect<T>), Mask(CoverageMask<'a>),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DrawState<T, F, S, P> {
    pub(crate) transform: Affine<T>, pub(crate) fill_rule: FillRule,
    pub(crate) flatten: F, pub(crate) stroke: S, pub(crate) paint: P,
    pub(crate) global_alpha: u8,
}

pub(crate) struct GlobalAlphaPaint<'a, S> {
    pub(crate) sampler: &'a S, pub(crate) alpha: u8,
}

impl<'a, S> GlobalAlphaPaint<'a, S> {
    pub(crate) fn new(sampler: &'a S, alpha: u8) -> Self { Self { sampler, alpha } }
}

pub(crate) const BYTES_PER_PIXEL: u32 = 4;

#[derive(Debug)] enum PixmapData<'a> { Owned(Vec<u8>), Borrowed(&'a mut [u8]) }

/// Owned or borrowed premultiplied sRGBA8 pixel storage.
///
/// ```
/// use ugl_rs::common::Pixmap;
///
/// let owned = Pixmap::new(2, 1).unwrap();
/// assert_eq!((owned.stride(), owned.as_bytes().len()), (8, 8));
///
/// let mut bytes = [0; 12];
/// let borrowed = Pixmap::from_buffer(&mut bytes, 2, 1, 12).unwrap();
/// assert_eq!((borrowed.width(), borrowed.height()), (2, 1));
/// ```
#[derive(Debug)] pub struct Pixmap<'a> {
    data: PixmapData<'a>, pub(crate) width: u32, pub(crate) height: u32,
    pub(crate) stride: u32,
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

    pub(crate) fn blend_solid_span(&mut self, x: u32, y: u32, len: u32,
        color: PremulSRGBA8, coverage: u8) {
        let terms = solid_blend_terms(color, coverage);
        let start = y as usize * self.stride as usize
            + x as usize * BYTES_PER_PIXEL as usize;
        let end = start + len as usize * BYTES_PER_PIXEL as usize;
        blend_solid_bytes(&mut self.as_bytes_mut()[start..end], terms);
    }

    #[cfg(feature = "fixed")] pub(crate) fn blend_solid_tile(&mut self, x: u32, y: u32,
        width: u32, height: u32, color: PremulSRGBA8) {
        let terms = solid_blend_terms(color, u8::MAX);
        for row in y..y + height {
            let start = row as usize * self.stride as usize
                + x as usize * BYTES_PER_PIXEL as usize;
            let end = start + width as usize * BYTES_PER_PIXEL as usize;
            blend_solid_bytes(&mut self.as_bytes_mut()[start..end], terms);
        }
    }

}

pub(crate) fn solid_blend_terms(color: PremulSRGBA8, coverage: u8) -> ([u8; 3], u8, u8) {
    let mul_div_255 = |a, b| (a as u16 * b as u16 + 127).div_euclid(255) as u8;
    let [r, g, b, a] = color.to_array();
    let alpha = mul_div_255(a, coverage);
    ([mul_div_255(r, coverage), mul_div_255(g, coverage),
      mul_div_255(b, coverage)], alpha, u8::MAX - alpha)
}

pub(crate) fn blend_solid_bytes(bytes: &mut [u8],
    (source, alpha, inverse): ([u8; 3], u8, u8)) {
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


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EdgeCapacity { pub(crate) needed_at_least: usize }

pub(crate) struct EdgeSliceSink<'a, T = crate::common::geometry::Scalar> {
    pub(crate) edges: &'a mut [Edge<T>], pub(crate) len: usize,
}

impl<T> EdgeSink<T> for EdgeSliceSink<'_, T> {
    fn edge(&mut self, edge: Edge<T>) -> Result<(), Self::Error> {
        let slot = self.edges.get_mut(self.len)
            .ok_or(EdgeCapacity { needed_at_least: self.len + 1 })?;
        *slot = edge;   self.len += 1;  Ok(())
    }   type Error = EdgeCapacity;
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


pub(crate) fn validate_coverage_dimensions(width: u32, height: u32, target: &Pixmap<'_>) ->
    Result<(), RenderError> {
    if (width, height) != (target.width, target.height) {
        return Err(RenderError::CoverageDimensionsMismatch {
            coverage: (width, height), target: (target.width, target.height),
        });
    }   Ok(())
}
