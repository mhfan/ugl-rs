//! Allocation-free dash decomposition for flattened `f32` contours.

use crate::geometry::{Point, Scalar};
#[cfg(feature = "fixed")]
use crate::{geometry::{FIXED_DEVICE_RAW_LIMIT, FixedScalar}, math::integer_sqrt_u64};

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum DashPatternError {
    Empty, NonFiniteLength, NonPositiveLength, NonFinitePhase,
    CycleOverflow, SlotCountOverflow,
}

/// Validated alternating on/off lengths and starting phase.
#[derive(Clone, Copy, Debug)] pub struct DashPattern<'a> {
    lengths: &'a [f32], phase: f32, cycle: f32, slots: usize,
}

impl<'a> DashPattern<'a> {
    pub fn new(lengths: &'a [f32], phase: f32) -> Result<Self, DashPatternError> {
        if lengths.is_empty() { return Err(DashPatternError::Empty); }
        if !phase.is_finite() { return Err(DashPatternError::NonFinitePhase); }
        let mut cycle = 0.0;
        for &length in lengths {
            if !length.is_finite() { return Err(DashPatternError::NonFiniteLength); }
            if length <= 0.0 { return Err(DashPatternError::NonPositiveLength); }
            cycle += length;
        }
        let slots = if lengths.len() & 1 == 0 { lengths.len() } else {
            lengths.len().checked_mul(2).ok_or(DashPatternError::SlotCountOverflow)?
        };
        if slots != lengths.len() { cycle *= 2.0; }
        if !cycle.is_finite() { return Err(DashPatternError::CycleOverflow); }
        let phase = libm::fmodf(phase, cycle);
        Ok(Self { lengths, phase: if phase < 0.0 { phase + cycle } else { phase },
            cycle, slots })
    }

    pub fn lengths(&self) -> &'a [f32] { self.lengths }
    pub fn phase(&self) -> f32 { self.phase }
    pub fn cycle(&self) -> f32 { self.cycle }

    fn initial_state(self) -> DashState {
        let (mut index, mut phase) = (0, self.phase);
        while phase >= self.length(index) {
            phase -= self.length(index);
            index = self.next(index);
        }
        DashState { index, remaining: self.length(index) - phase }
    }

    fn length(self, index: usize) -> f32 { self.lengths[index % self.lengths.len()] }
    fn next(self, index: usize) -> usize {
        if index + 1 == self.slots { 0 } else { index + 1 }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] pub struct DashContour {
    start: u32, len: u32, closed: bool,
}

impl DashContour {
    pub fn len(self) -> usize { self.len as _ }
    pub fn is_empty(self) -> bool { self.len == 0 }
    pub fn is_closed(self) -> bool { self.closed }
}

pub struct DashWorkspace<'a, T = Scalar> {
    pub points: &'a mut [Point<T>],
    pub contours: &'a mut [DashContour],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum DashError {
    NonFinitePoint,
    #[cfg(feature = "fixed")] CoordinateOutOfRange,
    PointCapacity { needed_at_least: usize },
    ContourCapacity { needed_at_least: usize },
    IndexOverflow,
}

#[derive(Debug)] pub struct DashedPath<'a, T = Scalar> {
    points: &'a [Point<T>], contours: &'a [DashContour],
}

impl<'a, T> DashedPath<'a, T> {
    pub fn contours(&self) -> impl ExactSizeIterator<Item = (&'a [Point<T>], bool)> + 'a {
        self.contours.iter().map(|contour| {
            let start = contour.start as usize;
            (&self.points[start..start + contour.len()], contour.is_closed())
        })
    }
}

#[derive(Clone, Copy)] struct DashState { index: usize, remaining: f32 }

/// Decomposes one flattened contour into open on-dash polylines.
///
/// Closed contours continue through their closing segment. When an on interval
/// crosses the closure seam, its last and first pieces are merged so the seam
/// receives a join rather than two caps.
pub fn dash_polyline<'a>(points: &[Point], closed: bool, pattern: DashPattern<'_>,
    workspace: &'a mut DashWorkspace<'_>) -> Result<DashedPath<'a>, DashError> {
    if points.iter().any(|point| !point.x.is_finite() || !point.y.is_finite()) {
        return Err(DashError::NonFinitePoint);
    }
    let mut writer = DashWriter {
        points: workspace.points, contours: workspace.contours,
        point_len: 0, contour_len: 0, current_start: None,
    };
    let Some(&first) = points.first() else { return Ok(writer.finish()); };
    let mut state = pattern.initial_state();
    if points.len() == 1 {
        if state.index & 1 == 0 {
            writer.begin(first)?;
            writer.end()?;
        }
        return Ok(writer.finish());
    }

    let (subpath_point_start, subpath_contour_start) = (writer.point_len, writer.contour_len);
    let starts_on = state.index & 1 == 0;
    let segment_count = points.len() - 1 + usize::from(closed);
    for index in 0..segment_count {
        let from = points[index % points.len()];
        let to = points[(index + 1) % points.len()];
        dash_segment(from, to, pattern, &mut state, &mut writer)?;
    }
    if writer.current_start.is_some() { writer.end()?; }
    if closed && starts_on {
        writer.merge_closure(subpath_point_start, subpath_contour_start)?;
    }
    Ok(writer.finish())
}

fn dash_segment(from: Point, to: Point, pattern: DashPattern<'_>,
    state: &mut DashState, writer: &mut DashWriter<'_>) -> Result<(), DashError> {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = libm::sqrtf(dx * dx + dy * dy);
    if !length.is_finite() { return Err(DashError::NonFinitePoint); }
    if length == 0.0 { return Ok(()); }
    let (unit_x, unit_y) = (dx / length, dy / length);
    let (mut current, mut left) = (from, length);
    while left > 0.0 {
        let ends_dash = state.remaining <= left;
        let step = state.remaining.min(left);
        let endpoint = if step == left { to } else {
            (current.x + unit_x * step, current.y + unit_y * step).into()
        };
        if state.index & 1 == 0 {
            if writer.current_start.is_none() { writer.begin(current)?; }
            writer.point(endpoint)?;
        }
        current = endpoint;
        left = if step == left { 0.0 } else { left - step };
        state.remaining = if ends_dash { 0.0 } else { state.remaining - step };
        if ends_dash {
            if state.index & 1 == 0 { writer.end()?; }
            state.index = pattern.next(state.index);
            state.remaining = pattern.length(state.index);
        }
    }
    Ok(())
}

#[cfg(feature = "fixed")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum FixedDashPatternError {
    Empty, NonPositiveLength, CycleOverflow, SlotCountOverflow,
}

#[cfg(feature = "fixed")]
#[derive(Clone, Copy, Debug)] pub struct FixedDashPattern<'a> {
    lengths: &'a [FixedScalar], phase: i32, cycle: i32, slots: usize,
}

#[cfg(feature = "fixed")] impl<'a> FixedDashPattern<'a> {
    pub fn new(lengths: &'a [FixedScalar], phase: FixedScalar) ->
        Result<Self, FixedDashPatternError> {
        if lengths.is_empty() { return Err(FixedDashPatternError::Empty); }
        let mut cycle = 0_i64;
        for length in lengths {
            if *length <= FixedScalar::ZERO {
                return Err(FixedDashPatternError::NonPositiveLength);
            }
            cycle = cycle.checked_add(length.to_bits() as _)
                .ok_or(FixedDashPatternError::CycleOverflow)?;
        }
        let slots = if lengths.len() & 1 == 0 { lengths.len() } else {
            lengths.len().checked_mul(2)
                .ok_or(FixedDashPatternError::SlotCountOverflow)?
        };
        if slots != lengths.len() {
            cycle = cycle.checked_mul(2).ok_or(FixedDashPatternError::CycleOverflow)?;
        }
        let cycle = i32::try_from(cycle).map_err(|_| FixedDashPatternError::CycleOverflow)?;
        Ok(Self { lengths, phase: phase.to_bits().rem_euclid(cycle), cycle, slots })
    }

    pub fn lengths(&self) -> &'a [FixedScalar] { self.lengths }
    pub fn phase(&self) -> FixedScalar { FixedScalar::from_bits(self.phase) }
    pub fn cycle(&self) -> FixedScalar { FixedScalar::from_bits(self.cycle) }

    fn initial_state(self) -> FixedDashState {
        let (mut index, mut phase) = (0, self.phase);
        while phase >= self.length(index) {
            phase -= self.length(index);
            index = self.next(index);
        }
        FixedDashState { index, remaining: self.length(index) - phase }
    }

    fn length(self, index: usize) -> i32 {
        self.lengths[index % self.lengths.len()].to_bits()
    }
    fn next(self, index: usize) -> usize {
        if index + 1 == self.slots { 0 } else { index + 1 }
    }
}

#[cfg(feature = "fixed")]
#[derive(Clone, Copy)] struct FixedDashState { index: usize, remaining: i32 }

/// Fixed-point counterpart of [`dash_polyline`] with integer distance state.
#[cfg(feature = "fixed")]
pub fn dash_polyline_fixed<'a>(points: &[Point<FixedScalar>], closed: bool,
    pattern: FixedDashPattern<'_>, workspace: &'a mut DashWorkspace<'_, FixedScalar>) ->
    Result<DashedPath<'a, FixedScalar>, DashError> {
    if points.iter().any(|point| [point.x.to_bits(), point.y.to_bits()].iter()
        .any(|value| value.unsigned_abs() > FIXED_DEVICE_RAW_LIMIT as u32)) {
        return Err(DashError::CoordinateOutOfRange);
    }
    let mut writer = DashWriter {
        points: workspace.points, contours: workspace.contours,
        point_len: 0, contour_len: 0, current_start: None,
    };
    let Some(&first) = points.first() else { return Ok(writer.finish()); };
    let mut state = pattern.initial_state();
    if points.len() == 1 {
        if state.index & 1 == 0 {
            writer.begin(first)?;
            writer.end()?;
        }
        return Ok(writer.finish());
    }

    let (point_start, contour_start) = (writer.point_len, writer.contour_len);
    let starts_on = state.index & 1 == 0;
    let segment_count = points.len() - 1 + usize::from(closed);
    for index in 0..segment_count {
        fixed_dash_segment(points[index % points.len()],
            points[(index + 1) % points.len()], pattern, &mut state, &mut writer)?;
    }
    if writer.current_start.is_some() { writer.end()?; }
    if closed && starts_on { writer.merge_closure(point_start, contour_start)?; }
    Ok(writer.finish())
}

#[cfg(feature = "fixed")]
fn fixed_dash_segment(from: Point<FixedScalar>, to: Point<FixedScalar>,
    pattern: FixedDashPattern<'_>, state: &mut FixedDashState,
    writer: &mut DashWriter<'_, FixedScalar>) -> Result<(), DashError> {
    let (dx, dy) = (to.x.to_bits() as i64 - from.x.to_bits() as i64,
                    to.y.to_bits() as i64 - from.y.to_bits() as i64);
    let length = integer_sqrt_u64((dx * dx + dy * dy) as _);
    if length == 0 { return Ok(()); }
    let (mut current, mut consumed) = (from, 0_u64);
    while consumed < length {
        let left = length - consumed;
        let remaining = state.remaining as u64;
        let ends_dash = remaining <= left;
        let step = remaining.min(left);
        consumed += step;
        let endpoint = if consumed == length { to } else {
            let interpolate = |start: FixedScalar, delta: i64| {
                let numerator = delta as i128 * consumed as i128;
                let denominator = length as i128;
                let offset = if numerator < 0 {
                    (numerator - denominator / 2) / denominator
                } else { (numerator + denominator / 2) / denominator };
                FixedScalar::from_bits((start.to_bits() as i128 + offset) as _)
            };
            (interpolate(from.x, dx), interpolate(from.y, dy)).into()
        };
        if state.index & 1 == 0 {
            if writer.current_start.is_none() { writer.begin(current)?; }
            writer.point(endpoint)?;
        }
        current = endpoint;
        state.remaining = if ends_dash { 0 } else { (remaining - step) as _ };
        if ends_dash {
            if state.index & 1 == 0 { writer.end()?; }
            state.index = pattern.next(state.index);
            state.remaining = pattern.length(state.index);
        }
    }
    Ok(())
}

struct DashWriter<'a, T = Scalar> {
    points: &'a mut [Point<T>], contours: &'a mut [DashContour],
    point_len: usize, contour_len: usize, current_start: Option<usize>,
}

impl<'a, T: Copy + PartialEq> DashWriter<'a, T> {
    fn begin(&mut self, point: Point<T>) -> Result<(), DashError> {
        self.current_start = Some(self.point_len);
        self.point(point)
    }

    fn point(&mut self, point: Point<T>) -> Result<(), DashError> {
        if self.current_start.is_some_and(|start| self.point_len > start) &&
           self.points[self.point_len - 1] == point { return Ok(()); }
        let needed = self.point_len.checked_add(1).ok_or(DashError::IndexOverflow)?;
        *self.points.get_mut(self.point_len)
            .ok_or(DashError::PointCapacity { needed_at_least: needed })? = point;
        self.point_len = needed;
        Ok(())
    }

    fn end(&mut self) -> Result<(), DashError> {
        let Some(start) = self.current_start.take() else { return Ok(()) };
        let needed = self.contour_len.checked_add(1).ok_or(DashError::IndexOverflow)?;
        let contour = DashContour {
            start: u32::try_from(start).map_err(|_| DashError::IndexOverflow)?,
            len: u32::try_from(self.point_len - start).map_err(|_| DashError::IndexOverflow)?,
            closed: false,
        };
        *self.contours.get_mut(self.contour_len)
            .ok_or(DashError::ContourCapacity { needed_at_least: needed })? = contour;
        self.contour_len = needed;
        Ok(())
    }

    fn merge_closure(&mut self, point_start: usize, contour_start: usize) ->
        Result<(), DashError> {
        let count = self.contour_len - contour_start;
        if count == 0 { return Ok(()); }
        let first_index = contour_start;
        let last_index = self.contour_len - 1;
        let first = self.contours[first_index];
        let last = self.contours[last_index];
        let first_start = first.start as usize;
        let last_start = last.start as usize;
        let first_point = self.points[first_start];
        let last_point = self.points[last_start + last.len() - 1];
        if first_point != last_point { return Ok(()); }
        if count == 1 {
            self.contours[first_index].closed = true;
            return Ok(());
        }

        let last_len = last.len();
        self.points[point_start..self.point_len].rotate_right(last_len);
        self.contours[first_index] = DashContour {
            start: u32::try_from(point_start).map_err(|_| DashError::IndexOverflow)?,
            len: first.len.checked_add(last.len).ok_or(DashError::IndexOverflow)?,
            closed: false,
        };
        for contour in &mut self.contours[first_index + 1..last_index] {
            contour.start = contour.start.checked_add(last.len)
                .ok_or(DashError::IndexOverflow)?;
        }
        self.contour_len -= 1;
        Ok(())
    }

    fn finish(self) -> DashedPath<'a, T> {
        DashedPath {
            points: &self.points[..self.point_len],
            contours: &self.contours[..self.contour_len],
        }
    }
}

#[cfg(test)] mod tests { use super::*;
    use alloc::{vec, vec::Vec};

    fn collect(points: &[Point], closed: bool, lengths: &[f32], phase: f32) ->
        Result<Vec<Vec<Point>>, DashError> {
        let pattern = DashPattern::new(lengths, phase).unwrap();
        let (mut output, mut contours) = ([Point::default(); 64], [DashContour::default(); 16]);
        let mut workspace = DashWorkspace { points: &mut output, contours: &mut contours };
        let dashed = dash_polyline(points, closed, pattern, &mut workspace)?;
        Ok(dashed.contours().map(|(points, _)| points.to_vec()).collect())
    }

    #[test] fn validates_pattern_and_normalizes_phase() {
        assert_eq!(DashPattern::new(&[], 0.0).unwrap_err(), DashPatternError::Empty);
        assert_eq!(DashPattern::new(&[1.0, 0.0], 0.0).unwrap_err(),
            DashPatternError::NonPositiveLength);
        let pattern = DashPattern::new(&[2.0, 1.0, 3.0], -1.0).unwrap();
        assert_eq!(pattern.cycle(), 12.0);
        assert_eq!(pattern.phase(), 11.0);
    }

    #[test] fn open_polyline_preserves_vertices_inside_on_intervals() {
        let points = [(0.0, 0.0).into(), (3.0, 0.0).into(), (3.0, 3.0).into()];
        assert_eq!(collect(&points, false, &[4.0, 2.0], 0.0).unwrap(), [
            vec![(0.0, 0.0).into(), (3.0, 0.0).into(), (3.0, 1.0).into()],
        ]);
    }

    #[test] fn phase_and_odd_patterns_follow_repeated_slot_parity() {
        let points = [(0.0, 0.0).into(), (10.0, 0.0).into()];
        assert_eq!(collect(&points, false, &[2.0, 1.0, 3.0], 2.0).unwrap(), [
            vec![(1.0, 0.0).into(), (4.0, 0.0).into()],
            vec![(6.0, 0.0).into(), (7.0, 0.0).into()],
        ]);
    }

    #[test] fn closed_contour_merges_on_interval_across_seam() {
        let square = [(0.0, 0.0).into(), (4.0, 0.0).into(),
            (4.0, 4.0).into(), (0.0, 4.0).into()];
        let dashed = collect(&square, true, &[6.0, 4.0], 0.0).unwrap();
        assert_eq!(dashed.len(), 1);
        assert_eq!(dashed[0].first(), Some(&(2.0, 4.0).into()));
        assert_eq!(dashed[0].last(), Some(&(4.0, 2.0).into()));
        assert!(dashed[0].contains(&(0.0, 0.0).into()));
    }

    #[test] fn reports_exact_workspace_capacity_class() {
        let points = [(0.0, 0.0).into(), (4.0, 0.0).into()];
        let pattern = DashPattern::new(&[1.0, 1.0], 0.0).unwrap();
        assert_eq!(dash_polyline(&points, false, pattern,
            &mut DashWorkspace { points: &mut [], contours: &mut [] }).unwrap_err(),
            DashError::PointCapacity { needed_at_least: 1 });
    }

    #[test] fn full_on_closed_contour_remains_closed_for_join_semantics() {
        let square = [(0.0, 0.0).into(), (1.0, 0.0).into(),
            (1.0, 1.0).into(), (0.0, 1.0).into()];
        let pattern = DashPattern::new(&[8.0, 1.0], 0.0).unwrap();
        let (mut output, mut contours) =
            ([Point::default(); 8], [DashContour::default(); 2]);
        let mut workspace = DashWorkspace { points: &mut output, contours: &mut contours };
        let dashed = dash_polyline(&square, true, pattern, &mut workspace).unwrap();
        let (_, closed) = dashed.contours().next().unwrap();
        assert!(closed);
    }

    #[cfg(feature = "fixed")]
    #[test] fn fixed_dash_matches_f32_on_exact_metric_segments() {
        let fixed = FixedScalar::from_num;
        let fixed_points = [(fixed(0), fixed(0)).into(), (fixed(6), fixed(8)).into()];
        let fixed_lengths = [fixed(3), fixed(2)];
        let fixed_pattern = FixedDashPattern::new(&fixed_lengths, fixed(1)).unwrap();
        let (mut fixed_output, mut fixed_contours) = (
            [(FixedScalar::ZERO, FixedScalar::ZERO).into(); 16],
            [DashContour::default(); 8],
        );
        let mut fixed_workspace = DashWorkspace {
            points: &mut fixed_output, contours: &mut fixed_contours,
        };
        let fixed_dashed = dash_polyline_fixed(
            &fixed_points, false, fixed_pattern, &mut fixed_workspace).unwrap();
        let fixed_result: Vec<Vec<_>> = fixed_dashed.contours()
            .map(|(points, _)| points.to_vec()).collect();

        let float_points = [(0.0, 0.0).into(), (6.0, 8.0).into()];
        let float_result = collect(&float_points, false, &[3.0, 2.0], 1.0).unwrap();
        assert_eq!(fixed_result.len(), float_result.len());
        for (fixed_contour, float_contour) in fixed_result.iter().zip(&float_result) {
            assert_eq!(fixed_contour.len(), float_contour.len());
            for (fixed, float) in fixed_contour.iter().zip(float_contour) {
                assert!((fixed.x.to_num::<f32>() - float.x).abs() <= 1.0 / 256.0);
                assert!((fixed.y.to_num::<f32>() - float.y).abs() <= 1.0 / 256.0);
            }
        }
    }

    #[cfg(feature = "fixed")]
    #[test] fn fixed_pattern_and_closed_seam_follow_reference_contract() {
        let fixed = FixedScalar::from_num;
        assert_eq!(FixedDashPattern::new(&[], fixed(0)).unwrap_err(),
            FixedDashPatternError::Empty);
        assert_eq!(FixedDashPattern::new(&[fixed(1), fixed(0)], fixed(0)).unwrap_err(),
            FixedDashPatternError::NonPositiveLength);
        let lengths = [fixed(6), fixed(4)];
        let pattern = FixedDashPattern::new(&lengths, fixed(0)).unwrap();
        let square = [(fixed(0), fixed(0)).into(), (fixed(4), fixed(0)).into(),
            (fixed(4), fixed(4)).into(), (fixed(0), fixed(4)).into()];
        let (mut output, mut contours) = (
            [(FixedScalar::ZERO, FixedScalar::ZERO).into(); 32],
            [DashContour::default(); 8],
        );
        let mut workspace = DashWorkspace { points: &mut output, contours: &mut contours };
        let dashed = dash_polyline_fixed(&square, true, pattern, &mut workspace).unwrap();
        assert_eq!(dashed.contours().len(), 1);
        let (points, closed) = dashed.contours().next().unwrap();
        assert!(!closed);
        assert!(points.contains(&(fixed(0), fixed(0)).into()));
    }
}
