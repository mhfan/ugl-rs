
use super::*;
use alloc::{vec, vec::Vec};
use core::convert::Infallible;
use crate::analytic::{Intersection as AnalyticIntersection,
    Workspace as AnalyticWorkspace, rasterize_edges as rasterize_edges_analytic};

fn fixed(value: f32) -> Scalar { Scalar::from_num(value) }

fn polygon_edges<T: Copy + PartialOrd>(points: &[Point<T>]) -> Vec<Edge<T>> {
    let mut edges = Vec::new();
    for index in 0..points.len() {
        if let Some(edge) = Edge::from_line(
            points[index], points[(index + 1) % points.len()]) {
            edges.push(edge);
        }
    }
    edges
}

fn assert_coverage_near(
    actual: &[u8], expected: &[u8], tolerance: u8, context: impl core::fmt::Display) {
    assert_eq!(actual.len(), expected.len(), "{context}: coverage dimensions differ");
    for (pixel, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
        assert!(actual.abs_diff(expected) <= tolerance,
            "{context}, pixel {pixel}: actual={actual}, expected={expected}");
    }
}

fn render(edges: &[Edge<Scalar>], width: usize, height: usize,
    fill_rule: FillRule) -> Vec<u8> {
    let mut lines = vec![Line::default(); edges.len()];
    prepare_lines(edges, &mut lines).unwrap();
    let requirements = strip_requirements(&lines, height as _).unwrap();
    let (mut segments, mut trapezoids, mut row_area, mut strip_offsets, mut strip_indices) = (
        vec![Segment::default(); lines.len()],
        vec![Trapezoid::default(); lines.len().div_ceil(2)],
        vec![0; width], vec![0; requirements.offsets], vec![0; requirements.indices],
    );
    let mut pixels = vec![0; width * height];
    rasterize_lines(&lines, width as _, height as _, fill_rule,
        &mut Workspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
        }, &mut |x, y, coverage| {
        pixels[y as usize * width + x as usize] = coverage;
        Ok::<_, Infallible>(())
    }).unwrap();
    pixels
}

fn render_region(edges: &[Edge<Scalar>], width: usize, height: usize,
    region: (u32, u32, u32, u32), fill_rule: FillRule) -> Vec<u8> {
    let mut lines = vec![Line::default(); edges.len()];
    prepare_lines(edges, &mut lines).unwrap();
    let requirements = strip_requirements(&lines, height as _).unwrap();
    let (mut segments, mut trapezoids, mut row_area, mut strip_offsets, mut strip_indices) = (
        vec![Segment::default(); lines.len()],
        vec![Trapezoid::default(); lines.len().div_ceil(2)],
        vec![0; (region.2 - region.0) as usize],
        vec![0; requirements.offsets], vec![0; requirements.indices]);
    let mut pixels = vec![0; width * height];
    rasterize_lines_region(&lines, width as _, height as _, region, fill_rule,
        &mut Workspace { segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
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

#[test] fn local_region_matches_full_raster_with_spanning_lines() {
    let points = [(-1.0, -1.0), (9.0, 1.25), (7.5, 9.0), (0.5, 7.25)]
        .map(|(x, y)| (fixed(x), fixed(y)).into());
    let edges = polygon_edges(&points);
    let (width, height, region) = (8, 8, (2, 2, 7, 7));
    let full = render(&edges, width, height, FillRule::NonZero);
    let local = render_region(&edges, width, height, region, FillRule::NonZero);
    for y in 0..height as u32 { for x in 0..width as u32 {
        let index = (y * width as u32 + x) as usize;
        if x >= region.0 && x < region.2 && y >= region.1 && y < region.3 {
            assert_eq!(local[index], full[index], "({x}, {y})");
        } else { assert_eq!(local[index], 0, "({x}, {y})"); }
    } }
}

#[test] fn diagonal_intersection_is_exact_in_raw_subpixels() {
    let edge = Edge::from_line((fixed(0.0), fixed(0.0)).into(),
                                (fixed(1.0), fixed(1.0)).into()).unwrap();
    let intersection = Line::new(edge).unwrap().intersection(fixed(0.5));
    assert_eq!(intersection.floor_raw(), 128);
}

#[test] fn exact_device_limit_is_accepted_and_next_raw_unit_is_rejected() {
    let limit = Scalar::from_bits(DEVICE_RAW_LIMIT);
    let negative_limit = Scalar::from_bits(-DEVICE_RAW_LIMIT);
    let line = Line::new(Edge {
        upper: (negative_limit, Scalar::ZERO).into(),
        lower: (limit, Scalar::ONE).into(), winding: 1,
    }).unwrap();
    assert_eq!(line.intersection(Scalar::ZERO).floor_raw(), -DEVICE_RAW_LIMIT as i64);

    let outside = Scalar::from_bits(DEVICE_RAW_LIMIT + 1);
    assert_eq!(Line::new(Edge {
        upper: (outside, Scalar::ZERO).into(),
        lower: (limit, Scalar::ONE).into(), winding: 1,
    }), Err(Error::CoordinateOutOfRange));
}

#[test] fn rasterizer_renders_aligned_and_fractional_rectangles() {
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

#[test] fn rasterizer_supports_both_fill_rules_end_to_end() {
    let edge = |x, winding| Edge {
        upper: (fixed(x), fixed(0.0)).into(),
        lower: (fixed(x), fixed(1.0)).into(), winding,
    };
    let edges = [edge(0.0, 1), edge(4.0, -1), edge(1.0, 1), edge(3.0, -1)];
    assert_eq!(render(&edges, 4, 1, FillRule::NonZero), [255; 4]);
    assert_eq!(render(&edges, 4, 1, FillRule::EvenOdd), [255, 0, 0, 255]);
}

#[test] fn triangles_track_the_f32_analytic_reference() {
    let mut state = 0x8f31_7a2d_u32;
    let mut random_raw = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state % (7 * SUBPIXEL_SCALE)) as i32 - SUBPIXEL_SCALE as i32
    };
    for case in 0..512 {
        let points: [Point<Scalar>; 3] = core::array::from_fn(|_| (
            Scalar::from_bits(random_raw()), Scalar::from_bits(random_raw())).into());
        let fixed_edges = polygon_edges(&points);
        let float_edges: Vec<Edge> = fixed_edges.iter().map(|edge| Edge {
            upper: (edge.upper.x.to_num(), edge.upper.y.to_num()).into(),
            lower: (edge.lower.x.to_num(), edge.lower.y.to_num()).into(),
            winding: edge.winding,
        }).collect();
        assert_coverage_near(
            &render(&fixed_edges, 6, 6, FillRule::NonZero),
            &render_analytic(&float_edges, 6, 6, FillRule::NonZero),
            2, format_args!("triangle case {case}"));
    }
}

#[test] fn self_intersections_track_the_f32_analytic_reference() {
    let scenes = [[(0, 0), (512, 512), (0, 512), (512, 0)],
                    [(32, 17), (737, 491), (61, 690), (689, 3)],
                    [(-64, 100), (800, 600), (0, 700), (720, -20)]];
    for (case, points) in scenes.into_iter().enumerate() {
        let points = points.map(|(x, y)|
            (Scalar::from_bits(x), Scalar::from_bits(y)).into());
        let fixed_edges = polygon_edges(&points);
        let float_edges: Vec<Edge> = fixed_edges.iter().map(|edge| Edge {
            upper: (edge.upper.x.to_num(), edge.upper.y.to_num()).into(),
            lower: (edge.lower.x.to_num(), edge.lower.y.to_num()).into(),
            winding: edge.winding,
        }).collect();
        for fill_rule in [FillRule::NonZero, FillRule::EvenOdd] {
            assert_coverage_near(
                &render(&fixed_edges, 3, 3, fill_rule),
                &render_analytic(&float_edges, 3, 3, fill_rule),
                2, format_args!("self-intersection case {case}, {fill_rule:?}"));
        }
    }
}

#[test] fn rational_crossing_events_round_only_at_the_area_boundary() {
    let line = |from, to| Line::new(Edge::from_line(from, to).unwrap()).unwrap();
    let left  = line((fixed(0.0), fixed(0.0)).into(), (fixed(3.0), fixed(2.0)).into());
    let right = line((fixed(2.0), fixed(0.0)).into(), (fixed(0.0), fixed(2.0)).into());
    assert_eq!(crossing_event(left, right), Some(Crossing { y: 205, x: 307 }));
}

#[test] fn randomized_quadrilaterals_track_the_f32_reference() {
    let mut state = 0xd431_72a9_u32;
    let mut coordinate = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        Scalar::from_bits((state % 2048) as i32 - 256)
    };
    for case in 0..256 {
        let points: [Point<Scalar>; 4] =
            core::array::from_fn(|_| (coordinate(), coordinate()).into());
        let fixed_edges = polygon_edges(&points);
        let float_edges: Vec<Edge> = fixed_edges.iter().map(|edge| Edge {
            upper: (edge.upper.x.to_num(), edge.upper.y.to_num()).into(),
            lower: (edge.lower.x.to_num(), edge.lower.y.to_num()).into(),
            winding: edge.winding,
        }).collect();
        for fill_rule in [FillRule::NonZero, FillRule::EvenOdd] {
            assert_coverage_near(
                &render(&fixed_edges, 6, 6, fill_rule),
                &render_analytic(&float_edges, 6, 6, fill_rule),
                2, format_args!(
                    "quadrilateral case {case}, {fill_rule:?}, points={points:?}"));
        }
    }
}

#[test] fn rational_order_handles_negative_values_and_different_denominators() {
    let  left = Intersection { num: -3, den: 2, winding: 1 };
    let right = Intersection { num: -4, den: 3, winding: -1 };
    assert_eq!(left.floor_raw(), -2);
    assert_eq!(left.cmp_x(&right), Ordering::Less);

    let half = Intersection { num: 1, den: 2, winding: 1 };
    let same = Intersection { num: 2, den: 4, winding: -1 };
    assert_eq!(half.cmp_x(&same), Ordering::Equal);
}

#[test] fn rational_rounding_is_symmetric_at_half_subpixels() {
    let value = |num| Intersection { num, den: 2, winding: 1 };
    assert_eq!(value( 1).round_raw(),  1);
    assert_eq!(value(-1).round_raw(), -1);
    assert_eq!(value( 3).round_raw(),  2);
    assert_eq!(value(-3).round_raw(), -2);
    assert_eq!(value( 2).round_raw(),  1);
}

#[test] fn coordinate_limit_is_explicit() {
    let outside = Scalar::from_bits(DEVICE_RAW_LIMIT + 1);
    let edge = Edge::from_line((Scalar::ZERO, Scalar::ZERO).into(),
        (outside, Scalar::ONE).into()).unwrap();
    assert_eq!(Line::new(edge), Err(Error::CoordinateOutOfRange));
}

#[test] fn manually_constructed_invalid_edges_are_rejected() {
    let edge = Edge {
        upper: (Scalar::ZERO, Scalar::ONE).into(),
        lower: (Scalar::ONE, Scalar::ZERO).into(), winding: 1,
    };
    assert_eq!(Line::new(edge), Err(Error::InvalidEdge));
}

#[test] fn line_preparation_is_bounded_and_transactional() {
    let edge = |x, winding| Edge {
        upper: (fixed(x), fixed(0.0)).into(),
        lower: (fixed(x), fixed(1.0)).into(), winding,
    };
    let sentinel = Line::new(edge(7.0, 1)).unwrap();
    let mut output = [sentinel; 2];

    assert_eq!(prepare_lines(&[edge(0.0, 1), edge(1.0, -1)], &mut output), Ok(2));
    assert_eq!(output[0].intersection(fixed(0.5)).floor_raw(), 0);
    output = [sentinel; 2];
    assert_eq!(prepare_lines(&[edge(0.0, 1), edge(1.0, 0)], &mut output),
        Err(Error::InvalidEdge));
    assert_eq!(output, [sentinel; 2]);
    assert_eq!(prepare_lines(&[edge(0.0, 1), edge(1.0, -1)], &mut output[..1]),
        Err(Error::WorkspaceTooSmall {
            kind: WorkspaceKind::Lines, required: 2,
        }));
        assert_eq!(output, [sentinel; 2]);
}

#[test] fn strip_bins_are_compact_bounded_and_transactional() {
    let line = |x, top, bottom| Line::new(Edge {
        upper: (fixed(x), fixed(top)).into(),
        lower: (fixed(x), fixed(bottom)).into(), winding: 1,
    }).unwrap();
    let lines = [line(0.0, 0.0, 16.0), line(1.0, 15.5, 32.5), line(2.0, -10.0, -1.0)];
    assert_eq!(strip_requirements(&lines, 64),
        Ok(StripRequirements { offsets: 5, indices: 4 }));

    let (mut offsets, mut indices) = ([0; 5], [0; 4]);
    let bins = build_strip_bins(&lines, 64, &mut offsets, &mut indices).unwrap();
    assert_eq!(bins.offsets, [0, 2, 3, 4, 4]);
    assert_eq!(bins.indices(0), [0, 1]);
    assert_eq!(bins.indices(1), [1]);
    assert_eq!(bins.indices(2), [1]);
    assert!(bins.indices(3).is_empty());

    let (mut offsets, mut indices) = ([7; 5], [9; 4]);
    assert_eq!(build_strip_bins(&lines, 64, &mut offsets[..4], &mut indices).unwrap_err(),
        Error::WorkspaceTooSmall {
            kind: WorkspaceKind::StripOffsets, required: 5,
        });
    assert_eq!((offsets, indices), ([7; 5], [9; 4]));
    assert_eq!(build_strip_bins(&lines, 64, &mut offsets, &mut indices[..3]).unwrap_err(),
        Error::WorkspaceTooSmall {
            kind: WorkspaceKind::StripIndices, required: 4,
        });
    assert_eq!((offsets, indices), ([7; 5], [9; 4]));
}

#[test] fn retained_coverage_is_compact_sparse_and_replays_exactly() {
    assert_eq!(core::mem::size_of::<CoverageRun>(), 12);
    let edge = |x, top, bottom, winding| Edge {
        upper: (fixed(x), fixed(top)).into(),
        lower: (fixed(x), fixed(bottom)).into(), winding,
    };
    let edges = [
        edge(0.5, 0.5, 20.25, 1), edge(2.5, 0.5, 20.25, -1),
        edge(1.0, 32.0, 33.0, 1), edge(3.0, 32.0, 33.0, -1),
    ];
    let mut lines = [Line::default(); 4];
    prepare_lines(&edges, &mut lines).unwrap();
    let requirements = strip_requirements(&lines, 40).unwrap();
    let (mut segments, mut trapezoids, mut row_area, mut offsets, mut indices) = (
        [Segment::default(); 4], [Trapezoid::default(); 2], [0; 4],
        vec![0; requirements.offsets], vec![0; requirements.indices],
    );
    let mut raster_workspace = Workspace {
        segments: &mut segments, trapezoids: &mut trapezoids, row_area: &mut row_area,
        strip_offsets: &mut offsets, strip_indices: &mut indices,
    };
    let (mut strips, mut runs) = ([CoverageStrip::default(); 3],
                                  [CoverageRun::default(); 64]);
    let retained = rasterize_lines_to_strips(&lines, 4, 40, FillRule::NonZero,
        &mut raster_workspace,
        CoverageWorkspace { strips: &mut strips, runs: &mut runs }).unwrap();

    assert_eq!(retained.strips().iter().map(|strip| strip.y).collect::<Vec<_>>(),
        [0, 16, 32]);
    assert!(retained.runs().iter().all(|run| run.row < STRIP_HEIGHT as u8));
    let mut replayed = vec![0; 4 * 40];
    retained.replay(&mut |x, y, coverage| {
        replayed[y as usize * 4 + x as usize] = coverage;
        Ok::<_, Infallible>(())
    }).unwrap();
    assert_eq!(replayed, render(&edges, 4, 40, FillRule::NonZero));
}

#[test] fn retained_coverage_reports_each_caller_owned_capacity() {
    let edges = [
        Edge { upper: (fixed(0.0), fixed(0.0)).into(),
               lower: (fixed(0.0), fixed(1.0)).into(), winding: 1 },
        Edge { upper: (fixed(1.0), fixed(0.0)).into(),
               lower: (fixed(1.0), fixed(1.0)).into(), winding: -1 },
    ];
    let mut lines = [Line::default(); 2];  prepare_lines(&edges, &mut lines).unwrap();
    let requirements = strip_requirements(&lines, 1).unwrap();
    let (mut segments, mut trapezoids, mut row_area, mut offsets, mut indices) = (
        [Segment::default(); 2], [Trapezoid::default(); 1], [0; 1],
        vec![0; requirements.offsets], vec![0; requirements.indices],
    );
    let mut raster_workspace = Workspace {
        segments: &mut segments, trapezoids: &mut trapezoids, row_area: &mut row_area,
        strip_offsets: &mut offsets, strip_indices: &mut indices,
    };
    let mut run = [CoverageRun::default(); 1];
    assert_eq!(rasterize_lines_to_strips(&lines, 1, 1, FillRule::NonZero,
        &mut raster_workspace,
        CoverageWorkspace { strips: &mut [], runs: &mut run }).unwrap_err(),
        Error::WorkspaceTooSmall {
            kind: WorkspaceKind::CoverageStrips, required: 1,
        });

    let mut strip = [CoverageStrip::default(); 1];
    assert_eq!(rasterize_lines_to_strips(&lines, 1, 1, FillRule::NonZero,
        &mut raster_workspace,
        CoverageWorkspace { strips: &mut strip, runs: &mut [] }).unwrap_err(),
        Error::WorkspaceTooSmall {
            kind: WorkspaceKind::CoverageRuns, required: 1,
        });
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
    let mut lines = [Line::default(); 4];
    prepare_lines(&edges, &mut lines).unwrap();

    let first = next_slab_boundary(&lines, fixed(0.0), fixed(2.0)).unwrap();
    let second = next_slab_boundary(&lines, first, fixed(2.0)).unwrap();
    let third = next_slab_boundary(&lines, second, fixed(2.0)).unwrap();
    assert_eq!((first, second, third), (fixed(0.5), fixed(1.5), fixed(2.0)));

    let mut segments = [Segment::default(); 4];
    let count = collect_segments(&lines, first, second, &mut segments).unwrap();
    assert_eq!(count, 4);
    assert!(segments[..count].iter().all(|segment|
        segment.top_y() == first && segment.bottom_y() == second));
    let mut trapezoids = [Trapezoid::default(); 2];
    assert_eq!(collect_trapezoids(&mut segments[..count], FillRule::EvenOdd,
        &mut trapezoids), Ok(2));
}

#[test] fn slab_clipping_preserves_exact_boundary_intersections() {
    let line = Line::new(Edge::from_line(
        (fixed(-1.0), fixed(-1.0)).into(),
        (fixed(2.0),  fixed(2.0) ).into()).unwrap()).unwrap();
    let mut segments = [Segment::default(); 1];

    assert_eq!(collect_segments(&[line], fixed(0.0), fixed(1.0), &mut segments), Ok(1));
    assert_eq!((segments[0].top_y(), segments[0].bottom_y()), (fixed(0.0), fixed(1.0)));
    assert_eq!((segments[0].top_x.floor_raw(),
                segments[0].bottom_x.floor_raw()), (0, 256));
    assert_eq!( segments[0].height_raw(), 256);
    assert_eq!(collect_segments(&[line], fixed(3.0), fixed(4.0), &mut segments), Ok(0));
}

#[test] fn slab_errors_do_not_modify_output() {
    let sentinel = Segment { line_index: 0, top_y: 7, bottom_y: 9,
            top_x: Intersection::default(),
        bottom_x: Intersection::default(),
    };
    let line = Line::new(Edge::from_line(
        (fixed(0.0), fixed(0.0)).into(),
        (fixed(1.0), fixed(1.0)).into()).unwrap()).unwrap();
    let mut output = [sentinel];

    assert_eq!(collect_segments(&[line], fixed(1.0), fixed(1.0), &mut output),
        Err(Error::InvalidSlab));
    assert_eq!(output, [sentinel]);
    assert_eq!(collect_segments(&[line], fixed(0.0), fixed(1.0), &mut []),
        Err(Error::WorkspaceTooSmall {
            kind: WorkspaceKind::Segments, required: 1,
        }));
    assert_eq!(output, [sentinel]);
}

#[test] fn slab_segments_form_rectangular_and_triangular_trapezoids() {
    let segment = |top_x, bottom_x, winding| Segment {
        line_index: 0, top_y: 0, bottom_y: 256,
            top_x: Intersection { num:    top_x, den: 1, winding },
        bottom_x: Intersection { num: bottom_x, den: 1, winding },
    };
    let mut output = [Trapezoid::default(); 1];
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
    let segment = |top_x, bottom_x, winding| Segment {
        line_index: 0, top_y: 0, bottom_y: 256,
            top_x: Intersection { num:    top_x, den: 1, winding },
        bottom_x: Intersection { num: bottom_x, den: 1, winding },
    };
    let rectangle = Trapezoid {
        left: segment(0, 0, 1), right: segment(256, 256, -1),
    };
    let triangle = Trapezoid {
        left: segment(128, 0, 1), right: segment(128, 256, -1),
    };
    assert_eq!(rectangle.area_twice_raw(), Ok(PIXEL_AREA_TWICE));
    assert_eq!(quantize_area_coverage(rectangle.area_twice_raw().unwrap()), 255);
    assert_eq!(triangle.area_twice_raw(), Ok(PIXEL_AREA_TWICE / 2));
    assert_eq!(quantize_area_coverage(triangle.area_twice_raw().unwrap()), 128);
    assert_eq!(quantize_area_coverage(PIXEL_AREA_TWICE * 2), 255);

    let inverted = Trapezoid { left: rectangle.right, right: rectangle.left };
    assert_eq!(inverted.area_twice_raw(), Err(Error::InvalidTrapezoid));
}

#[test] fn clamped_edge_integral_is_exact_for_constant_ramp_and_crossing_cases() {
    let (scale, height) = (SUBPIXEL_SCALE as i64, SUBPIXEL_SCALE);
    assert_eq!(integrate_clamped_edge_twice(scale / 2, scale / 2, height),
        PIXEL_AREA_TWICE / 2);
    assert_eq!(integrate_clamped_edge_twice(0, scale, height), PIXEL_AREA_TWICE / 2);
    assert_eq!(integrate_clamped_edge_twice(scale, 0, height), PIXEL_AREA_TWICE / 2);
    assert_eq!(integrate_clamped_edge_twice(-scale, scale, height),
        PIXEL_AREA_TWICE / 4);
    assert_eq!(integrate_clamped_edge_twice(scale, -scale, height),
        PIXEL_AREA_TWICE / 4);
    assert_eq!(integrate_clamped_edge_twice(scale, scale * 2, height),
        PIXEL_AREA_TWICE);
}

#[test] fn trapezoid_extracts_only_guaranteed_full_pixel_runs() {
    let segment = |top_y, bottom_y, top_x, bottom_x, winding| Segment {
        line_index: 0, top_y, bottom_y,
            top_x: Intersection { num:    top_x, den: 1, winding },
        bottom_x: Intersection { num: bottom_x, den: 1, winding },
    };
    let aligned = Trapezoid {
            left: segment(0, 256, 256, 256, 1),
        right: segment(0, 256, 1024, 1024, -1),
    };
    assert_eq!(aligned.full_pixel_range(8), Ok(1..4));

    let slanted = Trapezoid {
            left: segment(0, 256, 128, 256, 1),
        right: segment(0, 256, 896, 768, -1),
    };
    assert_eq!(slanted.full_pixel_range(8), Ok(1..3));

    let clipped = Trapezoid {
            left: segment(0, 256, -512, -256, 1),
        right: segment(0, 256, 512, 768, -1),
    };
    assert_eq!(clipped.full_pixel_range(2), Ok(0..2));

    let partial_height = Trapezoid {
            left: segment(0, 128, 0, 0, 1),
        right: segment(0, 128, 512, 512, -1),
    };
    assert_eq!(partial_height.full_pixel_range(8), Ok(0..0));
}

#[test] fn trapezoid_clips_boundary_pixels_without_allocation() {
    let segment = |top_y, bottom_y, top_x, bottom_x, winding| Segment {
        line_index: 0, top_y, bottom_y,
            top_x: Intersection { num:    top_x, den: 1, winding },
        bottom_x: Intersection { num: bottom_x, den: 1, winding },
    };
    let centered = Trapezoid {
            left: segment(0, 256, 128, 128, 1),
        right: segment(0, 256, 384, 384, -1),
    };
    assert_eq!(centered.pixel_area_twice_raw(0, 0), Ok(PIXEL_AREA_TWICE / 2));
    assert_eq!(centered.pixel_area_twice_raw(1, 0), Ok(PIXEL_AREA_TWICE / 2));
    assert_eq!(centered.pixel_area_twice_raw(2, 0), Ok(0));

    let diagonal = Trapezoid {
            left: segment(0, 256, 0, 256, 1),
        right: segment(0, 256, 256, 256, -1),
    };
    let area = diagonal.pixel_area_twice_raw(0, 0).unwrap();
    assert_eq!(area, PIXEL_AREA_TWICE / 2);
    assert_eq!(quantize_area_coverage(area), 128);

    let partial_height = Trapezoid {
            left: segment(128, 256, 0, 0, 1),
        right: segment(128, 256, 256, 256, -1),
    };
    assert_eq!(partial_height.pixel_area_twice_raw(0, 0), Ok(PIXEL_AREA_TWICE / 2));
    assert_eq!(partial_height.pixel_area_twice_raw(0, 1),
        Err(Error::InvalidSlabPartition));
}

#[test] fn slab_areas_accumulate_before_quantization_and_emit_as_runs() {
    let segment = |top_y, bottom_y, x, winding| Segment {
        line_index: 0, top_y, bottom_y,
            top_x: Intersection { num: x, den: 1, winding },
        bottom_x: Intersection { num: x, den: 1, winding },
    };
    let trapezoid = |top_y, bottom_y| Trapezoid {
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
    let segment = |x, winding| Segment {
        line_index: 0, top_y: 0, bottom_y: 256,
            top_x: Intersection { num: x, den: 1, winding },
        bottom_x: Intersection { num: x, den: 1, winding },
    };
    let trapezoid = Trapezoid { left: segment(128, 1), right: segment(896, -1) };
    let mut row = [0; 4];
    accumulate_trapezoid_row(trapezoid, 4, 0, &mut row).unwrap();
    assert_eq!(row, [PIXEL_AREA_TWICE / 2, PIXEL_AREA_TWICE,
                        PIXEL_AREA_TWICE, PIXEL_AREA_TWICE / 2]);
    assert_eq!(row.map(quantize_area_coverage), [128, 255, 255, 128]);
    assert_eq!(accumulate_trapezoid_row(trapezoid, 4, 0, &mut row[..3]),
        Err(Error::WorkspaceTooSmall {
            kind: WorkspaceKind::RowArea, required: 4,
        }));
}

#[test] fn trapezoid_construction_rejects_crossings_and_unpartitioned_slabs() {
    let segment = |top_y, bottom_y, top_x, bottom_x, winding| Segment {
        line_index: 0, top_y, bottom_y,
            top_x: Intersection { num:    top_x, den: 1, winding },
        bottom_x: Intersection { num: bottom_x, den: 1, winding },
    };
    let mut crossing = [ segment(0, 256, 0, 256, 1), segment(0, 256, 256, 0, -1) ];
    assert_eq!(collect_trapezoids(&mut crossing, FillRule::NonZero, &mut []),
        Err(Error::CrossingEdges));

    let mut unpartitioned = [segment(0, 128, 0, 0, 1), segment(0, 256, 256, 256, -1)];
    assert_eq!(collect_trapezoids(&mut unpartitioned, FillRule::NonZero, &mut []),
        Err(Error::InvalidSlabPartition));
}

#[test] fn scanline_collection_is_half_open_sorted_and_bounded() {
    let line = |from, to| Line::new(Edge::from_line(from, to).unwrap()).unwrap();
    let lines = [
        line((fixed(2.0), fixed(0.0)).into(), (fixed(1.0), fixed(1.0)).into()),
        line((fixed(0.0), fixed(0.0)).into(), (fixed(0.0), fixed(2.0)).into()),
    ];
    let mut intersections = [Intersection::default(); 2];

    assert_eq!(collect_intersections(&lines, fixed(0.5), &mut intersections), Ok(2));
    assert_eq!(intersections.map(Intersection::floor_raw), [0, 384]);
    assert_eq!(collect_intersections(&lines, fixed(1.0), &mut intersections), Ok(1));
    assert_eq!(intersections[0].floor_raw(), 0);
    assert_eq!(collect_intersections(&lines, fixed(0.5), &mut intersections[..1]),
        Err(Error::WorkspaceTooSmall {
            kind: WorkspaceKind::Intersections, required: 2,
        }));
}

#[test] fn crossing_events_form_exact_spans_for_both_fill_rules() {
    let crossing = |x, winding| Intersection { num: x, den: 1, winding, };
    let intersections = [crossing(0, 1), crossing(1, 1),
        crossing(2, -1), crossing(3, -1)];
    let mut spans = [Span::default(); 2];

    assert_eq!(collect_spans(&intersections, FillRule::NonZero, &mut spans), Ok(1));
    assert_eq!((spans[0].from.floor_raw(), spans[0].to.floor_raw()), (0, 3));
    assert_eq!(collect_spans(&intersections, FillRule::EvenOdd, &mut spans), Ok(2));
    assert_eq!(spans[..2].iter().map(|span|
        (span.from.floor_raw(), span.to.floor_raw())).collect::<alloc::vec::Vec<_>>(),
        [(0, 1), (2, 3)]);
}

#[test] fn coincident_crossings_are_grouped_and_errors_do_not_write_output() {
    let crossing = |x, winding| Intersection { num: x, den: 1, winding };
    let mut output = [Span { from: crossing(7, 0), to: crossing(9, 0) }];
    let sentinel = output;
    assert_eq!(collect_spans(&[crossing(0, 1), crossing(0, -1)],
        FillRule::NonZero, &mut output), Ok(0));
    assert_eq!(output, sentinel);

    assert_eq!(collect_spans(&[crossing(1, 1), crossing(0, -1)],
        FillRule::NonZero, &mut output), Err(Error::InvalidIntersectionOrder));
    assert_eq!(output, sentinel);
    assert_eq!(collect_spans(&[crossing(0, 1), crossing(1, -1), crossing(2, 1),
        crossing(3, -1)], FillRule::EvenOdd, &mut []),
        Err(Error::WorkspaceTooSmall {
            kind: WorkspaceKind::Spans, required: 2,
        }));
}
