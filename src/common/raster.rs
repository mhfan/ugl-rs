//! Backend-neutral coverage protocols and mask clipping.

use core::convert::Infallible;

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum FillRule { NonZero, EvenOdd }

impl FillRule {
    pub(crate) fn contains(self, winding: i32) -> bool {
        match self {
            Self::NonZero => winding != 0,
            Self::EvenOdd => winding & 1 != 0,
        }
    }
}

pub trait CoverageSink {    type Error;
    /// Receives a non-empty horizontal run with uniform non-zero coverage.
    ///
    /// Producers guarantee that `x + len` is representable and lies inside the
    /// target row. Consumers may therefore stream the run without clipping.
    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error>;

    fn pixel(&mut self, x: u32, y: u32, coverage: u8) -> Result<(), Self::Error> {
        self.span(x, y, 1, coverage)
    }
}

impl<E, F> CoverageSink for F where F: FnMut(u32, u32, u8) -> Result<(), E> {
    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        for x in x..x + len { self(x, y, coverage)?; }  Ok(())
    }   type Error = E;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum CoverageMaskError {
    StrideTooSmall { minimum: u32, actual: u32 },
    BufferTooSmall { minimum: usize, actual: usize },
    DimensionsOverflow,
}

/// Borrowed 8-bit coverage mask with explicit row stride.
#[derive(Clone, Copy, Debug)] pub struct CoverageMask<'a> {
    data: &'a [u8], width: u32, height: u32, stride: u32,
    origin_x: u32, origin_y: u32, data_width: u32, data_height: u32,
    non_zero_bounds: Option<(u32, u32, u32, u32)>,
}

/// Mutable storage used to rasterize a coverage mask without allocation.
#[derive(Debug)] pub struct CoverageMaskMut<'a> {
    data: &'a mut [u8], width: u32, height: u32, stride: u32,
}

fn validate_mask_buffer(length: usize, width: u32, height: u32, stride: u32) ->
    Result<(), CoverageMaskError> {
    if stride < width {
        return Err(CoverageMaskError::StrideTooSmall { minimum: width, actual: stride });
    }
    let (height, stride, width) = (
        usize::try_from(height).map_err(|_| CoverageMaskError::DimensionsOverflow)?,
        usize::try_from(stride).map_err(|_| CoverageMaskError::DimensionsOverflow)?,
        usize::try_from(width).map_err(|_| CoverageMaskError::DimensionsOverflow)?,
    );
    let minimum = if height == 0 { 0 } else {
        stride.checked_mul(height - 1).and_then(|offset| offset.checked_add(width))
            .ok_or(CoverageMaskError::DimensionsOverflow)?
    };
    if length < minimum {
        return Err(CoverageMaskError::BufferTooSmall { minimum, actual: length });
    }   Ok(())
}

impl<'a> CoverageMask<'a> {
    /// Validates the storage and derives non-zero bounds once.
    ///
    /// The returned mask is cheap to copy and should be retained across draws;
    /// masked rendering reuses its cached bounds instead of rescanning pixels.
    pub fn new(data: &'a [u8], width: u32, height: u32, stride: u32) ->
        Result<Self, CoverageMaskError> {
        validate_mask_buffer(data.len(), width, height, stride)?;
        let non_zero_bounds = find_non_zero_bounds(data, width, height, stride);
        Ok(Self { data, width, height, stride, origin_x: 0, origin_y: 0,
            data_width: width, data_height: height, non_zero_bounds })
    }

    /// Wraps storage for a local subregion while retaining full-target coordinates.
    pub fn from_region(data: &'a [u8], dimensions: (u32, u32),
        region: (u32, u32, u32, u32), stride: u32) -> Result<Self, CoverageMaskError> {
        let (width, height) = dimensions;
        let (origin_x, origin_y, right, bottom) = region;
        if origin_x > right || origin_y > bottom || right > width || bottom > height {
            return Err(CoverageMaskError::DimensionsOverflow);
        }
        let (data_width, data_height) = (right - origin_x, bottom - origin_y);
        validate_mask_buffer(data.len(), data_width, data_height, stride)?;
        let non_zero_bounds = find_non_zero_bounds(data, data_width, data_height, stride)
            .map(|(left, top, right, bottom)|
                (left + origin_x, top + origin_y, right + origin_x, bottom + origin_y));
        Ok(Self { data, width, height, stride, origin_x, origin_y,
            data_width, data_height, non_zero_bounds })
    }

    pub fn  width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn stride(&self) -> u32 { self.stride }
    pub fn as_bytes(&self) -> &[u8] { self.data }

    pub(crate) fn storage_region(&self) -> (u32, u32, u32, u32) {
        (self.origin_x, self.origin_y,
            self.origin_x + self.data_width, self.origin_y + self.data_height)
    }

    pub(crate) fn non_zero_bounds(&self) -> Option<(u32, u32, u32, u32)> {
        self.non_zero_bounds
    }
}

fn find_non_zero_bounds(data: &[u8], width: u32, height: u32, stride: u32) ->
    Option<(u32, u32, u32, u32)> {
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    for y in 0..height {
        let start = y as usize * stride as usize;
        let row = &data[start..start + width as usize];
        let Some(first) = row.iter().position(|&coverage| coverage != 0) else { continue; };
        let last = row.iter().rposition(|&coverage| coverage != 0).unwrap() + 1;
        left = left.min(first as _);   right = right.max(last as _);
        top = top.min(y);              bottom = y + 1;
    }
    (left < right).then_some((left, top, right, bottom))
}

impl<'a> CoverageMaskMut<'a> {
    pub fn new(data: &'a mut [u8], width: u32, height: u32, stride: u32) ->
        Result<Self, CoverageMaskError> {
        validate_mask_buffer(data.len(), width, height, stride)?;
        Ok(Self { data, width, height, stride })
    }

    pub fn  width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn stride(&self) -> u32 { self.stride }
    pub fn as_mask(&self) -> CoverageMask<'_> {
        CoverageMask::new(self.data, self.width, self.height, self.stride)
            .expect("mutable mask was validated at construction")
    }

    pub fn clear(&mut self) {
        for y in 0..self.height as usize {
            let start = y * self.stride as usize;
            self.data[start..start + self.width as usize].fill(0);
        }
    }
}

impl CoverageSink for CoverageMaskMut<'_> {
    type Error = Infallible;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        if x >= self.width || y >= self.height { return Ok(()); }
        let len = len.min(self.width - x);
        let start = y as usize * self.stride as usize + x as usize;
        self.data[start..start + len as usize].fill(coverage);
        Ok(())
    }
}

/// Writes device-space coverage into tightly packed storage for one target subregion.
pub(crate) struct RegionMaskSink<'a> {
    data: &'a mut [u8], left: u32, top: u32, width: u32, height: u32,
}

impl<'a> RegionMaskSink<'a> {
    pub(crate) fn new(data: &'a mut [u8],
        region: (u32, u32, u32, u32)) -> Self {
        let (left, top, right, bottom) = region;
        Self { data, left, top, width: right - left, height: bottom - top }
    }
}

impl CoverageSink for RegionMaskSink<'_> {
    type Error = Infallible;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        if x < self.left || y < self.top || y >= self.top + self.height {
            return Ok(());
        }
        let start_x = x - self.left;
        if start_x >= self.width { return Ok(()); }
        let len = len.min(self.width - start_x);
        let start = (y - self.top) as usize * self.width as usize + start_x as usize;
        self.data[start..start + len as usize].fill(coverage);
        Ok(())
    }
}

/// Coverage adapter that multiplies incoming spans by a borrowed mask.
pub struct  MaskClipSink<'a, S> { mask: CoverageMask<'a>, sink: &'a mut S }

impl<'a, S> MaskClipSink<'a, S> {
    pub fn new(mask: CoverageMask<'a>, sink: &'a mut S) -> Self { Self { mask, sink } }
}

impl<S> CoverageSink for MaskClipSink<'_, S> where S: CoverageSink {
    type Error = S::Error;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        let (left, top, right, bottom) = self.mask.storage_region();
        if y < top || y >= bottom { return Ok(()); }
        let (start, end) = (x.max(left), x.saturating_add(len).min(right));
        if start >= end { return Ok(()); }
        let row = (y - top) as usize * self.mask.stride as usize;
        let mask = &self.mask.data[row + (start - left) as usize..
            row + (end - left) as usize];
        let mut cursor = 0;
        while cursor < mask.len() {
            let value = mask[cursor];
            let run = equal_prefix(&mask[cursor..], value);
            let clipped = (coverage as u16 * value as u16 + 127).div_euclid(255) as u8;
            if clipped != 0 {
                self.sink.span(start + cursor as u32, y, run as _, clipped)?;
            }
            cursor += run;
        }   Ok(())
    }
}

fn equal_prefix(bytes: &[u8], value: u8) -> usize {
    let repeated = u64::from(value) * 0x0101_0101_0101_0101;
    let mut length = 0;
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        if u64::from_ne_bytes(chunk.try_into().unwrap()) != repeated { break; }
        length += 8;
    }
    for &byte in &bytes[length..] {
        if byte != value { break; }
        length += 1;
    }
    length
}

#[cfg(test)] mod tests { use super::*;
    use alloc::{vec, vec::Vec};
    use core::convert::Infallible;

    #[derive(Default)] struct SpanRecorder(Vec<(u32, u32, u32, u8)>);

    impl CoverageSink for SpanRecorder { type Error = Infallible;
        fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
            Result<(), Self::Error> {
            self.0.push((x, y, len, coverage)); Ok(())
        }
    }

    #[test] fn coverage_masks_validate_storage_preserve_padding_and_coalesce() {
        assert_eq!(CoverageMask::new(&[0; 4], 3, 2, 2).unwrap_err(),
            CoverageMaskError::StrideTooSmall { minimum: 3, actual: 2 });
        assert_eq!(CoverageMask::new(&[0; 6], 3, 2, 4).unwrap_err(),
            CoverageMaskError::BufferTooSmall { minimum: 7, actual: 6 });
        let (mut spans, mut data) = (SpanRecorder::default(), vec![9; 8]);
        let mut mask = CoverageMaskMut::new(&mut data, 3, 2, 4).unwrap();
        mask.clear(); mask.span(1, 0, 8, 128).unwrap();
        MaskClipSink::new(mask.as_mask(), &mut spans).span(0, 0, 3, 128).unwrap();
        assert_eq!(spans.0, [(1, 0, 2, 64)]);
        assert_eq!(data, [0, 128, 128, 9, 0, 0, 0, 9]);

        spans.0.clear();
        let bounded = CoverageMask::from_region(&[255, 128, 0, 255], (6, 4),
            (2, 1, 4, 3), 2).unwrap();
        MaskClipSink::new(bounded, &mut spans).span(0, 2, 6, u8::MAX).unwrap();
        assert_eq!(spans.0, [(3, 2, 1, 255)]);
        let data: Vec<_> = [0_u8; 13].into_iter().chain([255; 20])
            .chain([128; 7]).collect();
        spans.0.clear();
        let mask = CoverageMask::new(&data, 40, 1, 40).unwrap();
        MaskClipSink::new(mask, &mut spans).span(0, 0, 40, 128).unwrap();
        assert_eq!(spans.0, [(13, 0, 20, 128), (33, 0, 7, 64)]);
        MaskClipSink::new(mask, &mut spans)
            .span(u32::MAX, 0, u32::MAX, 255).unwrap();
    }

    #[test] fn coverage_mask_bounds_ignore_zero_rows_and_stride_padding() {
        let data = [0, 0, 0, 9, 0, 7, 8, 9, 0, 0, 0, 9];
        let mask = CoverageMask::new(&data, 3, 3, 4).unwrap();
        assert_eq!(mask.non_zero_bounds(), Some((1, 1, 3, 2)));
        assert_eq!(CoverageMask::new(&[0; 12], 3, 3, 4).unwrap().non_zero_bounds(), None);
    }
}
