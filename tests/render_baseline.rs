
use ugl_rs::{analytic::{Cell as AnalyticCell, Intersection as AnalyticIntersection},
    color::{PremulSRGBA8, LinearPremulRGBA, PremulRGBA, SRGBA, SRGBA as RGBA},
    canvas::{RenderOptions, RenderWorkspace, StrokePathOptions,
        StrokeWorkspace, Pixmap, render_paint as render_canvas_paint, render_solid,
        render_stroke_solid,
    }, canvas_linear::{LinearPixmap, render_paint as render_paint_linear,
        render_solid as render_solid_linear},
    edge::Edge, geometry::{Affine, PathBuilder}, raster::FillRule,
    stroke::{LineCap, LineJoin, StrokeContour, StrokeOptions},
    sampler::{ConicGradient, GradientStop, GradientStops, LinearGradient, LinearPaintSampler,
        PaintSampler, RadialGradient, SpreadMode, TransformedPaint},
};

const  WIDTH: u32 = 4;
const HEIGHT: u32 = 4;

fn legacy_pixel(pixel: PremulSRGBA8) -> PremulRGBA<u8> {
    let [r, g, b, a] = pixel.to_array();
    (r, g, b, a).into()
}

fn render(builder: PathBuilder, fill_rule: FillRule) -> [PremulRGBA<u8>; 16] {
    let mut bytes = [0; WIDTH as usize * HEIGHT as usize * 4];
    let mut target = Pixmap::from_buffer(&mut bytes, WIDTH, HEIGHT, WIDTH * 4).unwrap();
    let (mut edges, mut intersections, mut row_coverage) = (
        [Edge::default(); 8], [AnalyticIntersection::default(); 8],
        [AnalyticCell::default(); WIDTH as usize],
    );
    let (mut row_offsets, mut edge_indices) = ([0; HEIGHT as usize + 1], [0; 8]);
    render_solid(&builder.build(), Affine::identity(), RGBA::new(20, 200, 40, 160),
        RenderOptions { fill_rule, ..Default::default() }, &mut target,
        &mut RenderWorkspace {
            edges: &mut edges, intersections: &mut intersections,
            cells: &mut row_coverage,
            row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
        },
    ).unwrap();
    core::array::from_fn(|index| legacy_pixel(
        target.pixel(index as u32 % WIDTH, index as u32 / WIDTH).unwrap()))
}

fn render_paint(builder: PathBuilder, sampler: &impl PaintSampler) ->
    [PremulRGBA<u8>; 16] {
    let mut bytes = [0; WIDTH as usize * HEIGHT as usize * 4];
    let mut target = Pixmap::from_buffer(&mut bytes, WIDTH, HEIGHT, WIDTH * 4).unwrap();
    let (mut edges, mut intersections, mut row_coverage) = (
        [Edge::default(); 8], [AnalyticIntersection::default(); 8],
        [AnalyticCell::default(); WIDTH as usize],
    );
    let (mut row_offsets, mut edge_indices) = ([0; HEIGHT as usize + 1], [0; 8]);
    render_canvas_paint(&builder.build(), Affine::identity(), sampler,
        RenderOptions::default(), &mut target, &mut RenderWorkspace {
            edges: &mut edges, intersections: &mut intersections,
            cells: &mut row_coverage,
            row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
        },
    ).unwrap();
    core::array::from_fn(|index|
        legacy_pixel(target.pixel(index as u32 % WIDTH, index as u32 / WIDTH).unwrap()))
}

fn render_stroke_with(builder: PathBuilder, stroke: StrokeOptions) ->
    [PremulRGBA<u8>; 16] {
    let mut bytes = [0; WIDTH as usize * HEIGHT as usize * 4];
    let mut target = Pixmap::from_buffer(&mut bytes, WIDTH, HEIGHT, WIDTH * 4).unwrap();
    let (mut contours, mut edges) = ([StrokeContour::default(); 2], [Edge::default(); 64]);
    let (mut points, mut row_coverage) =
        ([Default::default(); 8], [AnalyticCell::default(); WIDTH as usize]);
    let mut intersections = [AnalyticIntersection::default(); 64];
    let (mut row_offsets, mut edge_indices) = ([0; HEIGHT as usize + 1], [0; 64]);
    render_stroke_solid(&builder.build(), Affine::identity(),
        RGBA::new(20, 200, 40, 160),
        StrokePathOptions { stroke, ..Default::default() }, &mut target,
        &mut StrokeWorkspace {
            points: &mut points, contours: &mut contours, edges: &mut edges,
            intersections: &mut intersections, cells: &mut row_coverage,
            row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
        }).unwrap();
    core::array::from_fn(|index|
        legacy_pixel(target.pixel(index as u32 % WIDTH, index as u32 / WIDTH).unwrap()))
}

fn render_stroke(builder: PathBuilder) -> [PremulRGBA<u8>; 16] {
    render_stroke_with(builder, StrokeOptions::default())
}

fn render_linear_layers(builder: PathBuilder, colors: &[SRGBA<u8>]) ->
    [PremulRGBA<u8>; 16] {
    let path = builder.build();
    let mut linear = [LinearPremulRGBA::default(); 16];
    let mut target = LinearPixmap::from_buffer(&mut linear, WIDTH, HEIGHT, WIDTH).unwrap();
    let (mut edges, mut intersections, mut row_coverage) = (
        [Edge::default(); 8], [AnalyticIntersection::default(); 8],
        [AnalyticCell::default(); WIDTH as usize],
    );
    let (mut row_offsets, mut edge_indices) = ([0; HEIGHT as usize + 1], [0; 8]);
    for color in colors {
        render_solid_linear(&path, Affine::identity(), *color,
            RenderOptions::default(), &mut target, &mut RenderWorkspace {
                edges: &mut edges, intersections: &mut intersections,
                cells: &mut row_coverage,
                row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
            }).unwrap();
    }
    let mut bytes = [0; 16 * 4];
    let mut encoded = Pixmap::from_buffer(&mut bytes, WIDTH, HEIGHT, WIDTH * 4).unwrap();
    target.encode_into(&mut encoded).unwrap();
    core::array::from_fn(|index|
        legacy_pixel(encoded.pixel(
            index as u32 % WIDTH, index as u32 / WIDTH).unwrap()))
}

fn render_linear_paint(builder: PathBuilder, sampler: &impl LinearPaintSampler) ->
    [PremulRGBA<u8>; 16] {
    let mut linear = [LinearPremulRGBA::default(); 16];
    let mut target = LinearPixmap::from_buffer(&mut linear, WIDTH, HEIGHT, WIDTH).unwrap();
    let (mut edges, mut intersections, mut row_coverage) = (
        [Edge::default(); 8], [AnalyticIntersection::default(); 8],
        [AnalyticCell::default(); WIDTH as usize],
    );
    let (mut row_offsets, mut edge_indices) = ([0; HEIGHT as usize + 1], [0; 8]);
    render_paint_linear(&builder.build(), Affine::identity(), sampler,
        RenderOptions::default(), &mut target, &mut RenderWorkspace {
            edges: &mut edges, intersections: &mut intersections,
            cells: &mut row_coverage,
            row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
        }).unwrap();
    let mut bytes = [0; 16 * 4];
    let mut encoded = Pixmap::from_buffer(&mut bytes, WIDTH, HEIGHT, WIDTH * 4).unwrap();
    target.encode_into(&mut encoded).unwrap();
    core::array::from_fn(|index|
        legacy_pixel(encoded.pixel(
            index as u32 % WIDTH, index as u32 / WIDTH).unwrap()))
}

#[test] fn aligned_rectangle_rgba_golden() {
    let mut path = PathBuilder::new();
    path.move_to((1.0, 1.0)).line_to((3.0, 1.0))
        .line_to((3.0, 3.0)).line_to((1.0, 3.0));

    let transparent = PremulRGBA::zeroed();
    let solid: PremulRGBA<u8> = (13, 125, 25, 160).into();
    assert_eq!(render(path, FillRule::NonZero), [
        transparent, transparent, transparent, transparent,
        transparent, solid,       solid,       transparent,
        transparent, solid,       solid,       transparent,
        transparent, transparent, transparent, transparent,
    ]);
}

#[test] fn diagonal_triangle_rgba_golden() {
    let mut path = PathBuilder::new();
    path.move_to((0.0, 0.0)).line_to((2.0, 0.0)).line_to((0.0, 2.0));

    let transparent = PremulRGBA::zeroed();
    let solid: PremulRGBA<u8> = (13, 125, 25, 160).into();
    let half: PremulRGBA<u8> = (7, 63, 13, 80).into();
    assert_eq!(render(path, FillRule::NonZero), [
        solid,       half,        transparent, transparent,
        half,        transparent, transparent, transparent,
        transparent, transparent, transparent, transparent,
        transparent, transparent, transparent, transparent,
    ]);
}

#[test] fn linear_source_over_and_fractional_coverage_rgba_golden() {
    let mut rectangle = PathBuilder::new();
    rectangle.move_to((0.0, 0.0)).line_to((4.0, 0.0))
        .line_to((4.0, 4.0)).line_to((0.0, 4.0));
    assert_eq!(render_linear_layers(rectangle,
        &[SRGBA::blue(), SRGBA::new(255, 0, 0, 128)]), [(188, 0, 187, 255).into(); 16]);

    let mut triangle = PathBuilder::new();
    triangle.move_to((0.0, 0.0)).line_to((2.0, 0.0)).line_to((0.0, 2.0));
    let (transparent, solid, half): (PremulRGBA<u8>, PremulRGBA<u8>, PremulRGBA<u8>) =
        (PremulRGBA::zeroed(), (13, 125, 25, 160).into(), (6, 63, 13, 80).into());
    assert_eq!(render_linear_layers(triangle, &[SRGBA::new(20, 200, 40, 160)]), [
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
    let row: [PremulRGBA<u8>; 4] = [
        (240, 0, 99, 255).into(), (207, 0, 165, 255).into(),
        (165, 0, 207, 255).into(), (99, 0, 240, 255).into(),
    ];
    assert_eq!(render_paint(path, &gradient),
        core::array::from_fn(|index| row[index % row.len()]));
}

#[test] fn linear_gradient_family_preserves_the_exact_presentation_boundary() {
    fn rectangle() -> PathBuilder {
        let mut path = PathBuilder::new();
        path.move_to((0.0, 0.0)).line_to((4.0, 0.0))
            .line_to((4.0, 4.0)).line_to((0.0, 4.0));
        path
    }
    fn assert_boundary<S: PaintSampler + LinearPaintSampler>(sampler: &S) {
        assert_eq!(render_linear_paint(rectangle(), sampler),
            render_paint(rectangle(), sampler));
    }

    let stops = [GradientStop::new(0.0, SRGBA::new(240, 20, 80, 32)),
                 GradientStop::new(0.4, SRGBA::new(10, 220, 40, 160)),
                 GradientStop::new(1.0, SRGBA::new(30, 60, 250, 224))];
    let stops = GradientStops::new(&stops).unwrap();
    let linear = LinearGradient::new((0.0, 0.0), (4.0, 3.0),
        stops, SpreadMode::Reflect).unwrap();
    let radial = RadialGradient::two_circle((0.5, 0.5), 0.25,
        (2.0, 2.0), 3.0, stops, SpreadMode::Repeat).unwrap();
    let conic = ConicGradient::new((2.0, 2.0), 0.37, stops).unwrap();
    let transformed = TransformedPaint::new(conic,
        Affine::new(1.25, 0.2, -0.3, 1.5, 0.5, -0.75)).unwrap();
    assert_boundary(&linear);
    assert_boundary(&radial);
    assert_boundary(&conic);
    assert_boundary(&transformed);
}

#[test] fn fractional_butt_stroke_rgba_golden() {
    let mut path = PathBuilder::new();
    path.move_to((0.5, 1.5)).line_to((3.5, 1.5));
    let transparent = PremulRGBA::zeroed();
    let solid: PremulRGBA<u8> = (13, 125, 25, 160).into();
    let half: PremulRGBA<u8> = (7, 63, 13, 80).into();
    assert_eq!(render_stroke(path), [
        transparent, transparent, transparent, transparent,
        half,        solid,       solid,       half,
        transparent, transparent, transparent, transparent,
        transparent, transparent, transparent, transparent,
    ]);
}

#[test] fn stroke_caps_and_joins_rgba_golden() {
    let mut line = PathBuilder::new();
    line.move_to((1.5, 1.5)).line_to((2.5, 1.5));
    let alpha = |builder, options| render_stroke_with(builder, options)
        .map(|pixel| pixel.alpha());
    let options = StrokeOptions::new(2.0).unwrap();
    assert_eq!(alpha(line.clone(), options.with_cap(LineCap::Butt)), [
         0, 40, 40,  0,   0, 80, 80,  0,   0, 40, 40,  0,   0,  0,  0, 0,
    ]);
    assert_eq!(alpha(line.clone(), options.with_cap(LineCap::Square)), [
        40, 80, 80, 40,  80,160,160, 80,  40, 80, 80, 40,   0,  0,  0, 0,
    ]);
    assert_eq!(alpha(line, options.with_cap(LineCap::Round)), [
         6, 68, 68,  6,  58,160,160, 58,   6, 68, 68,  6,   0,  0,  0, 0,
    ]);

    let mut corner = PathBuilder::new();
    corner.move_to((0.5, 3.5)).line_to((2.0, 0.5)).line_to((3.5, 3.5));
    assert_eq!(alpha(corner.clone(), options.with_join(LineJoin::Bevel)), [
         22,150,150, 22,  99,160,160, 99, 157,157,157,157,  80, 90, 90, 80,
    ]);
    let rounded = alpha(corner.clone(), options.with_join(LineJoin::Round));
    assert_eq!(rounded, [
         22,157,157, 22,  99,160,160, 99, 157,157,157,157,  80, 90, 90, 80,
    ]);
    assert_eq!(alpha(corner, options.with_join(LineJoin::Miter)), rounded);
}

#[cfg(feature = "fixed")]
#[test] fn fixed_triangles_track_the_pipeline() {
    use ugl_rs::{fixed::{canvas::render_solid, raster::{
            Line, Workspace, Segment, Trapezoid, prepare_lines, strip_requirements,
        }}, fixed::Scalar, geometry::Point,
    };

    fn fixed_edge(from: Point<Scalar>, to: Point<Scalar>) ->
        Option<Edge<Scalar>> {
        if from.y < to.y {
            Some(Edge { upper: from, lower: to, winding: 1 })
        } else if from.y > to.y {
            Some(Edge { upper: to, lower: from, winding: -1 })
        } else { None }
    }

    let scenes = [[(0.25, 0.25), (3.5, 0.75), (0.75, 3.5)],
                  [(0.5, 3.5), (1.75, 0.125), (3.75, 3.25)],
                  [(-0.5, 1.0), (2.0, -0.25), (4.25, 3.75)]];
    for points in scenes {
        let mut path = PathBuilder::new();
        path.move_to(points[0]).line_to(points[1]).line_to(points[2]);
        let reference = render(path, FillRule::NonZero);

        let fixed_points = points.map(|(x, y)|
            (Scalar::from_num(x), Scalar::from_num(y)).into());
        let edges: Vec<_> = (0..3).filter_map(|index|
            fixed_edge(fixed_points[index], fixed_points[(index + 1) % 3])).collect();
        let (mut lines, mut segments, mut trapezoids, mut row_area) = (
            [Line::default(); 3], [Segment::default(); 3],
            [Trapezoid::default(); 2], [0; WIDTH as usize],
        );
        let line_count = prepare_lines(&edges, &mut lines).unwrap();
        let requirements = strip_requirements(&lines[..line_count], HEIGHT).unwrap();
        let mut strip_offsets = vec![0; requirements.offsets];
        let mut strip_indices = vec![0; requirements.indices];
        let mut bytes = [0; WIDTH as usize * HEIGHT as usize * 4];
        let mut target = Pixmap::from_buffer(&mut bytes, WIDTH, HEIGHT, WIDTH * 4).unwrap();
        render_solid(&lines[..line_count], RGBA::new(20, 200, 40, 160),
            FillRule::NonZero, &mut target, &mut Workspace {
                segments: &mut segments, trapezoids: &mut trapezoids,
                row_area: &mut row_area, strip_offsets: &mut strip_offsets,
                strip_indices: &mut strip_indices,
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
