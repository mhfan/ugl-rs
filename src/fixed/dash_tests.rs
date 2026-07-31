use super::*;
use alloc::vec::Vec;
use crate::geometry::{FIXED_DEVICE_RAW_LIMIT, FixedScalar};

fn collect(points: &[Point], closed: bool, lengths: &[f32], phase: f32) ->
    Result<Vec<Vec<Point>>, DashError> {
    let pattern = DashPattern::new(lengths, phase).unwrap();
    let (mut output, mut contours) =
        ([Point::default(); 64], [DashContour::default(); 16]);
    let mut workspace = DashWorkspace { points: &mut output, contours: &mut contours };
    let dashed = dash_polyline(points, closed, pattern, &mut workspace)?;
    Ok(dashed.contours().map(|(points, _)| points.to_vec()).collect())
}

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


    #[test] fn fixed_capacity_preflight_is_exact_and_transactional() {
        let fixed = FixedScalar::from_num;
        let points = [(fixed(0), fixed(0)).into(), (fixed(4), fixed(0)).into()];
        let lengths = [fixed(1), fixed(1)];
        let pattern = FixedDashPattern::new(&lengths, fixed(0)).unwrap();
        assert_eq!(fixed_dash_requirements(&points, false, pattern).unwrap(),
            DashRequirements { points: 4, contours: 2 });
        let sentinel = (fixed(17), fixed(19)).into();
        let sentinel_contour = DashContour { start: 7, len: 9, closed: true };
        let (mut output, mut contours) = ([sentinel; 4], [sentinel_contour; 1]);
        assert_eq!(dash_polyline_fixed(&points, false, pattern,
            &mut DashWorkspace { points: &mut output, contours: &mut contours }).unwrap_err(),
            DashError::ContourCapacity { needed_at_least: 2 });
        assert_eq!(output, [sentinel; 4]);
        assert_eq!(contours, [sentinel_contour; 1]);
    }


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
                (FixedScalar::from_num(point.x), FixedScalar::from_num(point.y)).into())
                .collect();
            let lengths = [
                (random() % 8 + 1) as f32,
                (random() % 8 + 1) as f32,
                (random() % 8 + 1) as f32,
            ];
            let fixed_lengths = lengths.map(FixedScalar::from_num);
            let phase = (random() % 33) as i32 - 16;
            let closed = case & 1 != 0;
            let (mut float_output, mut float_contours) =
                ([Point::default(); 2048], [DashContour::default(); 1024]);
            let mut float_workspace = DashWorkspace {
                points: &mut float_output, contours: &mut float_contours,
            };
            let float_pattern = DashPattern::new(&lengths, phase as _).unwrap();
            let required = dash_requirements(&float_points, closed, float_pattern).unwrap();
            let float_dashed = dash_polyline(
                &float_points, closed, float_pattern, &mut float_workspace).unwrap();
            assert_eq!(required, DashRequirements {
                points: float_dashed.contours().map(|(points, _)| points.len()).sum(),
                contours: float_dashed.contours().len(),
            });
            assert!(float_dashed.contours().all(|(points, _)|
                !points.is_empty() && points.iter().all(|point|
                    point.x.is_finite() && point.y.is_finite())));

            let zero = FixedScalar::ZERO;
            let (mut fixed_output, mut fixed_contours) = (
                [(zero, zero).into(); 2048], [DashContour::default(); 1024],
            );
            let mut fixed_workspace = DashWorkspace {
                points: &mut fixed_output, contours: &mut fixed_contours,
            };
            let fixed_pattern = FixedDashPattern::new(
                &fixed_lengths, FixedScalar::from_num(phase)).unwrap();
            let required = fixed_dash_requirements(
                &fixed_points, closed, fixed_pattern).unwrap();
            let fixed_dashed = dash_polyline_fixed(
                &fixed_points, closed, fixed_pattern, &mut fixed_workspace).unwrap();
            assert_eq!(required, DashRequirements {
                points: fixed_dashed.contours().map(|(points, _)| points.len()).sum(),
                contours: fixed_dashed.contours().len(),
            });
            assert!(fixed_dashed.contours().all(|(points, _)|
                !points.is_empty() && points.iter().all(|point|
                    [point.x.to_bits(), point.y.to_bits()].iter().all(|value|
                        value.unsigned_abs() <= FIXED_DEVICE_RAW_LIMIT as u32))));
        }
    }
