//! Linear-light premultiplied framebuffer and analytic compositing path.
//!
//! Unlike [`crate::canvas::Pixmap`], this target retains `f32` linear-light
//! colors through source-over compositing. Encoding and RGBA8 quantization occur
//! only when [`LinearPixmap::encode_into`] presents into the compatibility
//! framebuffer.

use alloc::vec::Vec;
use core::convert::Infallible;
use crate::{
    canvas::{DashedStrokePathOptions, DashedStrokeWorkspace,
        RenderOptions, RenderWorkspace, StrokePathOptions,
        StrokeWorkspace, Pixmap, RenderError, render_path_to,
        render_stroke_dashed_to, render_stroke_to},
    color::{LinearPremulRGBA, Srgb8Encoder, SRGBA}, geometry::{Affine, Path, Rect},
    raster::{CoverageMask, CoverageSink, MaskClipSink, RectClipSink},
    sampler::{LinearPaintSampler, SolidPaint},
};

pub const LINEAR_DIRTY_TILE_SIZE: u32 = 16;

enum LinearPixmapData<'a> {
    Owned(Vec<LinearPremulRGBA<f32>>), Borrowed(&'a mut [LinearPremulRGBA<f32>]),
}

impl core::fmt::Debug for LinearPixmapData<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_tuple(match self {
            Self::Owned(_) => "Owned", Self::Borrowed(_) => "Borrowed",
        }).field(&self.as_slice().len()).finish()
    }
}

impl LinearPixmapData<'_> {
    fn as_slice(&self) -> &[LinearPremulRGBA<f32>] { match self {
        Self::Owned(data) => data, Self::Borrowed(data) => data,
    } }
    fn as_mut_slice(&mut self) -> &mut [LinearPremulRGBA<f32>] { match self {
        Self::Owned(data) => data, Self::Borrowed(data) => data,
    } }
}

/// Owned or borrowed premultiplied linear-light RGBA `f32` target.
///
/// `stride` is measured in pixels, not bytes. Caller-provided pixels must
/// already satisfy the [`LinearPremulRGBA`] invariant.
///
/// ```
/// use ugl_rs::{canvas_linear::LinearPixmap, color::LinearPremulRGBA};
///
/// let owned = LinearPixmap::new(2, 1).unwrap();
/// assert_eq!((owned.stride(), owned.as_pixels().len()), (2, 2));
///
/// let mut pixels = [LinearPremulRGBA::default(); 3];
/// let borrowed = LinearPixmap::from_buffer(&mut pixels, 2, 1, 3).unwrap();
/// assert_eq!((borrowed.width(), borrowed.height()), (2, 1));
/// ```
#[derive(Debug)] pub struct LinearPixmap<'a> {
    data: LinearPixmapData<'a>, width: u32, height: u32, stride: u32,
    dirty_tiles: Option<&'a mut [u64]>, dirty_tile_columns: u32, dirty_tile_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum LinearPixmapError {
    StrideTooSmall { minimum: u32, actual: u32 },
    BufferTooSmall { minimum: usize, actual: usize },
    DirtyTileStorageTooSmall { minimum: usize, actual: usize },
    DirtyTrackingUnavailable, DimensionsOverflow,
    DimensionsMismatch { source: (u32, u32), destination: (u32, u32) },
}

impl LinearPixmap<'static> {
    /// Creates zero-initialized, tightly packed linear working storage.
    pub fn new(width: u32, height: u32) -> Result<Self, LinearPixmapError> {
        let length = usize::try_from(width).ok().and_then(|width|
            usize::try_from(height).ok().and_then(|height| width.checked_mul(height)))
            .ok_or(LinearPixmapError::DimensionsOverflow)?;
        Ok(Self {
            data: LinearPixmapData::Owned(alloc::vec![LinearPremulRGBA::default(); length]),
            width, height, stride: width, dirty_tiles: None,
            dirty_tile_columns: width.div_ceil(LINEAR_DIRTY_TILE_SIZE), dirty_tile_count: 0,
        })
    }
}

impl<'a> LinearPixmap<'a> {
    pub fn from_buffer(data: &'a mut [LinearPremulRGBA<f32>], width: u32, height: u32,
        stride: u32) -> Result<Self, LinearPixmapError> {
        Self::new_inner(data, width, height, stride, None)
    }

    /// Creates a target which records modified 16×16 tiles in caller storage.
    pub fn with_dirty_tiles(data: &'a mut [LinearPremulRGBA<f32>], width: u32, height: u32,
        stride: u32, dirty_tiles: &'a mut [u64]) -> Result<Self, LinearPixmapError> {
        let required = Self::dirty_tile_words(width, height)?;
        if dirty_tiles.len() < required {
            return Err(LinearPixmapError::DirtyTileStorageTooSmall {
                minimum: required, actual: dirty_tiles.len(),
            });
        }
        dirty_tiles[..required].fill(0);
        Self::new_inner(data, width, height, stride, Some(&mut dirty_tiles[..required]))
    }

    pub fn dirty_tile_words(width: u32, height: u32) -> Result<usize, LinearPixmapError> {
        let columns = width.div_ceil(LINEAR_DIRTY_TILE_SIZE);
        let rows = height.div_ceil(LINEAR_DIRTY_TILE_SIZE);
        let tiles = columns.checked_mul(rows).ok_or(LinearPixmapError::DimensionsOverflow)?;
        usize::try_from(tiles.div_ceil(u64::BITS))
            .map_err(|_| LinearPixmapError::DimensionsOverflow)
    }

    fn new_inner(data: &'a mut [LinearPremulRGBA<f32>], width: u32, height: u32, stride: u32,
        dirty_tiles: Option<&'a mut [u64]>) -> Result<Self, LinearPixmapError> {
        if stride < width {
            return Err(LinearPixmapError::StrideTooSmall { minimum: width, actual: stride });
        }
        let (height_usize, stride_usize, width_usize) = (
            usize::try_from(height).map_err(|_| LinearPixmapError::DimensionsOverflow)?,
            usize::try_from(stride).map_err(|_| LinearPixmapError::DimensionsOverflow)?,
            usize::try_from(width) .map_err(|_| LinearPixmapError::DimensionsOverflow)?,
        );
        let minimum = if height_usize == 0 { 0 } else {
            stride_usize.checked_mul(height_usize - 1)
                .and_then(|offset| offset.checked_add(width_usize))
                .ok_or(LinearPixmapError::DimensionsOverflow)?
        };
        if data.len() < minimum {
            return Err(LinearPixmapError::BufferTooSmall { minimum, actual: data.len() });
        }
        Ok(Self { data: LinearPixmapData::Borrowed(data), width, height, stride,
            dirty_tiles, dirty_tile_count: 0,
            dirty_tile_columns: width.div_ceil(LINEAR_DIRTY_TILE_SIZE),
        })
    }

    pub fn  width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn stride(&self) -> u32 { self.stride }
    pub fn as_pixels(&self) -> &[LinearPremulRGBA<f32>] { self.data.as_slice() }

    /// Returns mutable storage and marks the complete target dirty when
    /// incremental presentation is enabled.
    pub fn as_pixels_mut(&mut self) -> &mut [LinearPremulRGBA<f32>] {
        self.mark_all_dirty(); self.data.as_mut_slice()
    }

    pub fn pixel(&self, x: u32, y: u32) -> Option<LinearPremulRGBA<f32>> {
        if x >= self.width || y >= self.height { return None; }
        Some(self.as_pixels()[y as usize * self.stride as usize + x as usize])
    }

    /// Encodes the working buffer into premultiplied sRGB RGBA8888.
    pub fn encode_into(&self, destination: &mut Pixmap<'_>) ->
        Result<(), LinearPixmapError> {
        self.validate_destination(destination)?;
        for y in 0..self.height {
            for x in 0..self.width {
                let color = self.as_pixels()[y as usize * self.stride as usize + x as usize];
                destination.write_encoded_pixel(x, y, color.to_encoded_srgba8());
            }
        }   Ok(())
    }

    /// Presents through a caller-owned transfer LUT instead of per-channel `powf`.
    pub fn encode_into_with(&self, destination: &mut Pixmap<'_>,
        encoder: Srgb8Encoder<'_>) -> Result<(), LinearPixmapError> {
        self.validate_destination(destination)?;
        for y in 0..self.height {
            for x in 0..self.width {
                let color = self.as_pixels()[y as usize * self.stride as usize + x as usize];
                destination.write_encoded_pixel(x, y, encoder.encode(color));
            }
        }   Ok(())
    }

    /// Encodes and consumes only tiles modified since construction or the last
    /// incremental presentation. Untouched destination pixels are preserved.
    pub fn encode_dirty_into(&mut self, destination: &mut Pixmap<'_>) ->
        Result<(), LinearPixmapError> {
        self.encode_dirty_with(destination, |color| color.to_encoded_srgba8())
    }

    /// LUT-accelerated incremental presentation of modified tiles.
    pub fn encode_dirty_into_with(&mut self, destination: &mut Pixmap<'_>,
        encoder: Srgb8Encoder<'_>) -> Result<(), LinearPixmapError> {
        self.encode_dirty_with(destination, |color| encoder.encode(color))
    }

    fn encode_dirty_with<F>(&mut self, destination: &mut Pixmap<'_>,
        encode: F) -> Result<(), LinearPixmapError>
        where F: Fn(LinearPremulRGBA<f32>) -> crate::color::PremulSRGBA8 {
        self.validate_destination(destination)?;
        let tile_area = u64::from(LINEAR_DIRTY_TILE_SIZE).pow(2);
        let pixel_count = u64::from(self.width) * u64::from(self.height);
        if u64::from(self.dirty_tile_count) * tile_area * 2 >= pixel_count {
            for y in 0..self.height {
                for x in 0..self.width {
                    let color = self.as_pixels()[y as usize * self.stride as usize + x as usize];
                    destination.write_encoded_pixel(x, y, encode(color));
                }
            }
            self.dirty_tiles.as_deref_mut()
                .ok_or(LinearPixmapError::DirtyTrackingUnavailable)?.fill(0);
            self.dirty_tile_count = 0;  return Ok(());
        }
        let columns = self.dirty_tile_columns;
        let data = self.data.as_slice();
        let dirty   = self.dirty_tiles.as_deref_mut()
            .ok_or(LinearPixmapError::DirtyTrackingUnavailable)?;
        let (width, height, stride) = (self.width, self.height, self.stride);
        let tile_count = columns * height.div_ceil(LINEAR_DIRTY_TILE_SIZE);
        for tile in 0..tile_count {
            let (word, mask) = ((tile / u64::BITS) as usize, 1_u64 << (tile % u64::BITS));
            if dirty[word] & mask == 0 { continue; }
            let (tile_x, tile_y) = (tile % columns, tile / columns);
            let x_start = tile_x * LINEAR_DIRTY_TILE_SIZE;
            let y_start = tile_y * LINEAR_DIRTY_TILE_SIZE;
            let x_end = (x_start + LINEAR_DIRTY_TILE_SIZE).min(width);
            let y_end = (y_start + LINEAR_DIRTY_TILE_SIZE).min(height);
            for y in y_start..y_end {
                for x in x_start..x_end {
                    let color = data[y as usize * stride as usize + x as usize];
                    destination.write_encoded_pixel(x, y, encode(color));
                }
            }   dirty[word] &= !mask;
        }   self.dirty_tile_count = 0;  Ok(())
    }

    fn validate_destination(&self, destination: &Pixmap<'_>) ->
        Result<(), LinearPixmapError> {
        if (self.width, self.height) != (destination.width(), destination.height()) {
            return Err(LinearPixmapError::DimensionsMismatch {
                source: (self.width, self.height),
                destination: (destination.width(), destination.height()),
            });
        }   Ok(())
    }

    fn blend_sampled_span<S: LinearPaintSampler>(&mut self, x: u32, y: u32,
        len: u32, sampler: &S, coverage: u8) {
        if coverage == 0 || len == 0 { return; }
        self.mark_dirty_span(x, y, len);
        let factor = coverage as f32 / u8::MAX as f32;
        if let Some(color) = sampler.solid_color_linear() {
            let stride = self.stride as usize;
            let pixels = &mut self.data.as_mut_slice()[y as usize * stride + x as usize..
                y as usize * stride + (x + len) as usize];
            if coverage == u8::MAX && color.alpha() == 1.0 {
                pixels.fill(color);
                return;
            }
            let source = color.scale(factor);
            for pixel in pixels {
                *pixel = source.src_over(*pixel);
            }   return;
        }
        let row = y as usize * self.stride as usize;
        let pixels = &mut self.data.as_mut_slice()[row + x as usize..row + (x + len) as usize];
        let mut pixels = pixels.iter_mut();
        if coverage == u8::MAX && sampler.is_opaque_linear() {
            sampler.sample_linear_span(x as f32 + 0.5, y as f32 + 0.5, 1.0, 0.0, len,
                |source| if let Some(pixel) = pixels.next() { *pixel = source; });
        } else {
            sampler.sample_linear_span(x as f32 + 0.5, y as f32 + 0.5, 1.0, 0.0, len,
                |source| if let Some(pixel) = pixels.next() {
                    *pixel = source.scale(factor).src_over(*pixel);
                });
        }
    }

    fn mark_dirty_span(&mut self, x: u32, y: u32, len: u32) {
        let Some(dirty) = self.dirty_tiles.as_deref_mut() else { return; };
        let last = (x + len - 1) / LINEAR_DIRTY_TILE_SIZE;
        let tile_y = y / LINEAR_DIRTY_TILE_SIZE;
        let first  = x / LINEAR_DIRTY_TILE_SIZE;
        for tile_x in first..=last {
            let tile = tile_y * self.dirty_tile_columns + tile_x;
            let word = (tile / u64::BITS) as usize;
            let mask = 1_u64 << (tile % u64::BITS);
            if  dirty[word] &  mask == 0 {
                dirty[word] |= mask;
                self.dirty_tile_count += 1;
            }
        }
    }

    fn mark_all_dirty(&mut self) {
        let rows = self.height.div_ceil(LINEAR_DIRTY_TILE_SIZE);
        let tile_count = self.dirty_tile_columns * rows;
        let Some(dirty) = self.dirty_tiles.as_deref_mut() else { return; };
        dirty.fill(u64::MAX);
        if let Some(last) = dirty.last_mut() {
            let remainder = tile_count % u64::BITS;
            if remainder != 0 { *last = (1_u64 << remainder) - 1; }
        }
        self.dirty_tile_count = tile_count;
    }
}

/// Renders a straight encoded-sRGB solid into a linear-light working target.
pub fn render_solid(path: &Path, transform: Affine, color: SRGBA<u8>,
    options: RenderOptions, target: &mut LinearPixmap<'_>,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError> {
    render_paint(path, transform, &SolidPaint::new(color),
        options, target, workspace)
}

/// Renders a linear sampler through the exact-area analytic rasterizer.
pub fn render_paint<S: LinearPaintSampler>(path: &Path, transform: Affine,
    sampler: &S, options: RenderOptions, target: &mut LinearPixmap<'_>,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = LinearPaintCompositor { target, sampler };
    render_path_to(path, transform, options, width, height,
        &mut compositor, workspace)
}

pub fn render_solid_clipped(path: &Path, transform: Affine, color: SRGBA<u8>,
    clip: Rect, options: RenderOptions, target: &mut LinearPixmap<'_>,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError> {
    render_paint_clipped(path, transform, &SolidPaint::new(color),
        clip, options, target, workspace)
}

pub fn render_paint_clipped<S: LinearPaintSampler>(path: &Path, transform: Affine,
    sampler: &S, clip: Rect, options: RenderOptions, target: &mut LinearPixmap<'_>,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = LinearPaintCompositor { target, sampler };
    render_path_to(path, transform, options, width, height,
        &mut RectClipSink::new(clip, &mut compositor), workspace)
}

pub fn render_solid_masked(path: &Path, transform: Affine, color: SRGBA<u8>,
    mask: CoverageMask<'_>, options: RenderOptions, target: &mut LinearPixmap<'_>,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError> {
    render_paint_masked(path, transform, &SolidPaint::new(color),
        mask, options, target, workspace)
}

pub fn render_paint_masked<S: LinearPaintSampler>(path: &Path,
    transform: Affine, sampler: &S, mask: CoverageMask<'_>,
    options: RenderOptions, target: &mut LinearPixmap<'_>,
    workspace: &mut RenderWorkspace<'_>) -> Result<(), RenderError> {
    validate_mask(mask, target)?;
    let (width, height) = (target.width, target.height);
    let mut compositor = LinearPaintCompositor { target, sampler };
    render_path_to(path, transform, options, width, height,
        &mut MaskClipSink::new(mask, &mut compositor), workspace)
}

pub fn render_stroke_solid(path: &Path, transform: Affine, color: SRGBA<u8>,
    options: StrokePathOptions, target: &mut LinearPixmap<'_>,
    workspace: &mut StrokeWorkspace<'_>) -> Result<(), RenderError> {
    render_stroke_paint(path, transform, &SolidPaint::new(color),
        options, target, workspace)
}

pub fn render_stroke_paint<S: LinearPaintSampler>(path: &Path, transform: Affine,
    sampler: &S, options: StrokePathOptions, target: &mut LinearPixmap<'_>,
    workspace: &mut StrokeWorkspace<'_>) -> Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = LinearPaintCompositor { target, sampler };
    render_stroke_to(path, transform, options, width, height,
        &mut compositor, workspace)
}

pub fn render_stroke_solid_dashed(path: &Path, transform: Affine,
    color: SRGBA<u8>, options: DashedStrokePathOptions<'_>,
    target: &mut LinearPixmap<'_>, workspace: &mut DashedStrokeWorkspace<'_>) ->
    Result<(), RenderError> {
    render_stroke_paint_dashed(path, transform, &SolidPaint::new(color),
        options, target, workspace)
}

pub fn render_stroke_paint_dashed<S: LinearPaintSampler>(path: &Path,
    transform: Affine, sampler: &S, options: DashedStrokePathOptions<'_>,
    target: &mut LinearPixmap<'_>, workspace: &mut DashedStrokeWorkspace<'_>) ->
    Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = LinearPaintCompositor { target, sampler };
    render_stroke_dashed_to(path, transform, options, width, height,
        &mut compositor, workspace)
}

pub fn render_stroke_solid_clipped(path: &Path, transform: Affine,
    color: SRGBA<u8>, clip: Rect, options: StrokePathOptions,
    target: &mut LinearPixmap<'_>, workspace: &mut StrokeWorkspace<'_>) ->
    Result<(), RenderError> {
    render_stroke_paint_clipped(path, transform, &SolidPaint::new(color),
        clip, options, target, workspace)
}

pub fn render_stroke_paint_clipped<S: LinearPaintSampler>(path: &Path,
    transform: Affine, sampler: &S, clip: Rect, options: StrokePathOptions,
    target: &mut LinearPixmap<'_>, workspace: &mut StrokeWorkspace<'_>) ->
    Result<(), RenderError> {
    let (width, height) = (target.width, target.height);
    let mut compositor = LinearPaintCompositor { target, sampler };
    render_stroke_to(path, transform, options, width, height,
        &mut RectClipSink::new(clip, &mut compositor), workspace)
}

pub fn render_stroke_solid_masked(path: &Path, transform: Affine,
    color: SRGBA<u8>, mask: CoverageMask<'_>, options: StrokePathOptions,
    target: &mut LinearPixmap<'_>, workspace: &mut StrokeWorkspace<'_>) ->
    Result<(), RenderError> {
    render_stroke_paint_masked(path, transform, &SolidPaint::new(color),
        mask, options, target, workspace)
}

pub fn render_stroke_paint_masked<S: LinearPaintSampler>(path: &Path,
    transform: Affine, sampler: &S, mask: CoverageMask<'_>,
    options: StrokePathOptions, target: &mut LinearPixmap<'_>,
    workspace: &mut StrokeWorkspace<'_>) -> Result<(), RenderError> {
    validate_mask(mask, target)?;
    let (width, height) = (target.width, target.height);
    let mut compositor = LinearPaintCompositor { target, sampler };
    render_stroke_to(path, transform, options, width, height,
        &mut MaskClipSink::new(mask, &mut compositor), workspace)
}

fn validate_mask(mask: CoverageMask<'_>, target: &LinearPixmap<'_>) ->
    Result<(), RenderError> {
    if (mask.width(), mask.height()) != (target.width, target.height) {
        return Err(RenderError::CoverageDimensionsMismatch {
            coverage: (mask.width(), mask.height()), target: (target.width, target.height),
        });
    }   Ok(())
}

struct LinearPaintCompositor<'a, 'b, S> {
    target: &'a mut LinearPixmap<'b>, sampler: &'a S,
}

impl<S: LinearPaintSampler> CoverageSink for LinearPaintCompositor<'_, '_, S> {
    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        self.target.blend_sampled_span(x, y, len, self.sampler, coverage);
        Ok(())
    }
    type Error = Infallible;
}

#[cfg(test)] mod tests { use super::*;
    use crate::{analytic::{Cell as AnalyticCell, Intersection as AnalyticIntersection},
        edge::Edge, geometry::{PathBuilder, Point},
        raster::CoverageMask, stroke::StrokeContour};

    fn rectangle() -> Path {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((1.0, 0.0))
               .line_to((1.0, 1.0)).line_to((0.0, 1.0)).close();
        builder.build()
    }

    fn render(color: SRGBA<u8>, target: &mut LinearPixmap<'_>) {
        let (mut edges, mut intersections, mut cells) = ([Edge::default(); 4],
            [AnalyticIntersection::default(); 2], [AnalyticCell::default(); 1]);
        render_solid(&rectangle(), Affine::identity(), color,
            RenderOptions::default(), target, &mut RenderWorkspace {
                intersections: &mut intersections, cells: &mut cells,
                edges: &mut edges, row_offsets: &mut [0; 2], edge_indices: &mut [0; 4],
            }).unwrap();
    }

    #[test] fn linear_pixmap_validates_pixel_stride_and_presentation_dimensions() {
        assert_eq!(LinearPixmap::from_buffer(&mut [LinearPremulRGBA::default(); 2], 2, 1, 1)
            .unwrap_err(), LinearPixmapError::StrideTooSmall { minimum: 2, actual: 1 });
        assert_eq!(LinearPixmap::from_buffer(&mut [LinearPremulRGBA::default(); 1], 2, 1, 2)
            .unwrap_err(), LinearPixmapError::BufferTooSmall { minimum: 2, actual: 1 });

        let mut pixels = [LinearPremulRGBA::default(); 2];
        let source = LinearPixmap::from_buffer(&mut pixels, 2, 1, 2).unwrap();
        let mut bytes = [0; 4];
        let mut destination = Pixmap::from_buffer(&mut bytes, 1, 1, 4).unwrap();
        assert_eq!(source.encode_into(&mut destination).unwrap_err(),
            LinearPixmapError::DimensionsMismatch { source: (2, 1), destination: (1, 1) });
        assert_eq!(LinearPixmap::dirty_tile_words(32, 16), Ok(1));
        assert_eq!(LinearPixmap::with_dirty_tiles(
            &mut [LinearPremulRGBA::default(); 1], 32, 16, 32, &mut []).unwrap_err(),
            LinearPixmapError::DirtyTileStorageTooSmall { minimum: 1, actual: 0 });
        assert_eq!(LinearPixmap::from_buffer(
            &mut [LinearPremulRGBA::default(); 1], 1, 1, 1).unwrap()
            .encode_dirty_into(&mut Pixmap::from_buffer(&mut [0; 4], 1, 1, 4).unwrap())
            .unwrap_err(), LinearPixmapError::DirtyTrackingUnavailable);
    }

    #[test] fn linear_pixmap_supports_owned_and_borrowed_storage() {
        let mut owned = LinearPixmap::new(2, 1).unwrap();
        assert_eq!((owned.width(), owned.height(), owned.stride()), (2, 1, 2));
        owned.as_pixels_mut()[1] = LinearPremulRGBA::new(0.25, 0.0, 0.0, 0.5).unwrap();
        assert_eq!(owned.pixel(1, 0).unwrap().to_array(), [0.25, 0.0, 0.0, 0.5]);

        let mut storage = [LinearPremulRGBA::default(); 3];
        let mut borrowed = LinearPixmap::from_buffer(&mut storage, 2, 1, 3).unwrap();
        borrowed.as_pixels_mut()[0] = LinearPremulRGBA::new(0.0, 0.25, 0.0, 0.5).unwrap();
        assert_eq!(borrowed.pixel(0, 0).unwrap().to_array(), [0.0, 0.25, 0.0, 0.5]);
    }

    #[test] fn linear_source_over_differs_from_encoded_domain_and_encodes_once() {
        let mut pixels = [LinearPremulRGBA::default(); 1];
        let mut target = LinearPixmap::from_buffer(&mut pixels, 1, 1, 1).unwrap();
        render(SRGBA::blue(), &mut target);
        render(SRGBA::new(255, 0, 0, 128), &mut target);

        let [r, g, b, a] = target.pixel(0, 0).unwrap().to_array();
        assert!((r - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(g, 0.0);
        assert!((b - 127.0 / 255.0).abs() < 1e-6);
        assert_eq!(a, 1.0);

        let mut bytes = [0; 4];
        target.encode_into(&mut Pixmap::from_buffer(&mut bytes, 1, 1, 4).unwrap()).unwrap();
        assert_eq!(bytes, [188, 0, 187, 255]);
        assert_ne!(bytes, [128, 0, 127, 255]);

        let (mut lut, mut approximate) = ([0; crate::color::SRGB8_ENCODE_LUT_SIZE], [0; 4]);
        target.encode_into_with(
            &mut Pixmap::from_buffer(&mut approximate, 1, 1, 4).unwrap(),
            Srgb8Encoder::new(&mut lut).unwrap()).unwrap();
        for channel in 0..4 {
            assert!(approximate[channel].abs_diff(bytes[channel]) <= 1);
        }
    }

    #[test] fn linear_clip_mask_and_stroke_share_the_coverage_pipeline() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((2.0, 0.0))
               .line_to((2.0, 1.0)).line_to((0.0, 1.0)).close();
        let path = builder.build();
        let mut intersections = [AnalyticIntersection::default(); 4];
        let (mut edges, mut cells) =
            ([Edge::default(); 4], [AnalyticCell::default(); 2]);
        let mut workspace = RenderWorkspace {
            intersections: &mut intersections, cells: &mut cells,
            edges: &mut edges, row_offsets: &mut [0; 2], edge_indices: &mut [0; 4],
        };

        let mut clipped = [LinearPremulRGBA::default(); 2];
        render_solid_clipped(&path, Affine::identity(), SRGBA::white(),
            Rect::from_ltrb(0.5, 0.0, 1.5, 1.0).unwrap(),
            RenderOptions::default(),
            &mut LinearPixmap::from_buffer(&mut clipped, 2, 1, 2).unwrap(),
            &mut workspace).unwrap();
        for pixel in clipped {
            assert!((pixel.to_array()[3] - 128.0 / 255.0).abs() < 1e-6);
        }

        let mut masked = [LinearPremulRGBA::default(); 2];
        render_solid_masked(&path, Affine::identity(), SRGBA::white(),
            CoverageMask::new(&[128, 255], 2, 1, 2).unwrap(),
            RenderOptions::default(),
            &mut LinearPixmap::from_buffer(&mut masked, 2, 1, 2).unwrap(),
            &mut workspace).unwrap();
        assert!((masked[0].to_array()[3] - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(masked[1].to_array()[3], 1.0);

        let mut line = PathBuilder::new();
        line.move_to((0.0, 0.5)).line_to((2.0, 0.5));
        let (mut points, mut stroke_edges, mut contours) =
            ([Point::default(); 2], [Edge::default(); 4], [StrokeContour::default(); 1]);
        let mut stroke_pixels = [LinearPremulRGBA::default(); 2];
        render_stroke_solid(&line.build(), Affine::identity(), SRGBA::white(),
            StrokePathOptions::default(),
            &mut LinearPixmap::from_buffer(&mut stroke_pixels, 2, 1, 2).unwrap(),
            &mut StrokeWorkspace {
                points: &mut points, contours: &mut contours, edges: &mut stroke_edges,
                intersections: &mut [AnalyticIntersection::default(); 4],
                cells: &mut [AnalyticCell::default(); 2], row_offsets: &mut [0; 2],
                edge_indices: &mut [0; 4],
            }).unwrap();
        assert_eq!(stroke_pixels[0].to_array()[3], 1.0);
        assert_eq!(stroke_pixels[1].to_array()[3], 1.0);
    }

    #[test] fn dirty_presentation_updates_touched_tiles_once() {
        let mut pixels = [LinearPremulRGBA::default(); 48 * 16];
        let mut dirty = [u64::MAX; 1];
        let mut target = LinearPixmap::with_dirty_tiles(&mut pixels,
            48, 16, 48, &mut dirty).unwrap();
        render_solid(&rectangle(), Affine::identity(), SRGBA::white(),
            RenderOptions::default(), &mut target, &mut RenderWorkspace {
                intersections: &mut [AnalyticIntersection::default(); 4],
                cells: &mut [AnalyticCell::default(); 48], edges: &mut [Edge::default(); 4],
                row_offsets: &mut [0; 17], edge_indices: &mut [0; 4],
            }).unwrap();

        let mut bytes = [17; 48 * 16 * 4];
        target.encode_dirty_into(&mut Pixmap::from_buffer(&mut bytes,
            48, 16, 192).unwrap()).unwrap();
        assert_eq!(&bytes[..4], &[255; 4]);
        assert_eq!(&bytes[16 * 4..16 * 4 + 4], &[17; 4]);

        bytes[..4].fill(9);
        target.encode_dirty_into(&mut Pixmap::from_buffer(&mut bytes,
            48, 16, 192).unwrap()).unwrap();
        assert_eq!(&bytes[..4], &[9; 4]);
    }

    #[test] fn mutable_pixel_access_marks_the_complete_target_dirty() {
        let mut pixels = [LinearPremulRGBA::default(); 32 * 16];
        let mut dirty = [0];
        let mut target = LinearPixmap::with_dirty_tiles(
            &mut pixels, 32, 16, 32, &mut dirty).unwrap();
        target.as_pixels_mut()[16] = LinearPremulRGBA::new(1.0, 0.0, 0.0, 1.0).unwrap();

        let mut bytes = [17; 32 * 16 * 4];
        target.encode_dirty_into(
            &mut Pixmap::from_buffer(&mut bytes, 32, 16, 128).unwrap()).unwrap();
        assert_eq!(&bytes[16 * 4..17 * 4], &[255, 0, 0, 255]);
        assert_eq!(&bytes[..4], &[0; 4]);
    }

    #[test] fn randomized_linear_source_over_matches_f64_reference() {
        let mut pixel = [LinearPremulRGBA::default(); 1];
        let mut target = LinearPixmap::from_buffer(&mut pixel, 1, 1, 1).unwrap();
        let (mut state, mut reference) = (0xA341_316C_u32, [0.0_f64; 4]);
        for _ in 0..4096 {
            let next = |state: &mut u32| {
                *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (*state >> 24) as u8
            };
            let color = SRGBA::new(
                next(&mut state), next(&mut state), next(&mut state), next(&mut state));
            let coverage = next(&mut state);
            let paint = SolidPaint::new(color);
            target.blend_sampled_span(0, 0, 1, &paint, coverage);

            let source = paint.linear_color().to_array().map(f64::from);
            let factor = f64::from(coverage) / f64::from(u8::MAX);
            let source = source.map(|channel| channel * factor);
            let inverse = 1.0 - source[3];
            for channel in 0..4 {
                reference[channel] = source[channel] + reference[channel] * inverse;
            }
            let actual = target.pixel(0, 0).unwrap().to_array();
            for channel in 0..4 {
                assert!((f64::from(actual[channel]) - reference[channel]).abs() < 2e-6);
            }
            assert!(actual[0] <= actual[3] && actual[1] <= actual[3] && actual[2] <= actual[3]);
        }
    }

    #[test] fn opaque_sampler_fast_path_matches_source_over_at_full_and_partial_coverage() {
        use crate::{color::SRGBA as RGBA,
            sampler::{GradientStop, GradientStops, LinearGradient, SpreadMode}};

        struct Composite<'a, S>(&'a S);
        impl<S: LinearPaintSampler> LinearPaintSampler for Composite<'_, S> {
            fn sample_linear(&self, x: f32, y: f32) -> LinearPremulRGBA<f32> {
                self.0.sample_linear(x, y)
            }
            fn sample_linear_span(&self, x: f32, y: f32, dx: f32, dy: f32, len: u32,
                emit: impl FnMut(LinearPremulRGBA<f32>)) {
                self.0.sample_linear_span(x, y, dx, dy, len, emit)
            }
        }

        let stops = [GradientStop::new(0.0, RGBA::red()),
                     GradientStop::new(1.0, RGBA::blue())];
        let gradient = LinearGradient::new((0.0, 0.0), (8.0, 0.0),
            GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
        assert!(gradient.is_opaque_linear());
        for coverage in [u8::MAX, 128] {
            let initial = SolidPaint::new(SRGBA::new(20, 200, 40, 160)).linear_color();
            let (mut fast, mut reference) = ([initial; 8], [initial; 8]);
            LinearPixmap::from_buffer(&mut fast, 8, 1, 8).unwrap()
                .blend_sampled_span(0, 0, 8, &gradient, coverage);
            LinearPixmap::from_buffer(&mut reference, 8, 1, 8).unwrap()
                .blend_sampled_span(0, 0, 8, &Composite(&gradient), coverage);
            assert_eq!(fast, reference);
        }
    }
}
