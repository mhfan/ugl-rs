//! Fixed-point dash decomposition.

use crate::{dash::{DashCounter, DashError, DashOutput, DashRequirements, DashWorkspace,
        DashWriter, DashedPath, validate_capacity}, fixed::math::integer_sqrt_u64,
    geometry::{FIXED_DEVICE_RAW_LIMIT, FixedScalar, Point}};

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum FixedDashPatternError {
    Empty, NonPositiveLength, CycleOverflow, SlotCountOverflow,
}

#[derive(Clone, Copy, Debug)] pub struct FixedDashPattern<'a> {
    lengths: &'a [FixedScalar], phase: i32, cycle: i32, slots: usize,
}

impl<'a> FixedDashPattern<'a> {
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

#[derive(Clone, Copy)] struct FixedDashState { index: usize, remaining: i32 }

/// Fixed-point counterpart of [`dash_polyline`] with integer distance state.
pub fn dash_polyline_fixed<'a>(points: &[Point<FixedScalar>], closed: bool,
    pattern: FixedDashPattern<'_>, workspace: &'a mut DashWorkspace<'_, FixedScalar>) ->
    Result<DashedPath<'a, FixedScalar>, DashError> {
    let required = fixed_dash_requirements(points, closed, pattern)?;
    validate_capacity(required, workspace.points.len(), workspace.contours.len())?;
    let mut writer = DashWriter {
        points: workspace.points, contours: workspace.contours,
        point_len: 0, contour_len: 0, current_start: None,
    };
    fixed_dash_polyline_to(points, closed, pattern, &mut writer)?;
    Ok(writer.finish())
}

/// Returns the exact workspace needed by [`dash_polyline_fixed`].
pub fn fixed_dash_requirements(points: &[Point<FixedScalar>], closed: bool,
    pattern: FixedDashPattern<'_>) -> Result<DashRequirements, DashError> {
    if points.iter().any(|point| [point.x.to_bits(), point.y.to_bits()].iter()
        .any(|value| value.unsigned_abs() > FIXED_DEVICE_RAW_LIMIT as u32)) {
        return Err(DashError::CoordinateOutOfRange);
    }
    let mut counter = DashCounter::default();
    fixed_dash_polyline_to(points, closed, pattern, &mut counter)?;
    Ok(counter.requirements())
}

fn fixed_dash_polyline_to<W: DashOutput<Point<FixedScalar>>>(
    points: &[Point<FixedScalar>], closed: bool, pattern: FixedDashPattern<'_>,
    writer: &mut W) -> Result<(), DashError> {
    let Some(&first) = points.first() else { return Ok(()); };
    let mut state = pattern.initial_state();
    if points.len() == 1 {
        if state.index & 1 == 0 {
            writer.begin(first)?;
            writer.end()?;
        }
        return Ok(());
    }

    let (point_start, contour_start) = writer.lengths();
    let starts_on = state.index & 1 == 0;
    let segment_count = points.len() - 1 + usize::from(closed);
    for index in 0..segment_count {
        fixed_dash_segment(points[index % points.len()],
            points[(index + 1) % points.len()], pattern, &mut state, writer)?;
    }
    if writer.is_active() { writer.end()?; }
    if closed && starts_on { writer.merge_closure(point_start, contour_start)?; }
    Ok(())
}

fn fixed_dash_segment<W: DashOutput<Point<FixedScalar>>>(
    from: Point<FixedScalar>, to: Point<FixedScalar>,
    pattern: FixedDashPattern<'_>, state: &mut FixedDashState,
    writer: &mut W) -> Result<(), DashError> {
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
            if !writer.is_active() { writer.begin(current)?; }
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
