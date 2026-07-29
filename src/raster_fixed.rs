//! Widened arithmetic primitives for the Q24.8 fixed-point raster backend.

use core::cmp::Ordering;
use crate::{edge::Edge, geometry::{FixedScalar, Point}, raster::{CoverageSink, FillRule}};

/// Accepted Q24.8 raw-coordinate magnitude for the fixed rasterizer.
///
/// This corresponds to ±2,097,152 device units and leaves enough headroom for
/// every line-intersection multiply-add to remain in `i64`.
pub const   SUBPIXEL_SCALE: u32 = 1 << 8;
pub const DEVICE_RAW_LIMIT: i32 = 1 << 29;
const PIXEL_AREA_TWICE: u64 = 2 * SUBPIXEL_SCALE as u64 * SUBPIXEL_SCALE as u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum FixedRasterError {
    CoordinateOutOfRange, CrossingEdges, DimensionsOverflow, InvalidEdge, InvalidIntersectionOrder,
    InvalidSlab, InvalidSlabPartition, InvalidTrapezoid, UnbalancedWinding,
    WorkspaceTooSmall { kind: FixedWorkspace, required: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedWorkspace { Lines, Segments, Trapezoids, RowArea, Intersections, Spans }

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

    fn segment_in_slab(&self, top: i32, bottom: i32) -> Option<FixedSegment> {
        let (line_top, line_bottom) = (self.y0, self.y0 + self.dy as i32);
        let (top_y, bottom_y) = (top.max(line_top), bottom.min(line_bottom));
        (top_y < bottom_y).then(|| FixedSegment { top_y, bottom_y,
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
pub struct FixedSegment { top_y: i32, bottom_y: i32,
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
}

/// Rasterizes prepared fixed-point lines into anti-aliased coverage runs.
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

    for y in first_row..last_row {
        let row = &mut workspace.row_area[..width_usize];  row.fill(0);
        let (mut top, bottom) = (FixedScalar::from_bits(extent(y)     as i32),
                                 FixedScalar::from_bits(extent(y + 1) as i32));
        while top < bottom {
            let next = next_slab_boundary(lines, top, bottom)
                .map_err(FixedRenderError::Raster)?;
            let segment_count = collect_segments(lines, top, next, workspace.segments)
                .map_err(FixedRenderError::Raster)?;
            let trapezoid_count = collect_trapezoids(
                &mut workspace.segments[..segment_count], fill_rule, workspace.trapezoids,
            ).map_err(FixedRenderError::Raster)?;
            for trapezoid in workspace.trapezoids[..trapezoid_count].iter().copied() {
                accumulate_trapezoid_row(trapezoid, width, y, row)
                    .map_err(FixedRenderError::Raster)?;
            }   top = next;
        }
        emit_area_runs(row, y, sink).map_err(FixedRenderError::Sink)?;
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
    }
    Ok(FixedScalar::from_bits(boundary))
}

/// Clips prepared lines to a horizontal slab without rounding x coordinates.
///
/// Only overlapping fragments are emitted. Capacity is checked before output
/// is modified.
pub fn collect_segments(lines: &[FixedLine], top: FixedScalar, bottom: FixedScalar,
    output: &mut [FixedSegment]) -> Result<usize, FixedRasterError> {
    let (top, bottom) = validate_slab(top, bottom)?;
    let required = lines.iter()
        .filter(|line| line.segment_in_slab(top, bottom).is_some()).count();
    if output.len() < required {
        return Err(FixedRasterError::WorkspaceTooSmall {
            kind: FixedWorkspace::Segments, required,
        });
    }

    let mut    count = 0;
    for segment in lines.iter().filter_map(|line| line.segment_in_slab(top, bottom)) {
        output[count] = segment;  count += 1;
    }       Ok(count)
}

fn validate_slab(top: FixedScalar, bottom: FixedScalar) ->
    Result<(i32, i32), FixedRasterError> {
    let (top, bottom) = (top.to_bits(), bottom.to_bits());
    if top >= bottom { return Err(FixedRasterError::InvalidSlab); }
    if [top,  bottom].iter().any(|value|
        value.unsigned_abs() > DEVICE_RAW_LIMIT as u32) {
        return Err(FixedRasterError::CoordinateOutOfRange);
    }
    Ok((top, bottom))
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

#[cfg(test)] mod tests { use super::*;
    use alloc::{vec, vec::Vec};
    use core::convert::Infallible;
    use crate::analytic::{AnalyticIntersection, AnalyticWorkspace, rasterize_edges_analytic};

    fn fixed(value: f32) -> FixedScalar { FixedScalar::from_num(value) }

    fn render(edges: &[Edge<FixedScalar>], width: usize, height: usize,
        fill_rule: FillRule) -> Vec<u8> {
        let mut lines = vec![FixedLine::default(); edges.len()];
        prepare_lines(edges, &mut lines).unwrap();
        let (mut segments, mut trapezoids, mut row_area) = (
            vec![FixedSegment::default(); lines.len()],
            vec![FixedTrapezoid::default(); lines.len().div_ceil(2)],
            vec![0; width],
        );
        let mut pixels = vec![0; width * height];
        rasterize_lines(&lines, width as _, height as _, fill_rule,
            &mut FixedRasterWorkspace { segments: &mut segments,
                trapezoids: &mut trapezoids, row_area: &mut row_area,
            }, &mut |x, y, coverage| {
                pixels[y as usize * width + x as usize] = coverage;
                Ok::<_, Infallible>(())
            }).unwrap();
        pixels
    }

    fn render_analytic(edges: &[Edge], width: usize, height: usize) -> Vec<u8> {
        let (mut pixels, mut row) = (vec![0; width * height], vec![0.0; width]);
        let mut intersections = vec![AnalyticIntersection::default(); edges.len()];
        rasterize_edges_analytic(edges, width as _, height as _, FillRule::NonZero,
            &mut AnalyticWorkspace {
                intersections: &mut intersections, row_coverage: &mut row,
            }, &mut |x, y, coverage| {
                pixels[y as usize * width + x as usize] = coverage;
                Ok::<_, Infallible>(())
            }).unwrap();
        pixels
    }

    #[test] fn diagonal_intersection_is_exact_in_raw_subpixels() {
        let edge = Edge::from_line((fixed(0.0), fixed(0.0)).into(),
                                   (fixed(1.0), fixed(1.0)).into()).unwrap();
        let intersection = FixedLine::new(edge).unwrap().intersection(fixed(0.5));
        assert_eq!(intersection.floor_raw(), 128);
    }

    #[test] fn fixed_rasterizer_renders_aligned_and_fractional_rectangles() {
        let rectangle = |left, right| [
            Edge { upper: (fixed(left), fixed(0.0)).into(),
                   lower: (fixed(left), fixed(1.0)).into(), winding: 1,
            },
            Edge { upper: (fixed(right), fixed(0.0)).into(),
                   lower: (fixed(right), fixed(1.0)).into(), winding: -1,
            },
        ];
        assert_eq!(render(&rectangle(1.0, 3.0), 4, 1, FillRule::NonZero), [0, 255, 255, 0]);
        assert_eq!(render(&rectangle(0.5, 1.5), 2, 1, FillRule::NonZero), [128, 128]);
    }

    #[test] fn fixed_rasterizer_supports_both_fill_rules_end_to_end() {
        let edge = |x, winding| Edge {
            upper: (fixed(x), fixed(0.0)).into(),
            lower: (fixed(x), fixed(1.0)).into(), winding,
        };
        let edges = [edge(0.0, 1), edge(4.0, -1), edge(1.0, 1), edge(3.0, -1)];
        assert_eq!(render(&edges, 4, 1, FillRule::NonZero), [255; 4]);
        assert_eq!(render(&edges, 4, 1, FillRule::EvenOdd), [255, 0, 0, 255]);
    }

    #[test] fn fixed_triangles_track_the_f32_analytic_reference() {
        let mut state = 0x8f31_7a2d_u32;
        let mut random_raw = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state % (7 * SUBPIXEL_SCALE)) as i32 - SUBPIXEL_SCALE as i32
        };
        for case in 0..512 {
            let points = [
                (FixedScalar::from_bits(random_raw()),
                 FixedScalar::from_bits(random_raw())).into(),
                (FixedScalar::from_bits(random_raw()),
                 FixedScalar::from_bits(random_raw())).into(),
                (FixedScalar::from_bits(random_raw()),
                 FixedScalar::from_bits(random_raw())).into(),
            ];
            let mut fixed_edges = Vec::new();
            for index in 0..3 {
                if let Some(edge) = Edge::from_line(points[index], points[(index + 1) % 3]) {
                    fixed_edges.push(edge);
                }
            }
            let float_edges: Vec<Edge> = fixed_edges.iter().map(|edge| Edge {
                upper: (edge.upper.x.to_num(), edge.upper.y.to_num()).into(),
                lower: (edge.lower.x.to_num(), edge.lower.y.to_num()).into(),
                winding: edge.winding,
            }).collect();
            let (fixed_pixels, float_pixels) = (
                render(&fixed_edges, 6, 6, FillRule::NonZero),
                render_analytic(&float_edges, 6, 6),
            );
            for (pixel, (fixed, reference)) in
                fixed_pixels.iter().zip(&float_pixels).enumerate() {
                assert!(fixed.abs_diff(*reference) <= 2,
                    "case {case}, pixel {pixel}: fixed={fixed}, f32={reference}");
            }
        }
    }

    #[test] fn rational_order_handles_negative_values_and_different_denominators() {
        let  left = FixedIntersection { num: -3, den: 2, winding: 1 };
        let right = FixedIntersection { num: -4, den: 3, winding: -1 };
        assert_eq!(left.floor_raw(), -2);
        assert_eq!(left.cmp_x(&right), Ordering::Less);

        let half = FixedIntersection { num: 1, den: 2, winding: 1 };
        let same = FixedIntersection { num: 2, den: 4, winding: -1 };
        assert_eq!(half.cmp_x(&same), Ordering::Equal);
    }

    #[test] fn rational_rounding_is_symmetric_at_half_subpixels() {
        let value = |num| FixedIntersection { num, den: 2, winding: 1 };
        assert_eq!(value( 1).round_raw(),  1);
        assert_eq!(value(-1).round_raw(), -1);
        assert_eq!(value( 3).round_raw(),  2);
        assert_eq!(value(-3).round_raw(), -2);
        assert_eq!(value( 2).round_raw(),  1);
    }

    #[test] fn coordinate_limit_is_explicit() {
        let outside = FixedScalar::from_bits(DEVICE_RAW_LIMIT + 1);
        let edge = Edge::from_line((FixedScalar::ZERO, FixedScalar::ZERO).into(),
            (outside, FixedScalar::ONE).into()).unwrap();
        assert_eq!(FixedLine::new(edge), Err(FixedRasterError::CoordinateOutOfRange));
    }

    #[test] fn manually_constructed_invalid_edges_are_rejected() {
        let edge = Edge {
            upper: (FixedScalar::ZERO, FixedScalar::ONE).into(),
            lower: (FixedScalar::ONE, FixedScalar::ZERO).into(), winding: 1,
        };
        assert_eq!(FixedLine::new(edge), Err(FixedRasterError::InvalidEdge));
    }

    #[test] fn line_preparation_is_bounded_and_transactional() {
        let edge = |x, winding| Edge {
            upper: (fixed(x), fixed(0.0)).into(),
            lower: (fixed(x), fixed(1.0)).into(), winding,
        };
        let sentinel = FixedLine::new(edge(7.0, 1)).unwrap();
        let mut output = [sentinel; 2];

        assert_eq!(prepare_lines(&[edge(0.0, 1), edge(1.0, -1)], &mut output), Ok(2));
        assert_eq!(output[0].intersection(fixed(0.5)).floor_raw(), 0);
        output = [sentinel; 2];
        assert_eq!(prepare_lines(&[edge(0.0, 1), edge(1.0, 0)], &mut output),
            Err(FixedRasterError::InvalidEdge));
        assert_eq!(output, [sentinel; 2]);
        assert_eq!(prepare_lines(&[edge(0.0, 1), edge(1.0, -1)], &mut output[..1]),
            Err(FixedRasterError::WorkspaceTooSmall {
                kind: FixedWorkspace::Lines, required: 2,
            }));
        assert_eq!(output, [sentinel; 2]);
    }

    #[test] fn slab_boundaries_advance_through_edge_vertices() {
        let edge = |x, top, bottom, winding| Edge {
            upper: (fixed(x), fixed(top)).into(),
            lower: (fixed(x), fixed(bottom)).into(), winding,
        };
        let edges = [
            edge(0.0, 0.0, 2.0, 1), edge(2.0, 0.0, 2.0, -1),
            edge(0.5, 0.5, 1.5, 1), edge(1.5, 0.5, 1.5, -1),
        ];
        let mut lines = [FixedLine::default(); 4];
        prepare_lines(&edges, &mut lines).unwrap();

        let first = next_slab_boundary(&lines, fixed(0.0), fixed(2.0)).unwrap();
        let second = next_slab_boundary(&lines, first, fixed(2.0)).unwrap();
        let third = next_slab_boundary(&lines, second, fixed(2.0)).unwrap();
        assert_eq!((first, second, third), (fixed(0.5), fixed(1.5), fixed(2.0)));

        let mut segments = [FixedSegment::default(); 4];
        let count = collect_segments(&lines, first, second, &mut segments).unwrap();
        assert_eq!(count, 4);
        assert!(segments[..count].iter().all(|segment|
            segment.top_y() == first && segment.bottom_y() == second));
        let mut trapezoids = [FixedTrapezoid::default(); 2];
        assert_eq!(collect_trapezoids(&mut segments[..count], FillRule::EvenOdd,
            &mut trapezoids), Ok(2));
    }

    #[test] fn slab_clipping_preserves_exact_boundary_intersections() {
        let line = FixedLine::new(Edge::from_line(
            (fixed(-1.0), fixed(-1.0)).into(),
            (fixed(2.0),  fixed(2.0) ).into()).unwrap()).unwrap();
        let mut segments = [FixedSegment::default(); 1];

        assert_eq!(collect_segments(&[line], fixed(0.0), fixed(1.0), &mut segments), Ok(1));
        assert_eq!((segments[0].top_y(), segments[0].bottom_y()), (fixed(0.0), fixed(1.0)));
        assert_eq!((segments[0].top_x.floor_raw(),
                    segments[0].bottom_x.floor_raw()), (0, 256));
        assert_eq!( segments[0].height_raw(), 256);
        assert_eq!(collect_segments(&[line], fixed(3.0), fixed(4.0), &mut segments), Ok(0));
    }

    #[test] fn slab_errors_do_not_modify_output() {
        let sentinel = FixedSegment { top_y: 7, bottom_y: 9,
               top_x: FixedIntersection::default(),
            bottom_x: FixedIntersection::default(),
        };
        let line = FixedLine::new(Edge::from_line(
            (fixed(0.0), fixed(0.0)).into(),
            (fixed(1.0), fixed(1.0)).into()).unwrap()).unwrap();
        let mut output = [sentinel];

        assert_eq!(collect_segments(&[line], fixed(1.0), fixed(1.0), &mut output),
            Err(FixedRasterError::InvalidSlab));
        assert_eq!(output, [sentinel]);
        assert_eq!(collect_segments(&[line], fixed(0.0), fixed(1.0), &mut []),
            Err(FixedRasterError::WorkspaceTooSmall {
                kind: FixedWorkspace::Segments, required: 1,
            }));
        assert_eq!(output, [sentinel]);
    }

    #[test] fn slab_segments_form_rectangular_and_triangular_trapezoids() {
        let segment = |top_x, bottom_x, winding| FixedSegment { top_y: 0, bottom_y: 256,
               top_x: FixedIntersection { num:    top_x, den: 1, winding },
            bottom_x: FixedIntersection { num: bottom_x, den: 1, winding },
        };
        let mut output = [FixedTrapezoid::default(); 1];
        let mut rectangle = [segment(0, 0, 1), segment(256, 256, -1)];
        assert_eq!(collect_trapezoids(&mut rectangle, FillRule::NonZero, &mut output), Ok(1));
        assert_eq!((output[0].left.top_x.floor_raw(), output[0].right.top_x.floor_raw()),
            (0, 256));

        let mut triangle = [segment(128, 256, -1), segment(128, 0, 1)];
        assert_eq!(collect_trapezoids(&mut triangle, FillRule::NonZero, &mut output), Ok(1));
        assert_eq!((output[0].left.bottom_x.floor_raw(),
                    output[0].right.bottom_x.floor_raw()), (0, 256));
    }

    #[test] fn trapezoid_area_quantizes_full_and_half_pixels_exactly() {
        let segment = |top_x, bottom_x, winding| FixedSegment { top_y: 0, bottom_y: 256,
               top_x: FixedIntersection { num:    top_x, den: 1, winding },
            bottom_x: FixedIntersection { num: bottom_x, den: 1, winding },
        };
        let rectangle = FixedTrapezoid {
            left: segment(0, 0, 1), right: segment(256, 256, -1),
        };
        let triangle = FixedTrapezoid {
            left: segment(128, 0, 1), right: segment(128, 256, -1),
        };
        assert_eq!(rectangle.area_twice_raw(), Ok(PIXEL_AREA_TWICE));
        assert_eq!(quantize_area_coverage(rectangle.area_twice_raw().unwrap()), 255);
        assert_eq!(triangle.area_twice_raw(), Ok(PIXEL_AREA_TWICE / 2));
        assert_eq!(quantize_area_coverage(triangle.area_twice_raw().unwrap()), 128);
        assert_eq!(quantize_area_coverage(PIXEL_AREA_TWICE * 2), 255);

        let inverted = FixedTrapezoid { left: rectangle.right, right: rectangle.left };
        assert_eq!(inverted.area_twice_raw(), Err(FixedRasterError::InvalidTrapezoid));
    }

    #[test] fn trapezoid_extracts_only_guaranteed_full_pixel_runs() {
        let segment = |top_y, bottom_y, top_x, bottom_x, winding| FixedSegment {
               top_x: FixedIntersection { num:    top_x, den: 1, winding },
            bottom_x: FixedIntersection { num: bottom_x, den: 1, winding },
            top_y, bottom_y,
        };
        let aligned = FixedTrapezoid {
             left: segment(0, 256, 256, 256, 1),
            right: segment(0, 256, 1024, 1024, -1),
        };
        assert_eq!(aligned.full_pixel_range(8), Ok(1..4));

        let slanted = FixedTrapezoid {
             left: segment(0, 256, 128, 256, 1),
            right: segment(0, 256, 896, 768, -1),
        };
        assert_eq!(slanted.full_pixel_range(8), Ok(1..3));

        let clipped = FixedTrapezoid {
             left: segment(0, 256, -512, -256, 1),
            right: segment(0, 256, 512, 768, -1),
        };
        assert_eq!(clipped.full_pixel_range(2), Ok(0..2));

        let partial_height = FixedTrapezoid {
             left: segment(0, 128, 0, 0, 1),
            right: segment(0, 128, 512, 512, -1),
        };
        assert_eq!(partial_height.full_pixel_range(8), Ok(0..0));
    }

    #[test] fn trapezoid_clips_boundary_pixels_without_allocation() {
        let segment = |top_y, bottom_y, top_x, bottom_x, winding| FixedSegment {
               top_x: FixedIntersection { num:    top_x, den: 1, winding },
            bottom_x: FixedIntersection { num: bottom_x, den: 1, winding },
            top_y, bottom_y,
        };
        let centered = FixedTrapezoid {
             left: segment(0, 256, 128, 128, 1),
            right: segment(0, 256, 384, 384, -1),
        };
        assert_eq!(centered.pixel_area_twice_raw(0, 0), Ok(PIXEL_AREA_TWICE / 2));
        assert_eq!(centered.pixel_area_twice_raw(1, 0), Ok(PIXEL_AREA_TWICE / 2));
        assert_eq!(centered.pixel_area_twice_raw(2, 0), Ok(0));

        let diagonal = FixedTrapezoid {
             left: segment(0, 256, 0, 256, 1),
            right: segment(0, 256, 256, 256, -1),
        };
        let area = diagonal.pixel_area_twice_raw(0, 0).unwrap();
        assert_eq!(area, PIXEL_AREA_TWICE / 2);
        assert_eq!(quantize_area_coverage(area), 128);

        let partial_height = FixedTrapezoid {
             left: segment(128, 256, 0, 0, 1),
            right: segment(128, 256, 256, 256, -1),
        };
        assert_eq!(partial_height.pixel_area_twice_raw(0, 0), Ok(PIXEL_AREA_TWICE / 2));
        assert_eq!(partial_height.pixel_area_twice_raw(0, 1),
            Err(FixedRasterError::InvalidSlabPartition));
    }

    #[test] fn slab_areas_accumulate_before_quantization_and_emit_as_runs() {
        let segment = |top_y, bottom_y, x, winding| FixedSegment { top_y, bottom_y,
               top_x: FixedIntersection { num: x, den: 1, winding },
            bottom_x: FixedIntersection { num: x, den: 1, winding },
        };
        let trapezoid = |top_y, bottom_y| FixedTrapezoid {
             left: segment(top_y, bottom_y, 0, 1),
            right: segment(top_y, bottom_y, 512, -1),
        };
        let mut row = [0; 3];
        accumulate_trapezoid_row(trapezoid(0, 128), 3, 0, &mut row).unwrap();
        accumulate_trapezoid_row(trapezoid(128, 256), 3, 0, &mut row).unwrap();
        assert_eq!(row, [PIXEL_AREA_TWICE, PIXEL_AREA_TWICE, 0]);

        #[derive(Default)] struct Runs(alloc::vec::Vec<(u32, u32, u32, u8)>);
        impl CoverageSink for Runs {    type Error = Infallible;
            fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
                Result<(), Self::Error> {
                self.0.push((x, y, len, coverage));  Ok(())
            }
        }
        let mut runs = Runs::default();
        emit_area_runs(&row, 0, &mut runs).unwrap();
        assert_eq!(runs.0, [(0, 0, 2, 255)]);
    }

    #[test] fn row_accumulation_combines_boundary_and_interior_pixels() {
        let segment = |x, winding| FixedSegment { top_y: 0, bottom_y: 256,
               top_x: FixedIntersection { num: x, den: 1, winding },
            bottom_x: FixedIntersection { num: x, den: 1, winding },
        };
        let trapezoid = FixedTrapezoid { left: segment(128, 1), right: segment(896, -1) };
        let mut row = [0; 4];
        accumulate_trapezoid_row(trapezoid, 4, 0, &mut row).unwrap();
        assert_eq!(row, [PIXEL_AREA_TWICE / 2, PIXEL_AREA_TWICE,
                         PIXEL_AREA_TWICE, PIXEL_AREA_TWICE / 2]);
        assert_eq!(row.map(quantize_area_coverage), [128, 255, 255, 128]);
        assert_eq!(accumulate_trapezoid_row(trapezoid, 4, 0, &mut row[..3]),
            Err(FixedRasterError::WorkspaceTooSmall {
                kind: FixedWorkspace::RowArea, required: 4,
            }));
    }

    #[test] fn trapezoid_construction_rejects_crossings_and_unpartitioned_slabs() {
        let segment = |top_y, bottom_y, top_x, bottom_x, winding| FixedSegment {
               top_x: FixedIntersection { num:    top_x, den: 1, winding },
            bottom_x: FixedIntersection { num: bottom_x, den: 1, winding },
            top_y, bottom_y,
        };
        let mut crossing = [ segment(0, 256, 0, 256, 1), segment(0, 256, 256, 0, -1) ];
        assert_eq!(collect_trapezoids(&mut crossing, FillRule::NonZero, &mut []),
            Err(FixedRasterError::CrossingEdges));

        let mut unpartitioned = [segment(0, 128, 0, 0, 1), segment(0, 256, 256, 256, -1)];
        assert_eq!(collect_trapezoids(&mut unpartitioned, FillRule::NonZero, &mut []),
            Err(FixedRasterError::InvalidSlabPartition));
    }

    #[test] fn scanline_collection_is_half_open_sorted_and_bounded() {
        let line = |from, to| FixedLine::new(Edge::from_line(from, to).unwrap()).unwrap();
        let lines = [
            line((fixed(2.0), fixed(0.0)).into(), (fixed(1.0), fixed(1.0)).into()),
            line((fixed(0.0), fixed(0.0)).into(), (fixed(0.0), fixed(2.0)).into()),
        ];
        let mut intersections = [FixedIntersection::default(); 2];

        assert_eq!(collect_intersections(&lines, fixed(0.5), &mut intersections), Ok(2));
        assert_eq!(intersections.map(FixedIntersection::floor_raw), [0, 384]);
        assert_eq!(collect_intersections(&lines, fixed(1.0), &mut intersections), Ok(1));
        assert_eq!(intersections[0].floor_raw(), 0);
        assert_eq!(collect_intersections(&lines, fixed(0.5), &mut intersections[..1]),
            Err(FixedRasterError::WorkspaceTooSmall {
                kind: FixedWorkspace::Intersections, required: 2,
            }));
    }

    #[test] fn crossing_events_form_exact_spans_for_both_fill_rules() {
        let crossing = |x, winding| FixedIntersection { num: x, den: 1, winding, };
        let intersections = [crossing(0, 1), crossing(1, 1),
            crossing(2, -1), crossing(3, -1)];
        let mut spans = [FixedSpan::default(); 2];

        assert_eq!(collect_spans(&intersections, FillRule::NonZero, &mut spans), Ok(1));
        assert_eq!((spans[0].from.floor_raw(), spans[0].to.floor_raw()), (0, 3));
        assert_eq!(collect_spans(&intersections, FillRule::EvenOdd, &mut spans), Ok(2));
        assert_eq!(spans[..2].iter().map(|span|
            (span.from.floor_raw(), span.to.floor_raw())).collect::<alloc::vec::Vec<_>>(),
            [(0, 1), (2, 3)]);
    }

    #[test] fn coincident_crossings_are_grouped_and_errors_do_not_write_output() {
        let crossing = |x, winding| FixedIntersection { num: x, den: 1, winding };
        let mut output = [FixedSpan { from: crossing(7, 0), to: crossing(9, 0) }];
        let sentinel = output;
        assert_eq!(collect_spans(&[crossing(0, 1), crossing(0, -1)],
            FillRule::NonZero, &mut output), Ok(0));
        assert_eq!(output, sentinel);

        assert_eq!(collect_spans(&[crossing(1, 1), crossing(0, -1)],
            FillRule::NonZero, &mut output), Err(FixedRasterError::InvalidIntersectionOrder));
        assert_eq!(output, sentinel);
        assert_eq!(collect_spans(&[crossing(0, 1), crossing(1, -1), crossing(2, 1),
            crossing(3, -1)], FillRule::EvenOdd, &mut []),
            Err(FixedRasterError::WorkspaceTooSmall {
                kind: FixedWorkspace::Spans, required: 2,
            }));
    }
}
