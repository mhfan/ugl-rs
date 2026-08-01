use crate::{dash::{DashCounter, DashError, DashOutput, DashRequirements, DashWorkspace,
    DashWriter, DashedPath, validate_capacity}, geometry::Point};
use crate::float::{fmod, sqrt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum DashPatternError {
    Empty, NonFiniteLength, NonPositiveLength, NonFinitePhase,
    CycleOverflow, SlotCountOverflow,
}

/// Validated alternating on/off lengths and starting phase.
///
/// Odd-length arrays are repeated to preserve alternating on/off parity, and
/// negative phases are normalized into the resulting cycle:
///
/// ```
/// use ugl_rs::dash::{DashPattern, DashPatternError};
///
/// let pattern = DashPattern::new(&[2.0, 1.0, 3.0], -1.0).unwrap();
/// assert_eq!((pattern.cycle(), pattern.phase()), (12.0, 11.0));
/// assert_eq!(DashPattern::new(&[], 0.0).unwrap_err(), DashPatternError::Empty);
/// assert_eq!(DashPattern::new(&[1.0, 0.0], 0.0).unwrap_err(),
///     DashPatternError::NonPositiveLength);
/// ```
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
        let phase = fmod(phase, cycle);
        Ok(Self { lengths, phase: if phase < 0.0 { phase + cycle } else { phase },
            cycle, slots })
    }

    pub fn lengths(&self) -> &'a [f32] { self.lengths }
    pub fn phase(&self) -> f32 { self.phase }
    pub fn cycle(&self) -> f32 { self.cycle }

    fn initial_state(self) -> DashState {
        let (mut index, mut phase) = (0, self.phase as f64);
        while phase >= self.length(index) as f64 {
            phase -= self.length(index) as f64;
            index = self.next(index);
        }
        DashState { index, remaining: self.length(index) - phase as f32 }
    }

    fn length(self, index: usize) -> f32 { self.lengths[index % self.lengths.len()] }
    fn next(self, index: usize) -> usize {
        if index + 1 == self.slots { 0 } else { index + 1 }
    }
}

#[derive(Clone, Copy)]
struct DashState { index: usize, remaining: f32 }

/// Decomposes one flattened contour into open on-dash polylines.
///
/// Closed contours continue through their closing segment. When an on interval
/// crosses the closure seam, its last and first pieces are merged so the seam
/// receives a join rather than two caps.
pub fn dash_polyline<'a>(points: &[Point], closed: bool, pattern: DashPattern<'_>,
    workspace: &'a mut DashWorkspace<'_>) -> Result<DashedPath<'a>, DashError> {
    let required = dash_requirements(points, closed, pattern)?;
    validate_capacity(required, workspace.points.len(), workspace.contours.len())?;
    let mut writer = DashWriter {
        points: workspace.points, contours: workspace.contours,
        point_len: 0, contour_len: 0, current_start: None,
    };
    dash_polyline_to(points, closed, pattern, &mut writer)?;
    Ok(writer.finish())
}

/// Returns the exact workspace needed by [`dash_polyline`].
pub fn dash_requirements(points: &[Point], closed: bool, pattern: DashPattern<'_>) ->
    Result<DashRequirements, DashError> {
    if points.iter().any(|point| !point.x.is_finite() || !point.y.is_finite()) {
        return Err(DashError::NonFinitePoint);
    }
    let mut counter = DashCounter::default();
    dash_polyline_to(points, closed, pattern, &mut counter)?;
    Ok(counter.requirements())
}

fn dash_polyline_to<W: DashOutput<Point>>(points: &[Point], closed: bool,
    pattern: DashPattern<'_>, writer: &mut W) -> Result<(), DashError> {
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
        let from = points[index % points.len()];
        let to = points[(index + 1) % points.len()];
        dash_segment(from, to, pattern, &mut state, writer)?;
    }
    if writer.is_active() { writer.end()?; }
    if closed && starts_on {
        writer.merge_closure(point_start, contour_start)?;
    }
    Ok(())
}

fn dash_segment<W: DashOutput<Point>>(from: Point, to: Point, pattern: DashPattern<'_>,
    state: &mut DashState, writer: &mut W) -> Result<(), DashError> {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = sqrt(dx * dx + dy * dy);
    if !length.is_finite() { return Err(DashError::NonFinitePoint); }
    if length == 0.0 { return Ok(()); }
    let (unit_x, unit_y) = (dx / length, dy / length);
    let (mut current, mut consumed) = (from, 0.0);
    while consumed < length {
        let left = length - consumed;
        let ends_dash = state.remaining <= left;
        let step = state.remaining.min(left);
        let next_consumed = consumed + step;
        if next_consumed == consumed { return Err(DashError::PrecisionExhausted); }
        consumed = next_consumed;
        let endpoint = if consumed == length { to } else {
            (from.x + unit_x * consumed, from.y + unit_y * consumed).into()
        };
        if state.index & 1 == 0 {
            if !writer.is_active() { writer.begin(current)?; }
            writer.point(endpoint)?;
        }
        current = endpoint;
        state.remaining = if ends_dash { 0.0 } else { state.remaining - step };
        if ends_dash {
            if state.index & 1 == 0 { writer.end()?; }
            state.index = pattern.next(state.index);
            state.remaining = pattern.length(state.index);
        }
    }
    Ok(())
}
