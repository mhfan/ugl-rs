//! Widened arithmetic primitives for the Q24.8 fixed-point raster backend.

use core::cmp::Ordering;
use crate::{edge::Edge, geometry::{FixedScalar, Point}, raster::{CoverageSink, FillRule}};

/// Accepted Q24.8 raw-coordinate magnitude for the fixed rasterizer.
///
/// This corresponds to ±2,097,152 device units and leaves enough headroom for
/// every line-intersection multiply-add to remain in `i64`.
pub const   SUBPIXEL_SCALE: u32 = 1 << 8;
pub const DEVICE_RAW_LIMIT: i32 = 1 << 29;
pub const FIXED_STRIP_HEIGHT: u32 = 16;
const PIXEL_AREA_TWICE: u64 = 2 * SUBPIXEL_SCALE as u64 * SUBPIXEL_SCALE as u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum FixedRasterError {
    CoordinateOutOfRange, CrossingEdges, DimensionsOverflow, InvalidEdge, InvalidIntersectionOrder,
    InvalidSlab, InvalidSlabPartition, InvalidTrapezoid, UnbalancedWinding,
    WorkspaceTooSmall { kind: FixedWorkspace, required: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedWorkspace {
    Lines, Segments, Trapezoids, RowArea, Intersections, Spans,
    StripOffsets, StripIndices, CoverageStrips, CoverageRuns,
    CoverageTiles, CoverageTileRuns, CoverageTilePieces, CoverageTileColumns,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedRenderError<E> { Raster(FixedRasterError), Sink(E) }

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FixedLine { x0: i32, y0: i32, dx: i64, dy: u32, winding: i8 }

impl FixedLine {
    pub fn new(edge: Edge<FixedScalar>) -> Result<Self, FixedRasterError> {
        let (x0, y0, x1, y1) = (
            edge.upper.x.to_bits(), edge.upper.y.to_bits(),
            edge.lower.x.to_bits(), edge.lower.y.to_bits(),
        );
        if [x0, y0, x1, y1].iter()
            .any(|value| value.unsigned_abs() > DEVICE_RAW_LIMIT as u32) {
            return Err(FixedRasterError::CoordinateOutOfRange);
        }
        let dy = y1 - y0;
        if  dy <= 0 || !matches!(edge.winding, -1 | 1) {
            return Err(FixedRasterError::InvalidEdge);
        }
        Ok(Self { x0, y0, dx: x1 as i64 - x0 as i64, dy: dy as _, winding: edge.winding })
    }

    pub fn intersection(&self, y: FixedScalar) -> FixedIntersection {
        let offset = y.to_bits() as i64 - self.y0 as i64;
        FixedIntersection {  den: self.dy, winding: self.winding,
            num: self.x0 as i64 * self.dy as i64 +  self.dx * offset,
        }
    }

    fn contains_y(&self, y: FixedScalar) -> bool {
        let y = y.to_bits();
        self.y0 <= y && (y as i64) < self.y0 as i64 + self.dy as i64
    }

    fn segment_in_slab(&self, line_index: u32, top: i32, bottom: i32) -> Option<FixedSegment> {
        let (line_top, line_bottom) = (self.y0, self.y0 + self.dy as i32);
        let (top_y, bottom_y) = (top.max(line_top), bottom.min(line_bottom));
        (top_y < bottom_y).then(|| FixedSegment { line_index, top_y, bottom_y,
               top_x: self.intersection(FixedScalar::from_bits(top_y)),
            bottom_x: self.intersection(FixedScalar::from_bits(bottom_y)),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixedIntersection { num: i64, den: u32, pub winding: i8 }

impl Default for FixedIntersection {
    fn default() -> Self { Self { num: 0, den: 1, winding: 0 } }
}

impl FixedIntersection {
    pub fn floor_raw(self) -> i64 { self.num.div_euclid(self.den as i64) }

    /// Rounds to the nearest Q24.8 grid coordinate, with ties away from zero.
    pub fn round_raw(self) -> i64 { round_ratio(self.num, self.den as _) }

    pub fn cmp_x(&self, other: &Self) -> Ordering {
        let (left_divisor, right_divisor) = (self.den as i64, other.den as i64);
        let (left_floor, right_floor) = (self.num.div_euclid(left_divisor),
                                        other.num.div_euclid(right_divisor));
        left_floor.cmp(&right_floor).then_with(|| {
            let (left_remainder, right_remainder) = (
                 self.num.rem_euclid(left_divisor)  as u64,
                other.num.rem_euclid(right_divisor) as u64,
            );
            (left_remainder * other.den as u64).cmp(&(right_remainder * self.den as u64))
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FixedSpan { pub from: FixedIntersection, pub to: FixedIntersection }

/// A directed edge fragment clipped to one horizontal slab.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FixedSegment { line_index: u32, top_y: i32, bottom_y: i32,
    pub top_x: FixedIntersection, pub bottom_x: FixedIntersection,
}

impl FixedSegment {
       pub fn top_y(self) -> FixedScalar { FixedScalar::from_bits(self.top_y) }
    pub fn bottom_y(self) -> FixedScalar { FixedScalar::from_bits(self.bottom_y) }
    pub fn height_raw(self) -> u32 { (self.bottom_y - self.top_y) as _ }
}

/// A non-self-intersecting fill region bounded by two linear edge fragments.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FixedTrapezoid { pub left: FixedSegment, pub right: FixedSegment }

impl FixedTrapezoid {
    fn interior_pixel_range(self, width: u32) -> core::ops::Range<u32> {
        let scale = SUBPIXEL_SCALE as i64;
        let  left = self. left.top_x.round_raw().max(self. left.bottom_x.round_raw());
        let right = self.right.top_x.round_raw().min(self.right.bottom_x.round_raw());
        let (start, end): (u32, u32) = (
            (left.div_euclid(scale) + (left.rem_euclid(scale) != 0) as i64)
                .clamp(0, width as i64) as _,
            right.div_euclid(scale).clamp(0, width as i64) as _,
        );
        start..end.max(start)
    }

    /// Returns twice the Q24.8-grid area, avoiding a fractional division by two.
    pub fn area_twice_raw(self) -> Result<u64, FixedRasterError> {
        if  self.left.   top_y != self.right.   top_y ||
            self.left.bottom_y != self.right.bottom_y ||
            self.left.   top_y >= self. left.bottom_y {
            return Err(FixedRasterError::InvalidTrapezoid);
        }
        let (top_width, bottom_width) = (
            self.right.   top_x.round_raw() - self.left.   top_x.round_raw(),
            self.right.bottom_x.round_raw() - self.left.bottom_x.round_raw(),
        );
        if top_width < 0 || bottom_width < 0 {
            return Err(FixedRasterError::InvalidTrapezoid);
        }
        let height = (self.left.bottom_y - self.left.top_y) as u64;
         Ok(height * (top_width as u64 + bottom_width as u64))
    }

    /// Returns horizontally contiguous pixels guaranteed to have full coverage.
    ///
    /// Partial-height slabs and boundary pixels are excluded; they require
    /// analytic area accumulation.
    pub fn full_pixel_range(self, width: u32) ->
        Result<core::ops::Range<u32>, FixedRasterError> {
        self.area_twice_raw()?;
        if  self.left.top_y.rem_euclid(SUBPIXEL_SCALE as i32) != 0 ||
            self.left.bottom_y - self.left.top_y != SUBPIXEL_SCALE as i32 {
            return Ok(0..0);
        }
        Ok(self.interior_pixel_range(width))
    }

    /// Clips this row-local trapezoid to one pixel and returns doubled area.
    pub fn pixel_area_twice_raw(self, x: u32, y: u32) -> Result<u64, FixedRasterError> {
        self.area_twice_raw()?;
        let scale = SUBPIXEL_SCALE as u64;
        let (left, top) = (x as u64 * scale, y as u64 * scale);
        let (right, bottom) = (left + scale, top + scale);
        if right > DEVICE_RAW_LIMIT as u64 || bottom > DEVICE_RAW_LIMIT as u64 {
            return Err(FixedRasterError::CoordinateOutOfRange);
        }
        let (left, top, right, bottom) =
            (left as i64, top as i64, right as i64, bottom as i64);
        if (self.left.top_y as i64) < top || self.left.bottom_y as i64 > bottom {
            return Err(FixedRasterError::InvalidSlabPartition);
        }

        let polygon = [
            PixelPoint { x: self. left.   top_x.round_raw(), y: self. left.   top_y as _ },
            PixelPoint { x: self.right.   top_x.round_raw(), y: self.right.   top_y as _ },
            PixelPoint { x: self.right.bottom_x.round_raw(), y: self.right.bottom_y as _ },
            PixelPoint { x: self. left.bottom_x.round_raw(), y: self. left.bottom_y as _ },
        ];
        let (mut clipped_left, mut clipped_right) =
            ([PixelPoint::default(); 8], [PixelPoint::default(); 8]);
        let  left_count = clip_vertical(&polygon, left, true, &mut clipped_left);
        let right_count = clip_vertical(&clipped_left[..left_count],
            right, false, &mut clipped_right);
        if  right_count < 3 { return Ok(0); }

        let mut area_twice = 0_i64;
        for index in 0..right_count {
            let current = clipped_right[index];
            let next = clipped_right[(index + 1) % right_count];
            area_twice += (current.x - left) * (next.y - top)
                - (next.x - left) * (current.y - top);
        }
        Ok(area_twice.unsigned_abs().min(PIXEL_AREA_TWICE))
    }
}

type PixelPoint = Point<i64>;

fn clip_vertical(input: &[PixelPoint], boundary: i64, keep_greater: bool,
    output: &mut [PixelPoint; 8]) -> usize {
    let Some(mut previous) = input.last().copied() else { return 0; };
    let mut previous_inside = if keep_greater {
        previous.x >= boundary
    } else {
        previous.x <= boundary
    };
    let mut count = 0;
    for current in input.iter().copied() {
        let current_inside = if keep_greater {
            current.x >= boundary
        } else {
            current.x <= boundary
        };
        if current_inside != previous_inside {
            output[count] = intersect_vertical(previous, current, boundary);  count += 1;
        }
        if current_inside { output[count] = current; count += 1; }
        previous = current;  previous_inside = current_inside;
    }   count
}

fn intersect_vertical(from: PixelPoint, to: PixelPoint, x: i64) -> PixelPoint {
    let (mut numerator, mut denominator) =
        ((to.y - from.y) * (x - from.x), to.x - from.x);
    if denominator < 0 { numerator = -numerator; denominator = -denominator; }
    PixelPoint { x, y: from.y + round_ratio(numerator, denominator) }
}

fn round_ratio(numerator: i64, denominator: i64) -> i64 {
    debug_assert!(denominator > 0);
    let (floor, remainder) = (
        numerator.div_euclid(denominator), numerator.rem_euclid(denominator),
    );
    match (remainder * 2).cmp(&denominator) {
        Ordering::Equal if numerator >= 0 => floor + 1,
        Ordering::Equal | Ordering::Less  => floor,
        Ordering::Greater => floor + 1,
    }
}

fn round_ratio_i128(numerator: i128, denominator: i128) -> i128 {
    debug_assert!(denominator > 0);
    let (floor, remainder) = (
        numerator.div_euclid(denominator), numerator.rem_euclid(denominator),
    );
    match (remainder * 2).cmp(&denominator) {
        Ordering::Equal if numerator >= 0 => floor + 1,
        Ordering::Equal | Ordering::Less  => floor,
        Ordering::Greater => floor + 1,
    }
}

/// Maps a pixel-clipped doubled Q24.8 area to round-to-nearest 8-bit coverage.
pub fn quantize_area_coverage(area_twice_raw: u64) -> u8 {
    let area = area_twice_raw.min(PIXEL_AREA_TWICE);
    ((area * u8::MAX as u64 + PIXEL_AREA_TWICE / 2) / PIXEL_AREA_TWICE) as _
}

/// Accumulates one row-local trapezoid into a caller-owned doubled-area row.
pub fn accumulate_trapezoid_row(trapezoid: FixedTrapezoid, width: u32, y: u32,
    row_area: &mut [u64]) -> Result<(), FixedRasterError> {
    trapezoid.area_twice_raw()?;
    let width_usize = usize::try_from(width).map_err(|_|
        FixedRasterError::DimensionsOverflow)?;
    if row_area.len() < width_usize {
        return Err(FixedRasterError::WorkspaceTooSmall {
            kind: FixedWorkspace::RowArea, required: width_usize,
        });
    }
    let scale = SUBPIXEL_SCALE as i64;
    let row_top = y as u64 * SUBPIXEL_SCALE as u64;
    if  row_top + SUBPIXEL_SCALE as u64 > DEVICE_RAW_LIMIT as u64 {
        return Err(FixedRasterError::CoordinateOutOfRange);
    }
    if (trapezoid.left.top_y as i64) < row_top as i64 ||
        trapezoid.left.bottom_y as i64 > row_top as i64 + scale {
        return Err(FixedRasterError::InvalidSlabPartition);
    }

    let xs = [trapezoid. left.top_x.round_raw(), trapezoid. left.bottom_x.round_raw(),
              trapezoid.right.top_x.round_raw(), trapezoid.right.bottom_x.round_raw()];
    let (minimum, maximum) = (*xs.iter().min().unwrap(), *xs.iter().max().unwrap());
    let first = minimum.div_euclid(scale).clamp(0, width as i64) as u32;
    let last = (maximum.div_euclid(scale) +
               (maximum.rem_euclid(scale) != 0) as i64).clamp(0, width as i64) as u32;

    let interior = trapezoid.interior_pixel_range(width);
    let interior_area = 2 * trapezoid.left.height_raw() as u64 * SUBPIXEL_SCALE as u64;

    for x in first..last {
        let area = if interior.contains(&x) { interior_area } else {
            trapezoid.pixel_area_twice_raw(x, y)?
        };
        let cell = &mut row_area[x as usize];
        *cell = (*cell + area).min(PIXEL_AREA_TWICE);
    }   Ok(())
}

/// Quantizes and coalesces one accumulated fixed-point area row.
pub fn emit_area_runs<S>(row_area: &[u64], y: u32, sink: &mut S) ->
    Result<(), S::Error> where S: CoverageSink {
    let mut x = 0;
    while x < row_area.len() {
        let coverage = quantize_area_coverage(row_area[x]);
        if  coverage == 0 { x += 1; continue; }
        let start = x;      x += 1;
        while x < row_area.len() && quantize_area_coverage(row_area[x]) == coverage { x += 1; }
        sink.span(start as _, y, (x - start) as _, coverage)?;
    }   Ok(())
}

pub struct FixedRasterWorkspace<'a> {
    pub segments: &'a mut [FixedSegment],
    pub trapezoids: &'a mut [FixedTrapezoid],
    pub row_area: &'a mut [u64],
    pub strip_offsets: &'a mut [u32],
    pub strip_indices: &'a mut [u32],
}

/// One non-empty horizontal coverage run within a 16-row strip.
///
/// Keeping `y` strip-local makes the record 12 bytes while preserving the
/// Q24.8 backend's full supported device width.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] #[repr(C)]
pub struct FixedCoverageRun { pub x: u32, pub len: u32, pub row: u8, pub coverage: u8 }

/// Range of coverage runs belonging to one non-empty 16-row strip.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] #[repr(C)]
pub struct FixedCoverageStrip { pub y: u32, pub run_start: u32, pub run_count: u32 }

/// Caller-owned storage for optional retained sparse coverage.
pub struct FixedCoverageWorkspace<'a> {
    pub strips: &'a mut [FixedCoverageStrip],
    pub runs: &'a mut [FixedCoverageRun],
}

/// Borrowed sparse coverage produced by [`rasterize_lines_to_strips`].
#[derive(Clone, Copy, Debug)]
pub struct FixedCoverageStrips<'a> {
    width: u32, height: u32,
    strips: &'a [FixedCoverageStrip],
      runs: &'a [FixedCoverageRun],
}

impl<'a> FixedCoverageStrips<'a> {
    pub fn  width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn strips(&self) -> &'a [FixedCoverageStrip] { self.strips }
    pub fn   runs(&self) -> &'a [FixedCoverageRun] { self.runs }

    /// Replays retained coverage through the ordinary streaming sink contract.
    pub fn replay<S: CoverageSink>(&self, sink: &mut S) -> Result<(), S::Error> {
        for strip in self.strips {
            let start = strip.run_start as usize;
            for run in &self.runs[start..start + strip.run_count as usize] {
                sink.span(run.x, strip.y + run.row as u32, run.len, run.coverage)?;
            }
        }   Ok(())
    }
}

/// Caller-owned storage required to bin prepared lines for a target height.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FixedStripRequirements { pub offsets: usize, pub indices: usize }

/// Computes the exact strip-bin capacities required by [`rasterize_lines`].
pub fn fixed_strip_requirements(lines: &[FixedLine], height: u32) ->
    Result<FixedStripRequirements, FixedRasterError> {
    let offsets = usize::try_from(height.div_ceil(FIXED_STRIP_HEIGHT))
        .map_err(|_| FixedRasterError::DimensionsOverflow)?
        .checked_add(1).ok_or(FixedRasterError::DimensionsOverflow)?;
    let mut indices = 0_usize;
    for line in lines {
        if let Some(range) = line_strip_range(*line, height)? {
            indices = indices.checked_add(range.len())
                .ok_or(FixedRasterError::DimensionsOverflow)?;
        }
    }
    if lines.len() > u32::MAX as usize || indices > u32::MAX as usize {
        return Err(FixedRasterError::DimensionsOverflow);
    }   Ok(FixedStripRequirements { offsets, indices })
}

/// Rasterizes prepared fixed-point lines into anti-aliased coverage runs.
///
/// Edge crossings are located with exact `i128` rational arithmetic. Their
/// coordinates are rounded only when they become Q24.8 trapezoid boundaries.
pub fn rasterize_lines<S>(lines: &[FixedLine], width: u32, height: u32,
    fill_rule: FillRule, workspace: &mut FixedRasterWorkspace<'_>, sink: &mut S) ->
    Result<(), FixedRenderError<S::Error>> where S: CoverageSink {
    let width_usize = usize::try_from(width)
        .map_err(|_| FixedRenderError::Raster(FixedRasterError::DimensionsOverflow))?;
    let extent = |value: u32| value as u64 * SUBPIXEL_SCALE as u64;
    if extent(width) > DEVICE_RAW_LIMIT as u64 || extent(height) > DEVICE_RAW_LIMIT as u64 {
        return Err(FixedRenderError::Raster(FixedRasterError::CoordinateOutOfRange));
    }
    for (kind, available, required) in [
        (FixedWorkspace::Segments, workspace.  segments.len(), lines.len()),
        (FixedWorkspace::Trapezoids, workspace.trapezoids.len(), lines.len().div_ceil(2)),
        (FixedWorkspace::RowArea, workspace.  row_area.len(), width_usize)] {
        if available < required {
            return Err(FixedRenderError::Raster(
                FixedRasterError::WorkspaceTooSmall { kind, required }));
        }
    }
    let Some((first_line, rest)) = lines.split_first() else { return Ok(()); };
    let bins = build_strip_bins(lines, height,
        workspace.strip_offsets, workspace.strip_indices).map_err(FixedRenderError::Raster)?;
    let (mut minimum_y, mut maximum_y) =
        (first_line.y0, first_line.y0 + first_line.dy as i32);
    for line in rest {
        minimum_y = minimum_y.min(line.y0);
        maximum_y = maximum_y.max(line.y0 + line.dy as i32);
    }
    let scale = SUBPIXEL_SCALE as i32;
    let first_row = minimum_y.div_euclid(scale).clamp(0, height as i32) as u32;
    let last_row = (maximum_y.div_euclid(scale) +
        (maximum_y.rem_euclid(scale) != 0) as i32).clamp(0, height as i32) as u32;

    let (mut current_strip, mut pending, mut active_count) = (usize::MAX, 0, 0);
    for y in first_row..last_row {
        let strip = y as usize / FIXED_STRIP_HEIGHT as usize;
        let strip_lines = bins.indices(strip);
        if strip != current_strip {
            current_strip = strip;
            active_count = 0;
            pending = 0;
        }
        let row = &mut workspace.row_area[..width_usize];  row.fill(0);
        let (mut top, bottom) = (FixedScalar::from_bits(extent(y)     as i32),
                                 FixedScalar::from_bits(extent(y + 1) as i32));
        while top < bottom {
            active_count = retain_active_lines(
                lines, workspace.segments, active_count, top);
            activate_pending_lines(lines, strip_lines, &mut pending, top,
                workspace.segments, &mut active_count);
            let vertex_boundary = next_active_slab_boundary(lines, strip_lines, pending,
                &workspace.segments[..active_count], top, bottom);
            if active_count == 0 {
                if vertex_boundary >= bottom { break; }
                top = vertex_boundary;  continue;
            }
            prepare_active_segments(lines, &mut workspace.segments[..active_count],
                top, vertex_boundary).map_err(FixedRenderError::Raster)?;
            let (next, snap_top, snap_bottom) = next_crossing_boundary(lines,
                &mut workspace.segments[..active_count], top, vertex_boundary)
                .map_err(FixedRenderError::Raster)?;
            if next != vertex_boundary {
                prepare_active_segments(lines,
                    &mut workspace.segments[..active_count], top, next)
                    .map_err(FixedRenderError::Raster)?;
            }
            if snap_top {
                snap_crossing_events(lines, top,
                    &mut workspace.segments[..active_count], true);
            }
            if snap_bottom {
                snap_crossing_events(lines, next,
                    &mut workspace.segments[..active_count], false);
            }
            let segments = &mut workspace.segments[..active_count];
            let trapezoid_count = if next == vertex_boundary && !snap_top && !snap_bottom {
                collect_ordered_trapezoids(segments, fill_rule, workspace.trapezoids)
            } else {
                collect_trapezoids(segments, fill_rule, workspace.trapezoids)
            }.map_err(FixedRenderError::Raster)?;
            for trapezoid in workspace.trapezoids[..trapezoid_count].iter().copied() {
                accumulate_trapezoid_row(trapezoid, width, y, row)
                    .map_err(FixedRenderError::Raster)?;
            }   top = next;
        }
        emit_area_runs(row, y, sink).map_err(FixedRenderError::Sink)?;
    }   Ok(())
}

/// Rasterizes into compact caller-owned sparse coverage strips.
///
/// This optional retained form is intended for batching, caching, or a later
/// strip/tile compositor. [`rasterize_lines`] remains the lower-memory
/// streaming path. On insufficient capacity, `required` is the first
/// unavailable record count; callers may grow that buffer and retry.
pub fn rasterize_lines_to_strips<'a>(lines: &[FixedLine], width: u32, height: u32,
    fill_rule: FillRule, raster_workspace: &mut FixedRasterWorkspace<'_>,
    coverage_workspace: FixedCoverageWorkspace<'a>) ->
    Result<FixedCoverageStrips<'a>, FixedRasterError> {
    let mut encoder = FixedCoverageEncoder {
        strips: coverage_workspace.strips, runs: coverage_workspace.runs,
        width, height, strip_count: 0, run_count: 0,
    };
    match rasterize_lines(lines, width, height, fill_rule, raster_workspace, &mut encoder) {
        Ok(()) => Ok(encoder.finish()),
        Err(FixedRenderError::Raster(error) | FixedRenderError::Sink(error)) => Err(error),
    }
}

struct FixedCoverageEncoder<'a> {
    strips: &'a mut [FixedCoverageStrip],
      runs: &'a mut [FixedCoverageRun],
    width: u32, height: u32,
    strip_count: usize,
      run_count: usize,
}

impl<'a> FixedCoverageEncoder<'a> {
    fn finish(self) -> FixedCoverageStrips<'a> {
        FixedCoverageStrips {
            width: self.width, height: self.height,
            strips: &self.strips[..self.strip_count],
            runs: &self.runs[..self.run_count],
        }
    }
}

impl CoverageSink for FixedCoverageEncoder<'_> {
    type Error = FixedRasterError;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        let strip_y = y / FIXED_STRIP_HEIGHT * FIXED_STRIP_HEIGHT;
        let new_strip = self.strip_count == 0 ||
            self.strips[self.strip_count - 1].y != strip_y;
        if new_strip && self.strip_count == self.strips.len() {
            return Err(FixedRasterError::WorkspaceTooSmall {
                kind: FixedWorkspace::CoverageStrips, required: self.strip_count + 1,
            });
        }
        if self.run_count == self.runs.len() {
            return Err(FixedRasterError::WorkspaceTooSmall {
                kind: FixedWorkspace::CoverageRuns, required: self.run_count + 1,
            });
        }
        if self.run_count == u32::MAX as usize {
            return Err(FixedRasterError::DimensionsOverflow);
        }
        if new_strip {
            self.strips[self.strip_count] = FixedCoverageStrip {
                y: strip_y, run_start: self.run_count as _, run_count: 0,
            };
            self.strip_count += 1;
        }
        self.runs[self.run_count] =
            FixedCoverageRun { x, len, row: (y - strip_y) as _, coverage };
        self.run_count += 1;
        self.strips[self.strip_count - 1].run_count += 1;
        Ok(())
    }
}

#[derive(Debug)] struct FixedStripBins<'a> { offsets: &'a [u32], indices: &'a [u32] }

impl FixedStripBins<'_> {
    fn indices(&self, strip: usize) -> &[u32] {
        &self.indices[self.offsets[strip] as usize..self.offsets[strip + 1] as usize]
    }
}

fn line_strip_range(line: FixedLine, height: u32) ->
    Result<Option<core::ops::Range<usize>>, FixedRasterError> {
    let scale = SUBPIXEL_SCALE as i32;
    let height = i32::try_from(height).map_err(|_| FixedRasterError::DimensionsOverflow)?;
    let bottom = line.y0 + line.dy as i32;
    let first_row = line.y0.div_euclid(scale).clamp(0, height);
    let last_row = (bottom.div_euclid(scale) +
        (bottom.rem_euclid(scale) != 0) as i32).clamp(0, height);
    if  first_row >= last_row { return Ok(None); }
    let strip_height = FIXED_STRIP_HEIGHT as i32;
    let first = first_row.div_euclid(strip_height) as usize;
    let last = (last_row - 1).div_euclid(strip_height) as usize + 1;
    Ok(Some(first..last))
}

fn build_strip_bins<'a>(lines: &[FixedLine], height: u32, offsets: &'a mut [u32],
    indices: &'a mut [u32]) -> Result<FixedStripBins<'a>, FixedRasterError> {
    let required = fixed_strip_requirements(lines, height)?;
    for (kind, available, required) in [
        (FixedWorkspace::StripOffsets, offsets.len(), required.offsets),
        (FixedWorkspace::StripIndices, indices.len(), required.indices)] {
        if available < required {
            return Err(FixedRasterError::WorkspaceTooSmall { kind, required });
        }
    }
    let offsets = &mut offsets[..required.offsets];
    let indices = &mut indices[..required.indices];
    offsets.fill(0);

    for line in lines {
        if let Some(range) = line_strip_range(*line, height)? {
            for strip in range { offsets[strip + 1] += 1; }
        }
    }
    for strip in 1..offsets.len() { offsets[strip] += offsets[strip - 1]; }
    for (index, line) in lines.iter().enumerate() {
        if let Some(range) = line_strip_range(*line, height)? {
            for strip in range {
                let position = offsets[strip] as usize;
                indices[position] = index as _;
                offsets[strip] += 1;
            }
        }
    }
    for strip in (1..offsets.len()).rev() { offsets[strip] = offsets[strip - 1]; }
    offsets[0] = 0;
    for strip in 0..offsets.len() - 1 {
        let (start, end) = (offsets[strip] as usize, offsets[strip + 1] as usize);
        indices[start..end].sort_unstable_by(|left, right| {
            let (left, right) = (lines[*left as usize], lines[*right as usize]);
            left.y0.cmp(&right.y0)
                .then_with(|| (left.y0 + left.dy as i32).cmp(&(right.y0 + right.dy as i32)))
        });
    }   Ok(FixedStripBins { offsets, indices })
}

fn retain_active_lines(lines: &[FixedLine], segments: &mut [FixedSegment],
    count: usize, top: FixedScalar) -> usize {
    let top = top.to_bits();
    let mut retained = 0;
    for index in 0..count {
        let line = lines[segments[index].line_index as usize];
        if line.y0 + line.dy as i32 > top {
            segments[retained] = segments[index];
            retained += 1;
        }
    }       retained
}

fn activate_pending_lines(lines: &[FixedLine], strip_lines: &[u32], pending: &mut usize,
    top: FixedScalar, segments: &mut [FixedSegment], active_count: &mut usize) {
    let top = top.to_bits();
    while let Some(&line_index) = strip_lines.get(*pending) {
        let line = lines[line_index as usize];
        if line.y0 > top { break; }
        if line.y0 + line.dy as i32 > top {
            segments[*active_count] =
                FixedSegment { line_index, ..FixedSegment::default() };
            *active_count += 1;
        }
        *pending += 1;
    }
}

fn next_active_slab_boundary(lines: &[FixedLine], strip_lines: &[u32], pending: usize,
    active: &[FixedSegment], top: FixedScalar, bottom: FixedScalar) -> FixedScalar {
    let (top, mut boundary) = (top.to_bits(), bottom.to_bits());
    if let Some(&index) = strip_lines.get(pending) {
        let start = lines[index as usize].y0;
        if top < start && start < boundary { boundary = start; }
    }
    for segment in active {
        let end = {
            let line = lines[segment.line_index as usize];
            line.y0 + line.dy as i32
        };
        if top < end && end < boundary { boundary = end; }
    }   FixedScalar::from_bits(boundary)
}

fn prepare_active_segments(lines: &[FixedLine], segments: &mut [FixedSegment],
    top: FixedScalar, bottom: FixedScalar) -> Result<(), FixedRasterError> {
    let (top, bottom) = (top.to_bits(), bottom.to_bits());
    for segment in segments {
        *segment = lines[segment.line_index as usize]
            .segment_in_slab(segment.line_index, top, bottom)
            .ok_or(FixedRasterError::InvalidSlabPartition)?;
    }   Ok(())
}

/// Validates and widens fixed-point edges into caller-owned raster storage.
///
/// Validation is completed before output is written, so errors never expose a
/// mixture of old and newly prepared lines.
pub fn prepare_lines(edges: &[Edge<FixedScalar>], output: &mut [FixedLine]) ->
    Result<usize, FixedRasterError> {
    for edge in edges { FixedLine::new(*edge)?; }
    if output.len() < edges.len() {
        return Err(FixedRasterError::WorkspaceTooSmall {
            kind: FixedWorkspace::Lines, required: edges.len(),
        });
    }
    for (line, edge) in output.iter_mut().zip(edges) { *line = FixedLine::new(*edge)?; }
    Ok(edges.len())
}

/// Returns the next edge-vertex boundary within a requested horizontal slab.
///
/// Repeatedly advancing `top` to the returned value partitions the slab so
/// active segments share identical top and bottom coordinates.
pub fn next_slab_boundary(lines: &[FixedLine], top: FixedScalar, bottom: FixedScalar) ->
    Result<FixedScalar, FixedRasterError> {
    let (top, bottom) = validate_slab(top, bottom)?;

    let mut boundary = bottom;
    for line in lines {
        for vertex in [line.y0, line.y0 + line.dy as i32] {
            if top < vertex && vertex < boundary { boundary = vertex; }
        }
    }   Ok(FixedScalar::from_bits(boundary))
}

/// Clips prepared lines to a horizontal slab without rounding x coordinates.
///
/// Only overlapping fragments are emitted. Capacity is checked before output
/// is modified.
pub fn collect_segments(lines: &[FixedLine], top: FixedScalar, bottom: FixedScalar,
    output: &mut [FixedSegment]) -> Result<usize, FixedRasterError> {
    let (top, bottom) = validate_slab(top, bottom)?;
    if lines.len() > u32::MAX as usize { return Err(FixedRasterError::DimensionsOverflow); }
    let required = lines.iter().enumerate()
        .filter(|(index, line)| line.segment_in_slab(*index as _, top, bottom).is_some()).count();
    if output.len() < required {
        return Err(FixedRasterError::WorkspaceTooSmall {
            kind: FixedWorkspace::Segments, required,
        });
    }

    let mut    count = 0;
    for segment in lines.iter().enumerate().filter_map(|(index, line)|
        line.segment_in_slab(index as _, top, bottom)) {
        output[count] = segment;  count += 1;
    }       Ok(count)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] struct FixedCrossing { y: i32, x: i64 }

fn next_crossing_boundary(lines: &[FixedLine], segments: &mut [FixedSegment],
    top: FixedScalar, bottom: FixedScalar) ->
    Result<(FixedScalar, bool, bool), FixedRasterError> {
    let (top, bottom) = validate_slab(top, bottom)?;
    let (mut boundary, mut snap_top, mut snap_bottom) = (bottom, false, false);
    segments.sort_unstable_by(|left, right| left.top_x.cmp_x(&right.top_x)
        .then_with(|| left.bottom_x.cmp_x(&right.bottom_x)));
    for pair in segments.windows(2) {
        if !pair[0].bottom_x.cmp_x(&pair[1].bottom_x).is_gt() { continue; }
        let (left, right) = (pair[0].line_index as usize, pair[1].line_index as usize);
        let Some(event) = crossing_event(lines[left], lines[right]) else { continue; };
        if event.y == top {
            snap_top = true;
        } else if top < event.y && event.y < boundary {
            boundary = event.y;
            snap_bottom = true;
        } else if event.y == boundary {
            snap_bottom = true;
        }
    }
    Ok((FixedScalar::from_bits(boundary), snap_top, snap_bottom))
}

fn crossing_event(left: FixedLine, right: FixedLine) -> Option<FixedCrossing> {
    let (left_dy, right_dy) = (left.dy as i128, right.dy as i128);
    let (left_c, right_c) = (
         left.x0 as i128 *  left_dy -  left.dx as i128 *  left.y0 as i128,
        right.x0 as i128 * right_dy - right.dx as i128 * right.y0 as i128,
    );
    let (mut denominator, mut numerator) = (
        left.dx as i128 * right_dy - right.dx as i128 * left_dy,
        right_c * left_dy - left_c * right_dy,
    );
    if denominator == 0 { return None; }
    if denominator < 0 { denominator = -denominator; numerator = -numerator; }

    let overlap_top = left.y0.max(right.y0) as i128;
    let overlap_bottom = (left.y0 + left.dy as i32)
        .min(right.y0 + right.dy as i32) as i128;
    if numerator <= overlap_top * denominator ||
       numerator >= overlap_bottom * denominator { return None; }

    let x_numerator = left.dx as i128 * numerator + left_c * denominator;
    let x_denominator = left_dy * denominator;
    Some(FixedCrossing {
        y: i32::try_from(round_ratio_i128(numerator, denominator)).ok()?,
        x: i64::try_from(round_ratio_i128(x_numerator, x_denominator)).ok()?,
    })
}

fn snap_crossing_events(lines: &[FixedLine], y: FixedScalar,
    segments: &mut [FixedSegment], top: bool) {
    let y = y.to_bits();
    for left in 0..segments.len() {
        for right in left + 1..segments.len() {
            let (left_line, right_line) =
                (segments[left].line_index as usize, segments[right].line_index as usize);
            let Some(event) = crossing_event(lines[left_line], lines[right_line])
                .filter(|event| event.y == y) else { continue; };
            for (index, line) in [(left, lines[left_line]), (right, lines[right_line])] {
                let intersection =
                    FixedIntersection { num: event.x, den: 1, winding: line.winding };
                if top { segments[index].top_x = intersection; }
                else   { segments[index].bottom_x = intersection; }
            }
        }
    }
}

fn validate_slab(top: FixedScalar, bottom: FixedScalar) ->
    Result<(i32, i32), FixedRasterError> {
    let (top, bottom) = (top.to_bits(), bottom.to_bits());
    if top >= bottom { return Err(FixedRasterError::InvalidSlab); }
    if [top,  bottom].iter().any(|value|
        value.unsigned_abs() > DEVICE_RAW_LIMIT as u32) {
        return Err(FixedRasterError::CoordinateOutOfRange);
    }   Ok((top, bottom))
}

/// Pairs a fully partitioned slab's directed segments into fill trapezoids.
///
/// Segments are sorted in place by their order immediately inside the slab.
/// A crossing reports `CrossingEdges`; the caller can then split the slab at
/// the exact crossing y before retrying.
pub fn collect_trapezoids(segments: &mut [FixedSegment], fill_rule: FillRule,
    output: &mut [FixedTrapezoid]) -> Result<usize, FixedRasterError> {
    let Some(first) = segments.first() else { return Ok(0); };
    if segments.iter().any(|segment|
        segment.top_y != first.top_y || segment.bottom_y != first.bottom_y) {
        return Err(FixedRasterError::InvalidSlabPartition);
    }
    segments.sort_unstable_by(|left, right| left.top_x.cmp_x(&right.top_x)
        .then_with(|| left.bottom_x.cmp_x(&right.bottom_x)));
    collect_ordered_trapezoids(segments, fill_rule, output)
}

fn collect_ordered_trapezoids(segments: &[FixedSegment], fill_rule: FillRule,
    output: &mut [FixedTrapezoid]) -> Result<usize, FixedRasterError> {
    if segments.windows(2).any(|pair| pair[0].bottom_x.cmp_x(&pair[1].bottom_x).is_gt()) {
        return Err(FixedRasterError::CrossingEdges);
    }

    let mut required = 0;
    let winding = walk_trapezoids(segments, fill_rule, |_, _| required += 1);
    if winding != 0 { return Err(FixedRasterError::UnbalancedWinding); }
    if output.len() < required {
        return Err(FixedRasterError::WorkspaceTooSmall {
            kind: FixedWorkspace::Trapezoids, required,
        });
    }

    let mut    count = 0;
    walk_trapezoids(segments, fill_rule, |left, right| {
        output[count] = FixedTrapezoid { left, right };  count += 1;
    });     Ok(count)
}

fn walk_trapezoids<F>(segments: &[FixedSegment], fill_rule: FillRule, mut emit: F) -> i32
    where F: FnMut(FixedSegment, FixedSegment) {
    let (mut winding, mut left) = (0, None);
    for segment in segments {
        let was_inside = fill_rule.contains(winding);
        winding += segment.top_x.winding as i32;
        match (was_inside, fill_rule.contains(winding), left) {
            (false, true, _) => left = Some(*segment),
            (true, false, Some(start)) => {
                if start.   top_x.cmp_x(&segment.   top_x).is_ne() ||
                   start.bottom_x.cmp_x(&segment.bottom_x).is_ne() {
                    emit(start, *segment);
                }   left = None;
            }   _ => {}
        }
    }   winding
}

/// Collects and orders the intersections of a fixed-point scanline.
///
/// Edge activation uses the same half-open `[upper.y, lower.y)` convention as
/// the floating-point reference. The caller must provide space for every
/// potentially active line; no allocation or partial output occurs on error.
pub fn collect_intersections(lines: &[FixedLine], y: FixedScalar,
    output: &mut [FixedIntersection]) -> Result<usize, FixedRasterError> {
    let required = lines.iter().filter(|line| line.contains_y(y)).count();
    if output.len() < required {
        return Err(FixedRasterError::WorkspaceTooSmall {
            kind: FixedWorkspace::Intersections, required,
        });
    }

    let mut count = 0;
    for line in lines.iter().filter(|line| line.contains_y(y)) {
        output[count] = line.intersection(y);   count += 1;
    }
    output[..count].sort_unstable_by(FixedIntersection::cmp_x);
    Ok(count)
}

/// Converts ordered crossing events into exact rational fill spans.
///
/// Events at the same x coordinate are combined before the fill state changes,
/// preventing shared vertices and coincident edges from creating empty spans.
/// The output remains untouched if validation or capacity checking fails.
pub fn collect_spans(intersections: &[FixedIntersection], fill_rule: FillRule,
    output: &mut [FixedSpan]) -> Result<usize, FixedRasterError> {
    if intersections.windows(2).any(|pair| pair[0].cmp_x(&pair[1]).is_gt()) {
        return Err(FixedRasterError::InvalidIntersectionOrder);
    }
    let mut required = 0;
    let final_winding = walk_spans(intersections, fill_rule, |_, _| required += 1);
    if  final_winding != 0 { return Err(FixedRasterError::UnbalancedWinding); }
    if output.len() < required {
        return Err(FixedRasterError::WorkspaceTooSmall {
            kind: FixedWorkspace::Spans, required,
        });
    }

    let mut    count = 0;
    walk_spans(intersections, fill_rule, |from, to| {
        output[count] = FixedSpan { from, to };  count += 1;
    });     Ok(count)
}

fn walk_spans<F>(intersections: &[FixedIntersection], fill_rule: FillRule,
    mut emit: F) -> i32 where F: FnMut(FixedIntersection, FixedIntersection) {
    let (mut index, mut winding, mut start) = (0, 0, None);
    while index < intersections.len() {
        let crossing = intersections[index];
        let was_inside = fill_rule.contains(winding);
        while index < intersections.len()
            && crossing.cmp_x(&intersections[index]).is_eq() {
            winding += intersections[index].winding as i32;
            index += 1;
        }
        match (was_inside, fill_rule.contains(winding), start) {
            (false, true, _)       => start = Some(crossing),
            (true, false, Some(x)) => { emit(x, crossing); start = None; }
            _ => {}
        }
    }       winding
}

#[cfg(test)] #[path = "fixed_tests.rs"] mod tests;
