//! Shared dash storage and floating-point decomposition.

use super::geometry::{Point, Scalar};

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
    NonFinitePoint, PrecisionExhausted, CoordinateOutOfRange,
    PointCapacity { needed_at_least: usize },
    ContourCapacity { needed_at_least: usize },
    IndexOverflow,
}

#[derive(Debug)] pub struct DashedPath<'a, T = Scalar> {
    points: &'a [Point<T>], contours: &'a [DashContour],
}

impl<'a, T> DashedPath<'a, T> {
    pub fn point_count(&self) -> usize { self.points.len() }
    pub fn contour_count(&self) -> usize { self.contours.len() }

    pub fn contours(&self) -> impl ExactSizeIterator<Item = (&'a [Point<T>], bool)> + 'a {
        self.contours.iter().map(|contour| {
            let start = contour.start as usize;
            (&self.points[start..start + contour.len()], contour.is_closed())
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] pub struct DashRequirements {
    pub points: usize, pub contours: usize,
}

pub(crate) fn validate_capacity(required: DashRequirements, points: usize, contours: usize) ->
    Result<(), DashError> {
    if points < required.points {
        return Err(DashError::PointCapacity { needed_at_least: required.points });
    }
    if contours < required.contours {
        return Err(DashError::ContourCapacity { needed_at_least: required.contours });
    }
    Ok(())
}

pub(crate) trait DashOutput<T> {
    fn is_active(&self) -> bool;
    fn lengths(&self) -> (usize, usize);
    fn begin(&mut self, point: T) -> Result<(), DashError>;
    fn point(&mut self, point: T) -> Result<(), DashError>;
    fn end(&mut self) -> Result<(), DashError>;
    fn merge_closure(&mut self, point_start: usize, contour_start: usize) ->
        Result<(), DashError>;
}

pub(crate) struct DashCounter<T> {
    point_len: usize, contour_len: usize, current_len: usize,
    current_first: Option<T>, last: Option<T>, first_contour_first: Option<T>,
}

impl<T> Default for DashCounter<T> {
    fn default() -> Self {
        Self { point_len: 0, contour_len: 0, current_len: 0,
            current_first: None, last: None, first_contour_first: None }
    }
}

impl<T> DashCounter<T> {
    pub(crate) fn requirements(&self) -> DashRequirements {
        DashRequirements { points: self.point_len, contours: self.contour_len }
    }
}

impl<T: Copy + PartialEq> DashOutput<T> for DashCounter<T> {
    fn is_active(&self) -> bool { self.current_first.is_some() }
    fn lengths(&self) -> (usize, usize) { (self.point_len, self.contour_len) }

    fn begin(&mut self, point: T) -> Result<(), DashError> {
        self.current_first = Some(point);
        self.current_len = 0;
        self.point(point)
    }

    fn point(&mut self, point: T) -> Result<(), DashError> {
        if self.current_len != 0 && self.last == Some(point) { return Ok(()); }
        self.point_len = self.point_len.checked_add(1).ok_or(DashError::IndexOverflow)?;
        self.current_len = self.current_len.checked_add(1).ok_or(DashError::IndexOverflow)?;
        self.last = Some(point);
        Ok(())
    }

    fn end(&mut self) -> Result<(), DashError> {
        let Some(first) = self.current_first.take() else { return Ok(()) };
        u32::try_from(self.point_len - self.current_len)
            .map_err(|_| DashError::IndexOverflow)?;
        u32::try_from(self.current_len).map_err(|_| DashError::IndexOverflow)?;
        if self.contour_len == 0 { self.first_contour_first = Some(first); }
        self.contour_len = self.contour_len.checked_add(1).ok_or(DashError::IndexOverflow)?;
        self.current_len = 0;
        Ok(())
    }

    fn merge_closure(&mut self, _: usize, contour_start: usize) ->
        Result<(), DashError> {
        let count = self.contour_len - contour_start;
        if count > 1 && self.first_contour_first == self.last {
            self.contour_len -= 1;
        }
        Ok(())
    }
}

pub(crate) struct DashWriter<'a, T = Scalar> {
    pub(crate) points: &'a mut [Point<T>],
    pub(crate) contours: &'a mut [DashContour],
    pub(crate) point_len: usize, pub(crate) contour_len: usize,
    pub(crate) current_start: Option<usize>,
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

    pub(crate) fn finish(self) -> DashedPath<'a, T> {
        DashedPath {
            points: &self.points[..self.point_len],
            contours: &self.contours[..self.contour_len],
        }
    }
}

impl<T: Copy + PartialEq> DashOutput<Point<T>> for DashWriter<'_, T> {
    fn is_active(&self) -> bool { self.current_start.is_some() }
    fn lengths(&self) -> (usize, usize) { (self.point_len, self.contour_len) }
    fn begin(&mut self, point: Point<T>) -> Result<(), DashError> {
        DashWriter::begin(self, point)
    }
    fn point(&mut self, point: Point<T>) -> Result<(), DashError> {
        DashWriter::point(self, point)
    }
    fn end(&mut self) -> Result<(), DashError> { DashWriter::end(self) }
    fn merge_closure(&mut self, point_start: usize, contour_start: usize) ->
        Result<(), DashError> {
        DashWriter::merge_closure(self, point_start, contour_start)
    }
}

#[cfg(all(test, feature = "f32"))] mod tests { use super::*;
    use alloc::{vec, vec::Vec};
    use crate::float::dash::{DashPattern, dash_polyline, dash_requirements};

    fn collect(points: &[Point], closed: bool, lengths: &[f32], phase: f32) ->
        Result<Vec<Vec<Point>>, DashError> {
        let pattern = DashPattern::new(lengths, phase).unwrap();
        let (mut output, mut contours) = ([Point::default(); 64], [DashContour::default(); 16]);
        let mut workspace = DashWorkspace { points: &mut output, contours: &mut contours };
        let dashed = dash_polyline(points, closed, pattern, &mut workspace)?;
        Ok(dashed.contours().map(|(points, _)| points.to_vec()).collect())
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
        assert_eq!(dash_requirements(&points, false, pattern).unwrap(),
            DashRequirements { points: 4, contours: 2 });
        let (sentinel_point, sentinel_contour) =
            ((17.0, 19.0).into(), DashContour { start: 7, len: 9, closed: true });
        let (mut output, mut contours) = ([sentinel_point; 3], [sentinel_contour; 2]);
        assert_eq!(dash_polyline(&points, false, pattern,
            &mut DashWorkspace { points: &mut output, contours: &mut contours }).unwrap_err(),
            DashError::PointCapacity { needed_at_least: 4 });
        assert_eq!(output, [sentinel_point; 3]);
        assert_eq!(contours, [sentinel_contour; 2]);
    }

    #[test] fn reports_when_f32_can_no_longer_advance_a_short_dash() {
        let points = [(0.0, 0.0).into(), (2.0e9, 0.0).into()];
        let lengths = [1.0, 1.0e9, f32::MIN_POSITIVE, 1.0];
        let pattern = DashPattern::new(&lengths, 1.0).unwrap();
        let (mut output, mut contours) =
            ([Point::default(); 4], [DashContour::default(); 2]);
        assert_eq!(dash_polyline(&points, false, pattern,
            &mut DashWorkspace { points: &mut output, contours: &mut contours }).unwrap_err(),
            DashError::PrecisionExhausted);
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

}
