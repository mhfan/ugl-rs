use super::*;

fn encoded(color: SRGBA<u8>) -> PremulSRGBA8 { color.premul_encoded() }
fn red_blue_stops() -> [GradientStop; 2] {
    [GradientStop::new(0.0, SRGBA::red()),
     GradientStop::new(1.0, SRGBA::blue())]
}

    #[test] fn fixed_linear_gradient_matches_the_encoded_reference_ramp() {
        let stops = red_blue_stops();
        let mut storage = [PremulSRGBA8::zeroed(); 257];
        let stops = GradientStops::with_ramp(&stops, &mut storage).unwrap();
        let ramp = stops.encoded_ramp().unwrap();
        let (from, to) = ((FixedScalar::from_num(2), FixedScalar::from_num(0)),
                          (FixedScalar::from_num(10), FixedScalar::from_num(0)));
        for spread in [SpreadMode::Pad, SpreadMode::Repeat, SpreadMode::Reflect] {
            let fixed = FixedLinearGradient::new(from, to, ramp, spread).unwrap();
            let reference =
                LinearGradient::new((2.0, 0.0), (10.0, 0.0), stops, spread).unwrap();
            for x in 0..32 {
                assert_eq!(fixed.sample_fixed(x, 3),
                    reference.sample(x as f32 + 0.5, 3.5), "spread={spread:?}, x={x}");
            }
        }
    }


    #[test] fn fixed_linear_gradient_validates_geometry_and_widens_extremes() {
        let ramp = [encoded(SRGBA::<u8>::red()), encoded(SRGBA::<u8>::blue())];
        assert_eq!(FixedLinearGradient::new(
            (FixedScalar::from_num(0), FixedScalar::from_num(0)),
            (FixedScalar::from_num(1), FixedScalar::from_num(0)),
            &ramp[..1], SpreadMode::Pad).unwrap_err(), GradientError::RampTooSmall);
        assert_eq!(FixedLinearGradient::new(
            (FixedScalar::from_num(1), FixedScalar::from_num(2)),
            (FixedScalar::from_num(1), FixedScalar::from_num(2)),
            &ramp, SpreadMode::Pad).unwrap_err(), GradientError::DegenerateGeometry);

        let extreme = FixedLinearGradient::new(
            (FixedScalar::from_bits(i32::MIN), FixedScalar::from_bits(i32::MIN)),
            (FixedScalar::from_bits(i32::MAX), FixedScalar::from_bits(i32::MAX)),
            &ramp, SpreadMode::Reflect).unwrap();
        assert!(ramp.contains(&extreme.sample_fixed(u32::MAX, u32::MAX)));
    }


    #[test] fn fixed_concentric_radial_matches_the_encoded_reference_ramp() {
        let stops = red_blue_stops();
        let mut storage = [PremulSRGBA8::zeroed(); 257];
        let stops = GradientStops::with_ramp(&stops, &mut storage).unwrap();
        let ramp = stops.encoded_ramp().unwrap();
        let center = (FixedScalar::from_num(8), FixedScalar::from_num(8));
        for spread in [SpreadMode::Pad, SpreadMode::Repeat, SpreadMode::Reflect] {
            let fixed = FixedRadialGradient::new(
                center, FixedScalar::from_num(8), ramp, spread).unwrap();
            let reference = RadialGradient::new((8.0, 8.0), 8.0, stops, spread).unwrap();
            for y in 0..16 {
                for x in 0..16 {
                    let (actual, expected) = (fixed.sample_fixed(x, y),
                        reference.sample(x as f32 + 0.5, y as f32 + 0.5));
                    let actual = ramp.iter().position(|color| *color == actual).unwrap();
                    let expected = ramp.iter().position(|color| *color == expected).unwrap();
                    assert!(actual.abs_diff(expected) <= 1,
                        "spread={spread:?}, point=({x}, {y}), \
                         actual={actual}, expected={expected}");
                }
            }
        }

        let fixed = FixedRadialGradient::with_radii(center,
            FixedScalar::from_num(8), FixedScalar::ZERO, ramp, SpreadMode::Pad).unwrap();
        let reference = RadialGradient::two_circle(
            (8.0, 8.0), 8.0, (8.0, 8.0), 0.0, stops, SpreadMode::Pad).unwrap();
        for x in 0..16 {
            let (actual, expected) = (fixed.sample_fixed(x, 8),
                reference.sample(x as f32 + 0.5, 8.5));
            let actual = ramp.iter().position(|color| *color == actual).unwrap();
            let expected = ramp.iter().position(|color| *color == expected).unwrap();
            assert!(actual.abs_diff(expected) <= 1);
        }
    }


    #[test] fn fixed_concentric_radial_validates_radii_and_integer_sqrt() {
        let ramp = [encoded(SRGBA::<u8>::red()), encoded(SRGBA::<u8>::blue())];
        let center = (FixedScalar::ZERO, FixedScalar::ZERO);
        assert_eq!(FixedRadialGradient::new(center,
            FixedScalar::from_num(-1), &ramp, SpreadMode::Pad).unwrap_err(),
            GradientError::NegativeRadius);
        assert_eq!(FixedRadialGradient::with_radii(center,
            FixedScalar::from_num(2), FixedScalar::from_num(2),
            &ramp, SpreadMode::Pad).unwrap_err(), GradientError::DegenerateGeometry);

        for root in [0_u128, 1, 2, 3, 255, 65_535, u32::MAX as _] {
            let square = root * root;
            assert_eq!(integer_sqrt(square), root);
            if root != 0 { assert_eq!(integer_sqrt(square - 1), root - 1); }
            assert_eq!(integer_sqrt(square + root), root);
        }
        assert_eq!(integer_sqrt(u128::MAX), u64::MAX as u128);
        let mut value = 0x9e37_79b9_7f4a_7c15_d1b5_4a32_d192_ed03_u128;
        for _ in 0..1_000 {
            value = value.wrapping_mul(0xda94_2042_e4dd_58b5)
                         .wrapping_add(0x94d0_49bb_1331_11eb);
            let root = integer_sqrt(value);
            assert!(root * root <= value);
            if root < u64::MAX as u128 { assert!((root + 1) * (root + 1) > value); }
        }
    }


    #[test] fn fixed_two_circle_radial_matches_quadratic_and_linear_references() {
        fn assert_close(fixed: &FixedRadialGradient<'_>, reference: &RadialGradient<'_>,
            ramp: &[PremulSRGBA8], x: u32, y: u32) {
            let (actual, expected) = (fixed.sample_fixed(x, y),
                reference.sample(x as f32 + 0.5, y as f32 + 0.5));
            match (ramp.iter().position(|color| *color == actual),
                   ramp.iter().position(|color| *color == expected)) {
                (Some(actual), Some(expected)) => assert!(actual.abs_diff(expected) <= 1,
                    "point=({x}, {y}), actual={actual}, expected={expected}"),
                (None, None) => assert_eq!(actual, expected),
                _ => panic!("root validity differs at ({x}, {y}): {actual:?} != {expected:?}"),
            }
        }

        let stop_values = red_blue_stops();
        let mut storage = [PremulSRGBA8::zeroed(); 257];
        let stops = GradientStops::with_ramp(&stop_values, &mut storage).unwrap();
        let ramp = stops.encoded_ramp().unwrap();
        let fixed = FixedScalar::from_num;
        for spread in [SpreadMode::Pad, SpreadMode::Repeat, SpreadMode::Reflect] {
            let radial = FixedRadialGradient::two_circle(
                (fixed(1), fixed(0)), fixed(0), (fixed(0), fixed(0)), fixed(4),
                ramp, spread).unwrap();
            let reference = RadialGradient::two_circle(
                (1.0, 0.0), 0.0, (0.0, 0.0), 4.0, stops, spread).unwrap();
            for y in 0..8 {
                for x in 0..8 { assert_close(&radial, &reference, ramp, x, y); }
            }
        }

        let tangent = FixedRadialGradient::two_circle(
            (fixed(0), fixed(0)), fixed(0), (fixed(1), fixed(0)), fixed(1),
            ramp, SpreadMode::Pad).unwrap();
        let tangent_reference = RadialGradient::two_circle(
            (0.0, 0.0), 0.0, (1.0, 0.0), 1.0, stops, SpreadMode::Pad).unwrap();
        for y in 0..4 {
            for x in 0..4 { assert_close(&tangent, &tangent_reference, ramp, x, y); }
        }

        let near_tangent = FixedRadialGradient::two_circle(
            (fixed(4), fixed(4)), fixed(1),
            (FixedScalar::from_bits(4 * 256 + 257), fixed(4)), fixed(2),
            ramp, SpreadMode::Reflect).unwrap();
        let near_tangent_reference = RadialGradient::two_circle(
            (4.0, 4.0), 1.0, (5.0 + 1.0 / 256.0, 4.0), 2.0,
            stops, SpreadMode::Reflect).unwrap();
        for y in 0..12 {
            for x in 0..12 {
                assert_close(&near_tangent, &near_tangent_reference, ramp, x, y);
            }
        }
    }


    #[test] fn fixed_two_circle_radial_enforces_the_fixed_device_domain() {
        let ramp = [encoded(SRGBA::<u8>::red()), encoded(SRGBA::<u8>::blue())];
        let fixed = FixedScalar::from_num;
        assert_eq!(FixedRadialGradient::new(
            (FixedScalar::from_bits(FIXED_DEVICE_RAW_LIMIT + 1), fixed(0)), fixed(1),
            &ramp, SpreadMode::Pad).unwrap_err(), GradientError::CoordinateOutOfRange);
        let radial = FixedRadialGradient::new(
            (fixed(0), fixed(0)), fixed(1), &ramp, SpreadMode::Pad).unwrap();
        let first_outside_pixel = FIXED_DEVICE_RAW_LIMIT as u32 / 256;
        assert_eq!(radial.sample_fixed(first_outside_pixel, 0),
            PremulSRGBA8::zeroed());
    }


    #[test] fn fixed_conic_cordic_tracks_exact_angles_and_encoded_ramp() {
        assert_eq!(cordic_turn( 1,  0), FixedAngle::ZERO.to_bits());
        assert_eq!(cordic_turn( 0,  1), FixedAngle::QUARTER_TURN.to_bits());
        assert_eq!(cordic_turn(-1,  0), FixedAngle::HALF_TURN.to_bits());
        assert_eq!(cordic_turn( 0, -1), FixedAngle::THREE_QUARTER_TURN.to_bits());
        let mut maximum_error = 0.0_f32;
        for y in -64_i64..=64 {
            for x in -64_i64..=64 {
                if x == 0 && y == 0 { continue; }
                let actual = cordic_turn(x, y) as f32 / 4_294_967_296.0;
                let expected = SpreadMode::Repeat.map(
                    libm::atan2f(y as _, x as _) / TAU);
                let difference = (actual - expected).abs();
                maximum_error = maximum_error.max(difference.min(1.0 - difference));
            }
        }
        assert!(maximum_error <= 6e-6, "maximum turn error={maximum_error}");

        let stop_values = red_blue_stops();
        let mut storage = [PremulSRGBA8::zeroed(); 257];
        let stops = GradientStops::with_ramp(&stop_values, &mut storage).unwrap();
        let ramp = stops.encoded_ramp().unwrap();
        let fixed = FixedScalar::from_num;
        for (angle, start_angle) in [
            (FixedAngle::ZERO, 0.0),
            (FixedAngle::QUARTER_TURN, TAU / 4.0),
        ] {
            let conic = FixedConicGradient::new(
                (fixed(16), fixed(16)), angle, ramp).unwrap();
            let reference = ConicGradient::new((16.0, 16.0), start_angle, stops).unwrap();
            for y in 0..32 {
                for x in 0..32 {
                    let (actual, expected) = (conic.sample_fixed(x, y),
                        reference.sample(x as f32 + 0.5, y as f32 + 0.5));
                    let actual = ramp.iter().position(|color| *color == actual).unwrap();
                    let expected = ramp.iter().position(|color| *color == expected).unwrap();
                    assert!(actual.abs_diff(expected) <= 1,
                        "point=({x}, {y}), actual={actual}, expected={expected}");
                }
            }
        }
    }


    #[test] fn fixed_conic_validates_ramp_and_device_domain() {
        let ramp = [encoded(SRGBA::<u8>::red()), encoded(SRGBA::<u8>::blue())];
        let fixed = FixedScalar::from_num;
        assert_eq!(FixedAngle::from_turn_fraction(1, 4), Some(FixedAngle::QUARTER_TURN));
        assert_eq!(FixedAngle::from_turn_fraction(1, 0), None);
        assert_eq!(FixedConicGradient::new((fixed(0), fixed(0)),
            FixedAngle::ZERO, &ramp[..1]).unwrap_err(), GradientError::RampTooSmall);
        assert_eq!(FixedConicGradient::new(
            (FixedScalar::from_bits(FIXED_DEVICE_RAW_LIMIT + 1), fixed(0)),
            FixedAngle::ZERO, &ramp).unwrap_err(), GradientError::CoordinateOutOfRange);
        let conic = FixedConicGradient::new(
            (fixed(0), fixed(0)), FixedAngle::ZERO, &ramp).unwrap();
        assert_eq!(conic.sample_fixed(FIXED_DEVICE_RAW_LIMIT as u32 / 256, 0),
            PremulSRGBA8::zeroed());
    }

