//! Backend-neutral coverage protocols and mask clipping.

use alloc::{vec, vec::Vec};
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

/// One non-empty horizontal coverage run within a 16-row strip.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] #[repr(C)]
pub struct CoverageRun { pub x: u32, pub len: u32, pub row: u8, pub coverage: u8 }

/// Range of coverage runs belonging to one non-empty 16-row strip.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] #[repr(C)]
pub struct CoverageStrip { pub y: u32, pub run_start: u32, pub run_count: u32 }

/// Borrowed backend-neutral sparse 8-bit coverage.
#[derive(Clone, Copy, Debug)] pub struct CoverageStrips<'a> {
    width: u32, height: u32,
    strips: &'a [CoverageStrip], runs: &'a [CoverageRun],
}

impl<'a> CoverageStrips<'a> {
    pub(crate) fn from_parts(width: u32, height: u32,
        strips: &'a [CoverageStrip], runs: &'a [CoverageRun]) -> Self {
        Self { width, height, strips, runs }
    }
    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn strips(&self) -> &'a [CoverageStrip] { self.strips }
    pub fn runs(&self) -> &'a [CoverageRun] { self.runs }
    pub fn replay<S: CoverageSink>(&self, sink: &mut S) -> Result<(), S::Error> {
        for strip in self.strips {
            let start = strip.run_start as usize;
            for run in &self.runs[start..start + strip.run_count as usize] {
                sink.span(run.x, strip.y + u32::from(run.row), run.len, run.coverage)?;
            }
        }   Ok(())
    }
}

pub(crate) fn push_sparse_run(strips: &mut Vec<CoverageStrip>, runs: &mut Vec<CoverageRun>,
    y: u32, x: u32, len: u32, coverage: u8) {
    if coverage == 0 || len == 0 { return; }
    let strip_y = y / 16 * 16;
    if strips.last().is_none_or(|strip| strip.y != strip_y) {
        strips.push(CoverageStrip {
            y: strip_y, run_start: runs.len() as _, run_count: 0,
        });
    }
    let row = (y - strip_y) as u8;
    if let Some(last) = runs.last_mut()
        && last.row == row && last.coverage == coverage && last.x + last.len == x {
        last.len += len;
        return;
    }
    runs.push(CoverageRun { x, len, row, coverage });
    strips.last_mut().unwrap().run_count += 1;
}

pub(crate) struct SparseCoverageSink {
    pub(crate) strips: Vec<CoverageStrip>, pub(crate) runs: Vec<CoverageRun>,
}

impl SparseCoverageSink {
    pub(crate) fn new(region: (u32, u32, u32, u32), mut strips: Vec<CoverageStrip>,
        mut runs: Vec<CoverageRun>) -> Self {
        let (_, top, _, bottom) = region;
        strips.clear(); runs.clear();
        strips.reserve((bottom.div_ceil(16) - top / 16) as _);
        runs.reserve((bottom - top).saturating_mul(3) as _);
        Self { strips, runs }
    }
}

impl CoverageSink for SparseCoverageSink {
    type Error = Infallible;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        push_sparse_run(&mut self.strips, &mut self.runs, y, x, len, coverage);
        Ok(())
    }
}

pub(crate) struct SparseRuns<'a> {
    strips: core::slice::Iter<'a, CoverageStrip>, runs: &'a [CoverageRun],
    strip: Option<CoverageStrip>, index: usize,
}

impl<'a> SparseRuns<'a> {
    pub(crate) fn new(strips: &'a [CoverageStrip], runs: &'a [CoverageRun]) -> Self {
        Self { strips: strips.iter(), runs, strip: None, index: 0 }
    }
}

impl Iterator for SparseRuns<'_> {
    type Item = (u32, CoverageRun);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(strip) = self.strip {
                let end = strip.run_start as usize + strip.run_count as usize;
                if self.index < end {
                    let run = self.runs[self.index]; self.index += 1;
                    return Some((strip.y + u32::from(run.row), run));
                }
            }
            let strip = *self.strips.next()?;
            self.index = strip.run_start as usize;
            self.strip = Some(strip);
        }
    }
}

pub(crate) fn intersect_sparse_masks(left_strips: &[CoverageStrip],
    left_runs: &[CoverageRun], right_strips: &[CoverageStrip],
    right_runs: &[CoverageRun]) -> (Vec<CoverageStrip>, Vec<CoverageRun>) {
    let (mut strips, mut runs) = (
        Vec::with_capacity(left_strips.len().min(right_strips.len())),
        Vec::with_capacity(left_runs.len().min(right_runs.len())));
    let (mut left_iter, mut right_iter) = (
        SparseRuns::new(left_strips, left_runs), SparseRuns::new(right_strips, right_runs));
    let (mut left, mut right) = (left_iter.next(), right_iter.next());
    while let (Some((left_y, left_run)), Some((right_y, right_run))) = (left, right) {
        let (left_end, right_end) = (left_run.x + left_run.len, right_run.x + right_run.len);
        if left_y < right_y || left_y == right_y && left_end <= right_run.x {
            left = left_iter.next(); continue;
        }
        if right_y < left_y || left_y == right_y && right_end <= left_run.x {
            right = right_iter.next(); continue;
        }
        let (x, end) = (left_run.x.max(right_run.x), left_end.min(right_end));
        let coverage = (u16::from(left_run.coverage) * u16::from(right_run.coverage) + 127)
            .div_euclid(255) as u8;
        push_sparse_run(&mut strips, &mut runs, left_y, x, end - x, coverage);
        if left_end <= right_end { left = left_iter.next(); }
        if right_end <= left_end { right = right_iter.next(); }
    }
    (strips, runs)
}

pub(crate) fn clip_sparse_bounds(strips: &[CoverageStrip], runs: &[CoverageRun],
    bounds: (u32, u32, u32, u32), coverage: impl Fn(u8, u32, u32) -> u8) ->
    (Vec<CoverageStrip>, Vec<CoverageRun>) {
    let (x0, y0, x1, y1) = bounds;
    let (mut clipped_strips, mut clipped_runs) =
        (Vec::with_capacity(strips.len()), Vec::with_capacity(runs.len()));
    for strip in strips {
        let start = strip.run_start as usize;
        for run in &runs[start..start + strip.run_count as usize] {
            let y = strip.y + u32::from(run.row);
            if y < y0 || y >= y1 { continue; }
            let (start, end) = (run.x.max(x0), (run.x + run.len).min(x1));
            if start >= end { continue; }
            push_sparse_run(&mut clipped_strips, &mut clipped_runs,
                y, start, 1, coverage(run.coverage, start, y));
            if end > start + 2 {
                push_sparse_run(&mut clipped_strips, &mut clipped_runs,
                    y, start + 1, end - start - 2,
                    coverage(run.coverage, start + 1, y));
            }
            if end > start + 1 {
                push_sparse_run(&mut clipped_strips, &mut clipped_runs,
                    y, end - 1, 1, coverage(run.coverage, end - 1, y));
            }
        }
    }
    (clipped_strips, clipped_runs)
}

pub(crate) fn multiply_sparse_mask(data: &mut [u8],
    region: (u32, u32, u32, u32), stride: u32,
    strips: &[CoverageStrip], runs: &[CoverageRun]) {
    let (left, top, right, bottom) = region;
    for y in top..bottom {
        let row_offset = (y - top) as usize * stride as usize;
        let row = &mut data[row_offset..row_offset + (right - left) as usize];
        let strip_y = y / 16 * 16;
        let Ok(index) = strips.binary_search_by_key(&strip_y, |strip| strip.y) else {
            row.fill(0); continue;
        };
        let strip = strips[index];
        let strip_runs = &runs[strip.run_start as usize..
            strip.run_start as usize + strip.run_count as usize];
        let local_y = (y - strip_y) as u8;
        let start = strip_runs.partition_point(|run| run.row < local_y);
        let end = start + strip_runs[start..].partition_point(|run| run.row == local_y);
        let mut cursor = left;
        for run in &strip_runs[start..end] {
            let (run_left, run_right) = (left.max(run.x), right.min(run.x + run.len));
            if run_left >= run_right { continue; }
            row[(cursor - left) as usize..(run_left - left) as usize].fill(0);
            for value in &mut row[(run_left - left) as usize..(run_right - left) as usize] {
                *value = (u16::from(*value) * u16::from(run.coverage) + 127)
                    .div_euclid(255) as _;
            }
            cursor = run_right;
        }
        row[(cursor - left) as usize..].fill(0);
    }
}

pub(crate) enum SparseStorage {
    Empty,
    OpaqueRect((u32, u32, u32, u32)),
    Sparse { strips: Vec<CoverageStrip>, runs: Vec<CoverageRun> },
    Dense { data: Vec<u8>, left: u32, top: u32,
        right: u32, bottom: u32, stride: u32 },
}

pub(crate) fn finish_sparse_coverage(strips: Vec<CoverageStrip>, runs: Vec<CoverageRun>,
    width: u32, height: u32, spare_strips: &mut Vec<CoverageStrip>,
    spare_runs: &mut Vec<CoverageRun>) -> Option<SparseStorage> {
    let Some((first_y, first_run)) = SparseRuns::new(&strips, &runs).next() else {
        (*spare_strips, *spare_runs) = (strips, runs);
        return Some(SparseStorage::Empty);
    };
    let opaque_rect = {
        let mut expected_y = first_y;
        let mut iter = SparseRuns::new(&strips, &runs);
        iter.all(|(y, run)| {
            let matches = y == expected_y && run.x == first_run.x &&
                run.len == first_run.len && run.coverage == u8::MAX;
            expected_y = expected_y.saturating_add(1);
            matches
        }).then_some((first_run.x, first_y, first_run.x + first_run.len, expected_y))
    };
    if let Some(bounds) = opaque_rect {
        (*spare_strips, *spare_runs) = (strips, runs);
        return Some(SparseStorage::OpaqueRect(bounds));
    }
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    for (y, run) in SparseRuns::new(&strips, &runs) {
        left = left.min(run.x); top = top.min(y);
        right = right.max(run.x + run.len); bottom = bottom.max(y + 1);
    }
    let dense_len = usize::try_from(right - left).ok().and_then(|row|
        usize::try_from(bottom - top).ok().and_then(|height| row.checked_mul(height)))
        ?;
    let sparse_bytes = strips.len().checked_mul(core::mem::size_of::<CoverageStrip>())
        .and_then(|bytes| runs.len().checked_mul(core::mem::size_of::<CoverageRun>())
            .and_then(|runs| bytes.checked_add(runs)))?;
    // Encoded bytes also proxy replay cost: fragmented coverage pays one full
    // CoverageRun per short span. Retained capacity is deliberately not charged.
    if sparse_bytes < dense_len {
        return Some(SparseStorage::Sparse { strips, runs });
    }
    let stride = right - left;
    let mut data = vec![0; dense_len];
    for (y, run) in SparseRuns::new(&strips, &runs) {
        let start = (y - top) as usize * stride as usize + (run.x - left) as usize;
        data[start..start + run.len as usize].fill(run.coverage);
    }
    (*spare_strips, *spare_runs) = (strips, runs);
    Some(SparseStorage::Dense { data, left, top, right, bottom, stride })
}

fn for_each_mask_run(mask: CoverageMask<'_>, bounds: (u32, u32, u32, u32),
    mut visit: impl FnMut(u32, u32, u32, u8)) {
    let (left, top, right, bottom) = bounds;
    let (storage_left, storage_top, _, _) = mask.storage_region();
    for y in top..bottom {
        let row_start = (y - storage_top) as usize * mask.stride() as usize +
            (left - storage_left) as usize;
        let row = &mask.as_bytes()[row_start..row_start + (right - left) as usize];
        let mut x = 0;
        while x < row.len() {
            if row[x] == 0 { x += 1; continue; }
            let coverage = row[x];
            let len = row[x..].iter().position(|&value| value != coverage)
                .unwrap_or(row.len() - x);
            visit(y, left + x as u32, len as _, coverage);
            x += len;
        }
    }
}

pub(crate) fn sparse_mask_parts(mask: CoverageMask<'_>) ->
    Option<(Vec<CoverageStrip>, Vec<CoverageRun>)> {
    let (left, top, right, bottom) = mask.non_zero_bounds()?;
    let bounds = (left, top, right, bottom);
    let dense_bytes = usize::try_from(right - left).ok()?
        .checked_mul(usize::try_from(bottom - top).ok()?)?;
    let maximum_runs = usize::try_from(mask.non_zero_count()).ok()?;
    let maximum_strips = usize::try_from(bottom.div_ceil(16) - top / 16).ok()?;
    let maximum_sparse_bytes = maximum_strips
        .checked_mul(core::mem::size_of::<CoverageStrip>())?
        .checked_add(maximum_runs.checked_mul(core::mem::size_of::<CoverageRun>())?)?;
    let (strip_count, run_count) = if maximum_sparse_bytes < dense_bytes {
        (maximum_strips, maximum_runs)
    } else {
        let (mut strip_count, mut run_count) = (0, 0);
        let mut previous_strip = None;
        for_each_mask_run(mask, bounds, |y, _, _, _| {
            run_count += 1;
            let strip_y = y / 16 * 16;
            if previous_strip != Some(strip_y) {
                strip_count += 1; previous_strip = Some(strip_y);
            }
        });
        (strip_count, run_count)
    };
    let sparse_bytes = strip_count.checked_mul(core::mem::size_of::<CoverageStrip>())?
        .checked_add(run_count.checked_mul(core::mem::size_of::<CoverageRun>())?)?;
    if sparse_bytes >= dense_bytes { return None; }
    let (mut strips, mut runs) = (
        Vec::with_capacity(strip_count), Vec::with_capacity(run_count));
    for_each_mask_run(mask, bounds, |y, x, len, coverage|
        push_sparse_run(&mut strips, &mut runs, y, x, len, coverage));
    Some((strips, runs))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum CoverageMaskError {
    StrideTooSmall { minimum: u32, actual: u32 },
    BufferTooSmall { minimum: usize, actual: usize },
    DimensionsOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub(crate) enum MaskKind {
    Empty,
    OpaqueRect((u32, u32, u32, u32)),
    Coverage((u32, u32, u32, u32)),
}

impl MaskKind {
    fn bounds(self) -> Option<(u32, u32, u32, u32)> { match self {
        Self::Empty => None,
        Self::OpaqueRect(bounds) | Self::Coverage(bounds) => Some(bounds),
    } }
}

/// Borrowed 8-bit coverage mask with explicit row stride.
#[derive(Clone, Copy, Debug)] pub struct CoverageMask<'a> {
    data: &'a [u8], width: u32, height: u32, stride: u32,
    origin_x: u32, origin_y: u32, data_width: u32, data_height: u32,
    kind: MaskKind, non_zero_count: u64,
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
        let (kind, non_zero_count) = classify_mask(data, width, height, stride);
        Ok(Self { data, width, height, stride, origin_x: 0, origin_y: 0,
            data_width: width, data_height: height, kind, non_zero_count })
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
        let offset = |(left, top, right, bottom)|
            (left + origin_x, top + origin_y, right + origin_x, bottom + origin_y);
        let (kind, non_zero_count) = classify_mask(data, data_width, data_height, stride);
        let kind = match kind {
            MaskKind::Empty => MaskKind::Empty,
            MaskKind::OpaqueRect(bounds) => MaskKind::OpaqueRect(offset(bounds)),
            MaskKind::Coverage(bounds) => MaskKind::Coverage(offset(bounds)),
        };
        Ok(Self { data, width, height, stride, origin_x, origin_y,
            data_width, data_height, kind, non_zero_count })
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
        self.kind.bounds()
    }

    pub(crate) fn kind(&self) -> MaskKind { self.kind }
    /// Number of non-zero coverage samples within retained storage.
    pub fn non_zero_count(&self) -> u64 { self.non_zero_count }
}

fn classify_mask(data: &[u8], width: u32, height: u32, stride: u32) -> (MaskKind, u64) {
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    let (mut non_zero, mut all_opaque) = (0_u64, true);
    for y in 0..height {
        let start = y as usize * stride as usize;
        let row = &data[start..start + width as usize];
        let Some(first) = row.iter().position(|&coverage| coverage != 0) else { continue; };
        let last = row.iter().rposition(|&coverage| coverage != 0).unwrap() + 1;
        for &coverage in &row[first..last] {
            if coverage != 0 {
                non_zero += 1;
                all_opaque &= coverage == u8::MAX;
            }
        }
        left = left.min(first as _);   right = right.max(last as _);
        top = top.min(y);              bottom = y + 1;
    }
    if left >= right { return (MaskKind::Empty, 0); }
    let bounds = (left, top, right, bottom);
    let area = u64::from(right - left) * u64::from(bottom - top);
    let kind = if all_opaque && non_zero == area { MaskKind::OpaqueRect(bounds) }
        else { MaskKind::Coverage(bounds) };
    (kind, non_zero)
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

pub(crate) trait ClipMask: Copy {
    fn dimensions(self) -> (u32, u32);
    fn bounds(self) -> Option<(u32, u32, u32, u32)>;
    fn clip_span<S: CoverageSink>(self, x: u32, y: u32, len: u32,
        coverage: u8, sink: &mut S) -> Result<(), S::Error>;
}

impl ClipMask for CoverageMask<'_> {
    fn dimensions(self) -> (u32, u32) { (self.width, self.height) }
    fn bounds(self) -> Option<(u32, u32, u32, u32)> { self.non_zero_bounds() }

    fn clip_span<S: CoverageSink>(self, x: u32, y: u32, len: u32,
        coverage: u8, sink: &mut S) -> Result<(), S::Error> {
        match self.kind {
            MaskKind::Empty => return Ok(()),
            MaskKind::OpaqueRect((left, top, right, bottom)) => {
                if y < top || y >= bottom { return Ok(()); }
                let (start, end) = (x.max(left), x.saturating_add(len).min(right));
                return if start < end { sink.span(start, y, end - start, coverage) }
                    else { Ok(()) };
            }
            MaskKind::Coverage(_) => {}
        }
        let (left, top, right, bottom) = self.storage_region();
        if y < top || y >= bottom { return Ok(()); }
        let (start, end) = (x.max(left), x.saturating_add(len).min(right));
        if start >= end { return Ok(()); }
        let row = (y - top) as usize * self.stride as usize;
        let mask = &self.data[row + (start - left) as usize..
            row + (end - left) as usize];
        let mut cursor = 0;
        while cursor < mask.len() {
            let value = mask[cursor];
            let run = equal_prefix(&mask[cursor..], value);
            let clipped = (coverage as u16 * value as u16 + 127).div_euclid(255) as u8;
            if clipped != 0 {
                sink.span(start + cursor as u32, y, run as _, clipped)?;
            }
            cursor += run;
        }   Ok(())
    }
}

impl ClipMask for CoverageStrips<'_> {
    fn dimensions(self) -> (u32, u32) { (self.width, self.height) }
    fn bounds(self) -> Option<(u32, u32, u32, u32)> {
        let first = self.runs.first()?;
        let (mut left, mut top, mut right, mut bottom) =
            (first.x, self.height, first.x + first.len, 0);
        for strip in self.strips { for run in &self.runs[strip.run_start as usize..
            strip.run_start as usize + strip.run_count as usize] {
            let y = strip.y + u32::from(run.row);
            left = left.min(run.x); right = right.max(run.x + run.len);
            top = top.min(y); bottom = bottom.max(y + 1);
        } }
        Some((left, top, right, bottom))
    }
    fn clip_span<S: CoverageSink>(self, x: u32, y: u32, len: u32,
        coverage: u8, sink: &mut S) -> Result<(), S::Error> {
        let strip_y = y / 16 * 16;
        let Ok(index) = self.strips.binary_search_by_key(&strip_y, |strip| strip.y)
            else { return Ok(()); };
        let strip = self.strips[index];
        let runs = &self.runs[strip.run_start as usize..
            strip.run_start as usize + strip.run_count as usize];
        let row = (y - strip_y) as u8;
        let start = runs.partition_point(|run| run.row < row);
        let end = start + runs[start..].partition_point(|run| run.row == row);
        let incoming_end = x.saturating_add(len);
        for run in &runs[start..end] {
            let (left, right) = (x.max(run.x), incoming_end.min(run.x + run.len));
            if left >= right { continue; }
            let coverage = (u16::from(coverage) * u16::from(run.coverage) + 127)
                .div_euclid(255) as u8;
            if coverage != 0 { sink.span(left, y, right - left, coverage)?; }
        }
        Ok(())
    }
}

/// Coverage adapter that multiplies incoming spans by a borrowed mask.
pub(crate) struct MaskClipSink<'a, M, S> { mask: M, sink: &'a mut S }

impl<'a, M, S> MaskClipSink<'a, M, S> {
    pub(crate) fn new(mask: M, sink: &'a mut S) -> Self { Self { mask, sink } }
}

impl<M, S> CoverageSink for MaskClipSink<'_, M, S>
    where M: ClipMask, S: CoverageSink {
    type Error = S::Error;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        self.mask.clip_span(x, y, len, coverage, self.sink)
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

    #[test] fn coverage_mask_classifies_empty_opaque_rect_and_general_coverage() {
        assert_eq!(CoverageMask::new(&[0; 8], 4, 2, 4).unwrap().kind(), MaskKind::Empty);
        let rectangle = [0, 255, 255, 0, 0, 255, 255, 0];
        let rectangle = CoverageMask::new(&rectangle, 4, 2, 4).unwrap();
        assert_eq!(rectangle.kind(), MaskKind::OpaqueRect((1, 0, 3, 2)));
        let mut spans = SpanRecorder::default();
        MaskClipSink::new(rectangle, &mut spans).span(0, 1, 4, 127).unwrap();
        assert_eq!(spans.0, [(1, 1, 2, 127)]);
        let hole = [0, 255, 255, 0, 0, 255, 0, 0];
        assert_eq!(CoverageMask::new(&hole, 4, 2, 4).unwrap().kind(),
            MaskKind::Coverage((1, 0, 3, 2)));
        let antialiased = [0, 128, 255, 0, 0, 255, 255, 0];
        assert_eq!(CoverageMask::new(&antialiased, 4, 2, 4).unwrap().kind(),
            MaskKind::Coverage((1, 0, 3, 2)));
    }

    #[test] fn sparse_storage_coalesces_classifies_and_intersects_runs() {
        let mut sink = SparseCoverageSink::new((0, 0, 8, 2), Vec::new(), Vec::new());
        sink.span(1, 0, 2, 255).unwrap(); sink.span(3, 0, 2, 255).unwrap();
        sink.span(1, 1, 4, 255).unwrap();
        assert_eq!(sink.runs, [
            CoverageRun { x: 1, len: 4, row: 0, coverage: 255 },
            CoverageRun { x: 1, len: 4, row: 1, coverage: 255 },
        ]);
        let (mut spare_strips, mut spare_runs) = (Vec::new(), Vec::new());
        assert!(matches!(finish_sparse_coverage(sink.strips, sink.runs, 8, 2,
            &mut spare_strips, &mut spare_runs), Some(SparseStorage::OpaqueRect((1, 0, 5, 2)))));

        let (mut left_strips, mut left_runs) = (Vec::new(), Vec::new());
        let (mut right_strips, mut right_runs) = (Vec::new(), Vec::new());
        push_sparse_run(&mut left_strips, &mut left_runs, 0, 0, 4, 128);
        push_sparse_run(&mut right_strips, &mut right_runs, 0, 2, 4, 128);
        let (_, runs) = intersect_sparse_masks(
            &left_strips, &left_runs, &right_strips, &right_runs);
        assert_eq!(runs, [CoverageRun { x: 2, len: 2, row: 0, coverage: 64 }]);
    }
}
