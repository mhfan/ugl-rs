//! Fixed-point dash decomposition.

use crate::{common::{dash::{DashCounter, DashError, DashOutput, DashRequirements, DashWorkspace,
        DashWriter, DashedPath, validate_capacity},
        geometry::Point},
    fixed::{DEVICE_RAW_LIMIT, Scalar, math::integer_sqrt_u64},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum PatternError {
    Empty, NonPositiveLength, CycleOverflow, SlotCountOverflow,
}

/// Validated fixed-point dash lengths and normalized phase.
///
/// ```
/// use ugl_rs::fixed::{Scalar, dash::{Pattern, PatternError}};
///
/// let lengths = [Scalar::from_num(2), Scalar::from_num(1), Scalar::from_num(3)];
/// let pattern = Pattern::new(&lengths, Scalar::from_num(-1)).unwrap();
/// assert_eq!(pattern.cycle(), Scalar::from_num(12));
/// assert_eq!(pattern.phase(), Scalar::from_num(11));
/// assert_eq!(Pattern::new(&[], Scalar::ZERO).unwrap_err(), PatternError::Empty);
/// ```
#[derive(Clone, Copy, Debug)] pub struct Pattern<'a> {
    lengths: &'a [Scalar], phase: i32, cycle: i32, slots: usize,
}

impl<'a> Pattern<'a> {
    pub fn new(lengths: &'a [Scalar], phase: Scalar) ->
        Result<Self, PatternError> {
        if lengths.is_empty() { return Err(PatternError::Empty); }
        let mut cycle = 0_i64;
        for length in lengths {
            if *length <= Scalar::ZERO {
                return Err(PatternError::NonPositiveLength);
            }
            cycle = cycle.checked_add(length.to_bits() as _)
                .ok_or(PatternError::CycleOverflow)?;
        }
        let slots = if lengths.len() & 1 == 0 { lengths.len() } else {
            lengths.len().checked_mul(2)
                .ok_or(PatternError::SlotCountOverflow)?
        };
        if slots != lengths.len() {
            cycle = cycle.checked_mul(2).ok_or(PatternError::CycleOverflow)?;
        }
        let cycle = i32::try_from(cycle).map_err(|_| PatternError::CycleOverflow)?;
        Ok(Self { lengths, phase: phase.to_bits().rem_euclid(cycle), cycle, slots })
    }

    pub fn lengths(&self) -> &'a [Scalar] { self.lengths }
    pub fn phase(&self) -> Scalar { Scalar::from_bits(self.phase) }
    pub fn cycle(&self) -> Scalar { Scalar::from_bits(self.cycle) }

    fn initial_state(self) -> DashState {
        let (mut index, mut phase) = (0, self.phase);
        while phase >= self.length(index) {
            phase -= self.length(index);
            index = self.next(index);
        }
        DashState { index, remaining: self.length(index) - phase }
    }

    fn length(self, index: usize) -> i32 {
        self.lengths[index % self.lengths.len()].to_bits()
    }
    fn next(self, index: usize) -> usize {
        if index + 1 == self.slots { 0 } else { index + 1 }
    }
}

#[derive(Clone, Copy)] struct DashState { index: usize, remaining: i32 }

/// Fixed-point counterpart of [`crate::float::dash::dash_polyline`] with integer distance state.
pub fn dash_polyline<'a>(points: &[Point<Scalar>], closed: bool,
    pattern: Pattern<'_>, workspace: &'a mut DashWorkspace<'_, Scalar>) ->
    Result<DashedPath<'a, Scalar>, DashError> {
    let required = requirements(points, closed, pattern)?;
    validate_capacity(required, workspace.points.len(), workspace.contours.len())?;
    let mut writer = DashWriter {
        points: workspace.points, contours: workspace.contours,
        point_len: 0, contour_len: 0, current_start: None,
    };
    dash_polyline_to(points, closed, pattern, &mut writer)?;
    Ok(writer.finish())
}

/// Returns the exact workspace needed by this module's [`dash_polyline`].
pub fn requirements(points: &[Point<Scalar>], closed: bool,
    pattern: Pattern<'_>) -> Result<DashRequirements, DashError> {
    if points.iter().any(|point| [point.x.to_bits(), point.y.to_bits()].iter()
        .any(|value| value.unsigned_abs() > DEVICE_RAW_LIMIT as u32)) {
        return Err(DashError::CoordinateOutOfRange);
    }
    let mut counter = DashCounter::default();
    dash_polyline_to(points, closed, pattern, &mut counter)?;
    Ok(counter.requirements())
}

fn dash_polyline_to<W: DashOutput<Point<Scalar>>>(
    points: &[Point<Scalar>], closed: bool, pattern: Pattern<'_>,
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
        dash_segment(points[index % points.len()],
            points[(index + 1) % points.len()], pattern, &mut state, writer)?;
    }
    if writer.is_active() { writer.end()?; }
    if closed && starts_on { writer.merge_closure(point_start, contour_start)?; }
    Ok(())
}

fn dash_segment<W: DashOutput<Point<Scalar>>>(
    from: Point<Scalar>, to: Point<Scalar>,
    pattern: Pattern<'_>, state: &mut DashState,
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
            let interpolate = |start: Scalar, delta: i64| {
                let numerator = delta as i128 * consumed as i128;
                let denominator = length as i128;
                let offset = if numerator < 0 {
                    (numerator - denominator / 2) / denominator
                } else { (numerator + denominator / 2) / denominator };
                Scalar::from_bits((start.to_bits() as i128 + offset) as _)
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

#[cfg(test)] mod tests {
use super::*;
use crate::common::dash::{DashContour, DashError, DashRequirements, DashWorkspace};
#[cfg(feature = "f32")]
use crate::float::dash::{DashPattern as ReferencePattern,
    dash_polyline as reference_dash_polyline, dash_requirements};
#[cfg(feature = "f32")] use alloc::vec::Vec;
use crate::fixed::Scalar;
#[cfg(feature = "f32")] use crate::fixed::DEVICE_RAW_LIMIT;

#[cfg(feature = "f32")]
fn collect(points: &[Point], closed: bool, lengths: &[f32], phase: f32) ->
    Result<Vec<Vec<Point>>, DashError> {
    let pattern = ReferencePattern::new(lengths, phase).unwrap();
    let (mut output, mut contours) =
        ([Point::default(); 64], [DashContour::default(); 16]);
    let mut workspace = DashWorkspace { points: &mut output, contours: &mut contours };
    let dashed = reference_dash_polyline(points, closed, pattern, &mut workspace)?;
    Ok(dashed.contours().map(|(points, _)| points.to_vec()).collect())
}

    #[cfg(feature = "f32")]
    #[test] fn dash_matches_f32_on_exact_metric_segments() {
        let fixed = Scalar::from_num;
        let fixed_points = [(fixed(0), fixed(0)).into(), (fixed(6), fixed(8)).into()];
        let fixed_lengths = [fixed(3), fixed(2)];
        let fixed_pattern = Pattern::new(&fixed_lengths, fixed(1)).unwrap();
        let (mut fixed_output, mut fixed_contours) = (
            [(Scalar::ZERO, Scalar::ZERO).into(); 16],
            [DashContour::default(); 8],
        );
        let mut fixed_workspace = DashWorkspace {
            points: &mut fixed_output, contours: &mut fixed_contours,
        };
        let fixed_dashed = dash_polyline(
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


    #[test] fn pattern_and_closed_seam_follow_reference_contract() {
        let fixed = Scalar::from_num;
        assert_eq!(Pattern::new(&[], fixed(0)).unwrap_err(),
            PatternError::Empty);
        assert_eq!(Pattern::new(&[fixed(1), fixed(0)], fixed(0)).unwrap_err(),
            PatternError::NonPositiveLength);
        let lengths = [fixed(6), fixed(4)];
        let pattern = Pattern::new(&lengths, fixed(0)).unwrap();
        let square = [(fixed(0), fixed(0)).into(), (fixed(4), fixed(0)).into(),
            (fixed(4), fixed(4)).into(), (fixed(0), fixed(4)).into()];
        let (mut output, mut contours) = (
            [(Scalar::ZERO, Scalar::ZERO).into(); 32],
            [DashContour::default(); 8],
        );
        let mut workspace = DashWorkspace { points: &mut output, contours: &mut contours };
        let dashed = dash_polyline(&square, true, pattern, &mut workspace).unwrap();
        assert_eq!(dashed.contours().len(), 1);
        let (points, closed) = dashed.contours().next().unwrap();
        assert!(!closed);
        assert!(points.contains(&(fixed(0), fixed(0)).into()));
    }


    #[test] fn capacity_preflight_is_exact_and_transactional() {
        let fixed = Scalar::from_num;
        let points = [(fixed(0), fixed(0)).into(), (fixed(4), fixed(0)).into()];
        let lengths = [fixed(1), fixed(1)];
        let pattern = Pattern::new(&lengths, fixed(0)).unwrap();
        assert_eq!(requirements(&points, false, pattern).unwrap(),
            DashRequirements { points: 4, contours: 2 });
        let sentinel = (fixed(17), fixed(19)).into();
        let sentinel_contour = DashContour::default();
        let (mut output, mut contours) = ([sentinel; 4], [sentinel_contour; 1]);
        assert_eq!(dash_polyline(&points, false, pattern,
            &mut DashWorkspace { points: &mut output, contours: &mut contours }).unwrap_err(),
            DashError::ContourCapacity { needed_at_least: 2 });
        assert_eq!(output, [sentinel; 4]);
        assert_eq!(contours, [sentinel_contour; 1]);
    }


    #[cfg(feature = "f32")]
    #[test] fn randomized_f32_and_fixed_dash_outputs_remain_bounded() {
        let mut state = 0x4d59_5df4_d0f3_3173_u64;
        let mut random = || {
            state ^= state << 13; state ^= state >> 7; state ^= state << 17; state
        };
        for case in 0..256 {
            let mut float_points: Vec<Point> = Vec::new();
            for _ in 0..8 {
                let x = (random() % 129) as i32 - 64;
                let y = (random() % 129) as i32 - 64;
                float_points.push((x as f32, y as f32).into());
            }
            let fixed_points: Vec<_> = float_points.iter().map(|point|
                (Scalar::from_num(point.x), Scalar::from_num(point.y)).into())
                .collect();
            let lengths = [
                (random() % 8 + 1) as f32,
                (random() % 8 + 1) as f32,
                (random() % 8 + 1) as f32,
            ];
            let fixed_lengths = lengths.map(Scalar::from_num);
            let phase = (random() % 33) as i32 - 16;
            let closed = case & 1 != 0;
            let (mut float_output, mut float_contours) =
                ([Point::default(); 2048], [DashContour::default(); 1024]);
            let mut float_workspace = DashWorkspace {
                points: &mut float_output, contours: &mut float_contours,
            };
            let float_pattern = ReferencePattern::new(&lengths, phase as _).unwrap();
            let required = dash_requirements(&float_points, closed, float_pattern).unwrap();
            let float_dashed = reference_dash_polyline(
                &float_points, closed, float_pattern, &mut float_workspace).unwrap();
            assert_eq!(required, DashRequirements {
                points: float_dashed.contours().map(|(points, _)| points.len()).sum(),
                contours: float_dashed.contours().len(),
            });
            assert!(float_dashed.contours().all(|(points, _)|
                !points.is_empty() && points.iter().all(|point|
                    point.x.is_finite() && point.y.is_finite())));

            let zero = Scalar::ZERO;
            let (mut fixed_output, mut fixed_contours) = (
                [(zero, zero).into(); 2048], [DashContour::default(); 1024],
            );
            let mut fixed_workspace = DashWorkspace {
                points: &mut fixed_output, contours: &mut fixed_contours,
            };
            let fixed_pattern = Pattern::new(
                &fixed_lengths, Scalar::from_num(phase)).unwrap();
            let required = requirements(
                &fixed_points, closed, fixed_pattern).unwrap();
            let fixed_dashed = dash_polyline(
                &fixed_points, closed, fixed_pattern, &mut fixed_workspace).unwrap();
            assert_eq!(required, DashRequirements {
                points: fixed_dashed.contours().map(|(points, _)| points.len()).sum(),
                contours: fixed_dashed.contours().len(),
            });
            assert!(fixed_dashed.contours().all(|(points, _)|
                !points.is_empty() && points.iter().all(|point|
                    [point.x.to_bits(), point.y.to_bits()].iter().all(|value|
                        value.unsigned_abs() <= DEVICE_RAW_LIMIT as u32))));
        }
    }
}
