
use ugl_rs::{analytic::AnalyticIntersection, color::{PRGB32, RGBA}, edge::Edge,
    canvas::{AnalyticRenderOptions, AnalyticRenderWorkspace, AnalyticStrokeOptions,
        AnalyticStrokeWorkspace, PixmapMut, render_paint_analytic, render_solid_analytic,
        render_stroke_solid_analytic,
    }, geometry::{Affine, PathBuilder}, raster::FillRule,
    sampler::{GradientStop, GradientStops, LinearGradient, PaintSampler, SpreadMode},
    stroke::StrokeContour,
};

const  WIDTH: u32 = 4;
const HEIGHT: u32 = 4;

fn render_analytic(builder: PathBuilder, fill_rule: FillRule) -> [PRGB32<u8>; 16] {
    let mut bytes = [0; WIDTH as usize * HEIGHT as usize * 4];
    let mut target = PixmapMut::new(&mut bytes, WIDTH, HEIGHT, WIDTH * 4).unwrap();
    let (mut edges, mut intersections, mut row_coverage) = (
        [Edge::default(); 8], [AnalyticIntersection::default(); 8], [0.0; WIDTH as usize],
    );
    render_solid_analytic(&builder.build(), Affine::identity(), RGBA::new(20, 200, 40, 160),
        AnalyticRenderOptions { fill_rule, ..Default::default() }, &mut target,
        &mut AnalyticRenderWorkspace {
            edges: &mut edges, intersections: &mut intersections,
            row_coverage: &mut row_coverage,
        },
    ).unwrap();
    core::array::from_fn(|index| target.pixel(index as u32 % WIDTH, index as u32 / WIDTH).unwrap())
}

fn render_analytic_paint(builder: PathBuilder, sampler: &impl PaintSampler) ->
    [PRGB32<u8>; 16] {
    let mut bytes = [0; WIDTH as usize * HEIGHT as usize * 4];
    let mut target = PixmapMut::new(&mut bytes, WIDTH, HEIGHT, WIDTH * 4).unwrap();
    let (mut edges, mut intersections, mut row_coverage) = (
        [Edge::default(); 8], [AnalyticIntersection::default(); 8], [0.0; WIDTH as usize],
    );
    render_paint_analytic(&builder.build(), Affine::identity(), sampler,
        AnalyticRenderOptions::default(), &mut target, &mut AnalyticRenderWorkspace {
            edges: &mut edges, intersections: &mut intersections,
            row_coverage: &mut row_coverage,
        },
    ).unwrap();
    core::array::from_fn(|index|
        target.pixel(index as u32 % WIDTH, index as u32 / WIDTH).unwrap())
}

fn render_analytic_stroke(builder: PathBuilder) -> [PRGB32<u8>; 16] {
    let mut bytes = [0; WIDTH as usize * HEIGHT as usize * 4];
    let mut target = PixmapMut::new(&mut bytes, WIDTH, HEIGHT, WIDTH * 4).unwrap();
    let (mut points, mut contours, mut edges) = (
        [Default::default(); 2], [StrokeContour::default(); 1], [Edge::default(); 4],
    );
    let (mut intersections, mut row_coverage) = (
        [AnalyticIntersection::default(); 4], [0.0; WIDTH as usize],
    );
    render_stroke_solid_analytic(&builder.build(), Affine::identity(),
        RGBA::new(20, 200, 40, 160), AnalyticStrokeOptions::default(), &mut target,
        &mut AnalyticStrokeWorkspace {
            points: &mut points, contours: &mut contours, edges: &mut edges,
            intersections: &mut intersections, row_coverage: &mut row_coverage,
        }).unwrap();
    core::array::from_fn(|index|
        target.pixel(index as u32 % WIDTH, index as u32 / WIDTH).unwrap())
}

#[test] fn aligned_rectangle_rgba_golden() {
    let mut path = PathBuilder::new();
    path.move_to((1.0, 1.0)).line_to((3.0, 1.0))
        .line_to((3.0, 3.0)).line_to((1.0, 3.0));

    let transparent = PRGB32::zeroed();
    let solid: PRGB32<u8> = (13, 125, 25, 160).into();
    assert_eq!(render_analytic(path, FillRule::NonZero), [
        transparent, transparent, transparent, transparent,
        transparent, solid,       solid,       transparent,
        transparent, solid,       solid,       transparent,
        transparent, transparent, transparent, transparent,
    ]);
}

#[test] fn diagonal_triangle_rgba_golden() {
    let mut path = PathBuilder::new();
    path.move_to((0.0, 0.0)).line_to((2.0, 0.0)).line_to((0.0, 2.0));

    let transparent = PRGB32::zeroed();
    let solid: PRGB32<u8> = (13, 125, 25, 160).into();
    let half: PRGB32<u8> = (7, 63, 13, 80).into();
    assert_eq!(render_analytic(path, FillRule::NonZero), [
        solid,       half,        transparent, transparent,
        half,        transparent, transparent, transparent,
        transparent, transparent, transparent, transparent,
        transparent, transparent, transparent, transparent,
    ]);
}

#[test] fn linear_gradient_rgba_golden() {
    let mut path = PathBuilder::new();
    path.move_to((0.0, 0.0)).line_to((4.0, 0.0))
        .line_to((4.0, 4.0)).line_to((0.0, 4.0));
    let stops = [GradientStop::new(0.0, RGBA::red()),
                 GradientStop::new(1.0, RGBA::blue())];
    let gradient = LinearGradient::new((0.0, 0.0), (4.0, 0.0),
        GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
    let row: [PRGB32<u8>; 4] = [
        (223, 0, 32, 255).into(), (159, 0, 96, 255).into(),
        (96, 0, 159, 255).into(), (32, 0, 223, 255).into(),
    ];
    assert_eq!(render_analytic_paint(path, &gradient),
        core::array::from_fn(|index| row[index % row.len()]));
}

#[test] fn fractional_butt_stroke_rgba_golden() {
    let mut path = PathBuilder::new();
    path.move_to((0.5, 1.5)).line_to((3.5, 1.5));
    let transparent = PRGB32::zeroed();
    let solid: PRGB32<u8> = (13, 125, 25, 160).into();
    let half: PRGB32<u8> = (7, 63, 13, 80).into();
    assert_eq!(render_analytic_stroke(path), [
        transparent, transparent, transparent, transparent,
        half,        solid,       solid,       half,
        transparent, transparent, transparent, transparent,
        transparent, transparent, transparent, transparent,
    ]);
}

#[cfg(feature = "fixed")]
#[test] fn fixed_triangles_track_the_analytic_pipeline() {
    use ugl_rs::{canvas::render_solid_fixed, geometry::{FixedScalar, Point},
        raster_fixed::{FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid,
            prepare_lines},
    };

    fn fixed_edge(from: Point<FixedScalar>, to: Point<FixedScalar>) ->
        Option<Edge<FixedScalar>> {
        if from.y < to.y {
            Some(Edge { upper: from, lower: to, winding: 1 })
        } else if from.y > to.y {
            Some(Edge { upper: to, lower: from, winding: -1 })
        } else { None }
    }

    let scenes = [
        [(0.25, 0.25), (3.5, 0.75), (0.75, 3.5)],
        [(0.5, 3.5), (1.75, 0.125), (3.75, 3.25)],
        [(-0.5, 1.0), (2.0, -0.25), (4.25, 3.75)],
    ];
    for points in scenes {
        let mut path = PathBuilder::new();
        path.move_to(points[0]).line_to(points[1]).line_to(points[2]);
        let reference = render_analytic(path, FillRule::NonZero);

        let fixed_points = points.map(|(x, y)|
            (FixedScalar::from_num(x), FixedScalar::from_num(y)).into());
        let edges: Vec<_> = (0..3).filter_map(|index|
            fixed_edge(fixed_points[index], fixed_points[(index + 1) % 3])).collect();
        let (mut lines, mut segments, mut trapezoids, mut row_area) = (
            [FixedLine::default(); 3], [FixedSegment::default(); 3],
            [FixedTrapezoid::default(); 2], [0; WIDTH as usize],
        );
        let line_count = prepare_lines(&edges, &mut lines).unwrap();
        let requirements =
            ugl_rs::raster_fixed::fixed_strip_requirements(&lines[..line_count], HEIGHT).unwrap();
        let (mut strip_offsets, mut strip_indices) =
            (vec![0; requirements.offsets], vec![0; requirements.indices]);
        let mut bytes = [0; WIDTH as usize * HEIGHT as usize * 4];
        let mut target = PixmapMut::new(&mut bytes, WIDTH, HEIGHT, WIDTH * 4).unwrap();
        render_solid_fixed(&lines[..line_count], RGBA::new(20, 200, 40, 160),
            FillRule::NonZero, &mut target, &mut FixedRasterWorkspace {
                segments: &mut segments, trapezoids: &mut trapezoids,
                row_area: &mut row_area,
                strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
            },
        ).unwrap();

        for (index, reference) in reference.iter().enumerate() {
            let actual = target.pixel(index as u32 % WIDTH, index as u32 / WIDTH).unwrap();
            for (actual, reference) in actual.to_array().into_iter().zip(reference.to_array()) {
                assert!(actual.abs_diff(reference) <= 2,
                    "scene {points:?}, pixel {index}: fixed={actual}, analytic={reference}");
            }
        }
    }
}
