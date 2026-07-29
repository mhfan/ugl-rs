use ugl_rs::{
    analytic::AnalyticIntersection,
    canvas::{AnalyticRenderOptions, AnalyticRenderWorkspace, PixmapMut, render_solid_analytic},
    color::{PremulRGBA, RGBA},
    edge::Edge,
    geometry::{Affine, PathBuilder},
    raster::FillRule,
};

const WIDTH: u32 = 4;
const HEIGHT: u32 = 4;

fn render_analytic(builder: PathBuilder, fill_rule: FillRule) -> [PremulRGBA<u8>; 16] {
    let mut bytes = [0; WIDTH as usize * HEIGHT as usize * 4];
    let mut target = PixmapMut::new(&mut bytes, WIDTH, HEIGHT, WIDTH * 4).unwrap();
    let (mut edges, mut intersections, mut row_coverage) = (
        [Edge::default(); 8],
        [AnalyticIntersection::default(); 8],
        [0.0; WIDTH as usize],
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

#[test]
fn aligned_rectangle_rgba_golden() {
    let mut path = PathBuilder::new();
    path.move_to((1.0, 1.0)).line_to((3.0, 1.0))
        .line_to((3.0, 3.0)).line_to((1.0, 3.0));

    let transparent = PremulRGBA::zeroed();
    let solid: PremulRGBA<u8> = (13, 125, 25, 160).into();
    assert_eq!(render_analytic(path, FillRule::NonZero), [
        transparent, transparent, transparent, transparent,
        transparent, solid,       solid,       transparent,
        transparent, solid,       solid,       transparent,
        transparent, transparent, transparent, transparent,
    ]);
}

#[test]
fn diagonal_triangle_rgba_golden() {
    let mut path = PathBuilder::new();
    path.move_to((0.0, 0.0)).line_to((2.0, 0.0)).line_to((0.0, 2.0));

    let transparent = PremulRGBA::zeroed();
    let solid: PremulRGBA<u8> = (13, 125, 25, 160).into();
    let half: PremulRGBA<u8> = (7, 63, 13, 80).into();
    assert_eq!(render_analytic(path, FillRule::NonZero), [
        solid,       half,        transparent, transparent,
        half,        transparent, transparent, transparent,
        transparent, transparent, transparent, transparent,
        transparent, transparent, transparent, transparent,
    ]);
}

#[cfg(feature = "fixed")]
#[test]
fn fixed_triangles_track_the_analytic_pipeline() {
    use ugl_rs::{
        canvas::render_solid_fixed,
        geometry::{FixedScalar, Point},
        raster_fixed::{
            FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid, prepare_lines,
        },
    };

    fn fixed_edge(from: Point<FixedScalar>, to: Point<FixedScalar>) ->
        Option<Edge<FixedScalar>> {
        if from.y < to.y {
            Some(Edge { upper: from, lower: to, winding: 1 })
        } else if from.y > to.y {
            Some(Edge { upper: to, lower: from, winding: -1 })
        } else {
            None
        }
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
        let mut bytes = [0; WIDTH as usize * HEIGHT as usize * 4];
        let mut target = PixmapMut::new(&mut bytes, WIDTH, HEIGHT, WIDTH * 4).unwrap();
        render_solid_fixed(&lines[..line_count], RGBA::new(20, 200, 40, 160),
            FillRule::NonZero, &mut target, &mut FixedRasterWorkspace {
                segments: &mut segments, trapezoids: &mut trapezoids,
                row_area: &mut row_area,
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
