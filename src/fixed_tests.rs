
use super::*;
use alloc::{vec, vec::Vec};
use core::convert::Infallible;
use crate::analytic::{AnalyticIntersection, AnalyticWorkspace, rasterize_edges_analytic};

fn fixed(value: f32) -> FixedScalar { FixedScalar::from_num(value) }

fn render(edges: &[Edge<FixedScalar>], width: usize, height: usize,
    fill_rule: FillRule) -> Vec<u8> {
    let mut lines = vec![FixedLine::default(); edges.len()];
    prepare_lines(edges, &mut lines).unwrap();
    let requirements = fixed_strip_requirements(&lines, height as _).unwrap();
    let (mut segments, mut trapezoids, mut row_area, mut strip_offsets, mut strip_indices) = (
        vec![FixedSegment::default(); lines.len()],
        vec![FixedTrapezoid::default(); lines.len().div_ceil(2)],
        vec![0; width], vec![0; requirements.offsets], vec![0; requirements.indices],
    );
    let mut pixels = vec![0; width * height];
    rasterize_lines(&lines, width as _, height as _, fill_rule,
        &mut FixedRasterWorkspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
        }, &mut |x, y, coverage| {
        pixels[y as usize * width + x as usize] = coverage;
        Ok::<_, Infallible>(())
    }).unwrap();
    pixels
}

fn render_analytic(edges: &[Edge], width: usize, height: usize,
    fill_rule: FillRule) -> Vec<u8> {
    let (mut pixels, mut row) = (vec![0; width * height], vec![0.0; width]);
    let mut intersections = vec![AnalyticIntersection::default(); edges.len()];
    rasterize_edges_analytic(edges, width as _, height as _, fill_rule,
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
            render_analytic(&float_edges, 6, 6, FillRule::NonZero),
        );
        for (pixel, (fixed, reference)) in
            fixed_pixels.iter().zip(&float_pixels).enumerate() {
            assert!(fixed.abs_diff(*reference) <= 2,
                "case {case}, pixel {pixel}: fixed={fixed}, f32={reference}");
        }
    }
}

#[test] fn fixed_self_intersections_track_the_f32_analytic_reference() {
    let scenes = [[(0, 0), (512, 512), (0, 512), (512, 0)],
                    [(32, 17), (737, 491), (61, 690), (689, 3)],
                    [(-64, 100), (800, 600), (0, 700), (720, -20)]];
    for (case, points) in scenes.into_iter().enumerate() {
        let points = points.map(|(x, y)|
            (FixedScalar::from_bits(x), FixedScalar::from_bits(y)).into());
        let mut fixed_edges = Vec::new();
        for index in 0..points.len() {
            if let Some(edge) = Edge::from_line(points[index],
                points[(index + 1) % points.len()]) {
                fixed_edges.push(edge);
            }
        }
        let float_edges: Vec<Edge> = fixed_edges.iter().map(|edge| Edge {
            upper: (edge.upper.x.to_num(), edge.upper.y.to_num()).into(),
            lower: (edge.lower.x.to_num(), edge.lower.y.to_num()).into(),
            winding: edge.winding,
        }).collect();
        for fill_rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let fixed_pixels = render(&fixed_edges, 3, 3, fill_rule);
            let float_pixels = render_analytic(&float_edges, 3, 3, fill_rule);
            for (pixel, (fixed, reference)) in
                fixed_pixels.iter().zip(&float_pixels).enumerate() {
                assert!(fixed.abs_diff(*reference) <= 2,
                    "case {case}, pixel {pixel}, {fill_rule:?}: \
                        fixed={fixed}, f32={reference}");
            }
        }
    }
}

#[test] fn rational_crossing_events_round_only_at_the_area_boundary() {
    let line = |from, to| FixedLine::new(Edge::from_line(from, to).unwrap()).unwrap();
    let left  = line((fixed(0.0), fixed(0.0)).into(), (fixed(3.0), fixed(2.0)).into());
    let right = line((fixed(2.0), fixed(0.0)).into(), (fixed(0.0), fixed(2.0)).into());
    assert_eq!(crossing_event(left, right), Some(FixedCrossing { y: 205, x: 307 }));
}

#[test] fn randomized_fixed_quadrilaterals_track_the_f32_reference() {
    let mut state = 0xd431_72a9_u32;
    let mut coordinate = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        FixedScalar::from_bits((state % 2048) as i32 - 256)
    };
    for case in 0..256 {
        let points: [Point<FixedScalar>; 4] =
            core::array::from_fn(|_| (coordinate(), coordinate()).into());
        let mut fixed_edges = Vec::new();
        for index in 0..points.len() {
            if let Some(edge) = Edge::from_line(points[index],
                points[(index + 1) % points.len()]) {
                fixed_edges.push(edge);
            }
        }
        let float_edges: Vec<Edge> = fixed_edges.iter().map(|edge| Edge {
            upper: (edge.upper.x.to_num(), edge.upper.y.to_num()).into(),
            lower: (edge.lower.x.to_num(), edge.lower.y.to_num()).into(),
            winding: edge.winding,
        }).collect();
        for fill_rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let fixed_pixels = render(&fixed_edges, 6, 6, fill_rule);
            let float_pixels = render_analytic(&float_edges, 6, 6, fill_rule);
            for (pixel, (fixed, reference)) in
                fixed_pixels.iter().zip(&float_pixels).enumerate() {
                assert!(fixed.abs_diff(*reference) <= 2,
                    "case {case}, pixel {pixel}, {fill_rule:?}, points={points:?}: \
                        fixed={fixed}, f32={reference}");
            }
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

#[test] fn strip_bins_are_compact_bounded_and_transactional() {
    let line = |x, top, bottom| FixedLine::new(Edge {
        upper: (fixed(x), fixed(top)).into(),
        lower: (fixed(x), fixed(bottom)).into(), winding: 1,
    }).unwrap();
    let lines = [line(0.0, 0.0, 16.0), line(1.0, 15.5, 32.5), line(2.0, -10.0, -1.0)];
    assert_eq!(fixed_strip_requirements(&lines, 64),
        Ok(FixedStripRequirements { offsets: 5, indices: 4 }));

    let (mut offsets, mut indices) = ([0; 5], [0; 4]);
    let bins = build_strip_bins(&lines, 64, &mut offsets, &mut indices).unwrap();
    assert_eq!(bins.offsets, [0, 2, 3, 4, 4]);
    assert_eq!(bins.indices(0), [0, 1]);
    assert_eq!(bins.indices(1), [1]);
    assert_eq!(bins.indices(2), [1]);
    assert!(bins.indices(3).is_empty());

    let (mut offsets, mut indices) = ([7; 5], [9; 4]);
    assert_eq!(build_strip_bins(&lines, 64, &mut offsets[..4], &mut indices).unwrap_err(),
        FixedRasterError::WorkspaceTooSmall {
            kind: FixedWorkspace::StripOffsets, required: 5,
        });
    assert_eq!((offsets, indices), ([7; 5], [9; 4]));
    assert_eq!(build_strip_bins(&lines, 64, &mut offsets, &mut indices[..3]).unwrap_err(),
        FixedRasterError::WorkspaceTooSmall {
            kind: FixedWorkspace::StripIndices, required: 4,
        });
    assert_eq!((offsets, indices), ([7; 5], [9; 4]));
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
    let sentinel = FixedSegment { line_index: 0, top_y: 7, bottom_y: 9,
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
    let segment = |top_x, bottom_x, winding| FixedSegment {
        line_index: 0, top_y: 0, bottom_y: 256,
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
    let segment = |top_x, bottom_x, winding| FixedSegment {
        line_index: 0, top_y: 0, bottom_y: 256,
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
        line_index: 0, top_y, bottom_y,
            top_x: FixedIntersection { num:    top_x, den: 1, winding },
        bottom_x: FixedIntersection { num: bottom_x, den: 1, winding },
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
        line_index: 0, top_y, bottom_y,
            top_x: FixedIntersection { num:    top_x, den: 1, winding },
        bottom_x: FixedIntersection { num: bottom_x, den: 1, winding },
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
    let segment = |top_y, bottom_y, x, winding| FixedSegment {
        line_index: 0, top_y, bottom_y,
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
    let segment = |x, winding| FixedSegment {
        line_index: 0, top_y: 0, bottom_y: 256,
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
        line_index: 0, top_y, bottom_y,
            top_x: FixedIntersection { num:    top_x, den: 1, winding },
        bottom_x: FixedIntersection { num: bottom_x, den: 1, winding },
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
