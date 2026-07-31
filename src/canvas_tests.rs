
use super::*;
use crate::{analytic::AnalyticIntersection,
    color::{PremulSRGBA8, RGBA as GenericRGBA, SRGBA as RGBA},
    edge::Edge,
    geometry::{Affine, PathBuilder}, raster::Intersection,
    sampler::{GradientStop, GradientStops, LinearGradient, RadialGradient, SpreadMode},
    stroke::{LineCap, LineJoin},
};
use alloc::vec;

fn rectangle(left: f32, top: f32, right: f32, bottom: f32) -> Path {
    let mut builder = PathBuilder::new();
    builder.move_to((left, top)).line_to((right, top))
           .line_to((right, bottom)).line_to((left, bottom));
    builder.build()
}
fn red_blue_stops() -> [GradientStop; 2] {
    [GradientStop::new(0.0, RGBA::red()), GradientStop::new(1.0, RGBA::blue())]
}

struct AnalyticBuffers<const EDGES: usize, const WIDTH: usize> {
    intersections: [AnalyticIntersection; EDGES],
    edges: [Edge; EDGES], row_coverage: [f32; WIDTH],
    row_offsets: [u32; 9], edge_indices: [u32; EDGES],
}

impl<const EDGES: usize, const WIDTH: usize> AnalyticBuffers<EDGES, WIDTH> {
    fn new() -> Self { Self {
            intersections: [AnalyticIntersection::default(); EDGES],
            edges: [Edge::default(); EDGES], row_coverage: [0.0; WIDTH],
            row_offsets: [0; 9], edge_indices: [0; EDGES],
    } }

    fn workspace(&mut self) -> AnalyticRenderWorkspace<'_> {
        AnalyticRenderWorkspace { edges: &mut self.edges,
            intersections: &mut self.intersections,
             row_coverage: &mut self.row_coverage,
            row_offsets: &mut self.row_offsets, edge_indices: &mut self.edge_indices,
        }
    }
}

#[test] fn pixmap_validates_stride_and_preserves_padding() {
    let mut data = [0_u8; 11];
    assert_eq!(PixmapMut::new(&mut data, 2, 1, 7).unwrap_err(),
        PixmapError::StrideTooSmall { minimum: 8, actual: 7 });
    let mut target = PixmapMut::new(&mut data, 2, 1, 11).unwrap();
    target.blend_solid_span(0, 0, 2, GenericRGBA::<u8>::red().premul(), 255);
    assert_eq!(target.pixel_bytes(1, 0), Some([255, 0, 0, 255]));
    assert_eq!(&target.data[8..], &[0, 0, 0]);
}

#[test] fn pixmap_distinguishes_raw_bytes_from_valid_encoded_premul_pixels() {
    let mut data = [200, 20, 10, 100];
    let target = PixmapMut::new(&mut data, 1, 1, 4).unwrap();
    assert_eq!(target.pixel_bytes(0, 0), Some([200, 20, 10, 100]));
    assert_eq!(target.pixel(0, 0), None);
    assert_eq!(target.pixel_bytes(1, 0), None);
}

#[test] fn source_over_combines_coverage_alpha_and_premultiplied_destination() {
    let mut data = [0, 0, 255, 255];
    let mut target = PixmapMut::new(&mut data, 1, 1, 4).unwrap();
    target.blend_solid_span(
        0, 0, 1, GenericRGBA::<u8>::new(255, 0, 0, 128).premul(), 255);
    assert_eq!(target.pixel_bytes(0, 0), Some([128, 0, 127, 255]));
    let before = target.pixel_bytes(0, 0);
    target.blend_solid_span(0, 0, 1,
        GenericRGBA::<u8>::new(1, 2, 3, 0).premul(), 255);
    assert_eq!(target.pixel_bytes(0, 0), before);
}

#[test] fn solid_rectangle_renders_end_to_end_without_allocation() {
    let path = rectangle(1.0, 1.0, 3.0, 3.0);
    let mut pixels = vec![0; 4 * 4 * 4];
    let mut target = PixmapMut::new(&mut pixels, 4, 4, 16).unwrap();
    let (mut edges, mut intersections, mut row_coverage) =
        ([Edge::default(); 4], [Intersection::default(); 4], [0.0; 4]);
    render_solid(&path, Affine::identity(), RGBA::new(255, 0, 0, 128),
        RenderOptions::default(), &mut target,
        &mut RenderWorkspace { edges: &mut edges,
            intersections: &mut intersections,
             row_coverage: &mut row_coverage,
        },
    ).unwrap();
    assert_eq!(target.pixel_bytes(0, 0), Some([0; 4]));
    assert_eq!(target.pixel_bytes(1, 1), Some([128, 0, 0, 128]));
    assert_eq!(target.pixel_bytes(2, 2), Some([128, 0, 0, 128]));
    assert_eq!(target.pixel_bytes(3, 3), Some([0; 4]));
}

#[test] fn edge_capacity_failure_reports_required_lower_bound() {
    let (mut builder, mut pixels) = (PathBuilder::new(), [0; 16]);
    builder.move_to((0.0, 0.0)).line_to((1.0, 1.0)).line_to((2.0, 0.0));
    let mut target = PixmapMut::new(&mut pixels, 2, 2, 8).unwrap();
    let (mut edges, mut intersections, mut row_coverage) =
        ([Edge::default(); 1], [Intersection::default(); 2], [0.0; 2]);
    let result = render_solid(&builder.build(), Affine::identity(), RGBA::white(),
        RenderOptions::default(), &mut target,
        &mut RenderWorkspace { edges: &mut edges,
            intersections: &mut intersections,
             row_coverage: &mut row_coverage,
        },
    );
    assert_eq!(result, Err(RenderError::EdgeCapacity { needed_at_least: 2 }));
}

#[test] fn analytic_solid_rendering_uses_the_shared_compositor() {
    let (mut builder, mut pixels) = (PathBuilder::new(), [0; 4]);
    builder.move_to((0.0, 0.0)).line_to((1.0, 0.0)).line_to((0.0, 1.0));
    let mut target = PixmapMut::new(&mut pixels, 1, 1, 4).unwrap();
    let mut buffers = AnalyticBuffers::<2, 1>::new();
    render_solid_analytic(&builder.build(), Affine::identity(), RGBA::white(),
        AnalyticRenderOptions::default(), &mut target, &mut buffers.workspace()).unwrap();
    assert_eq!(target.pixel_bytes(0, 0), Some([128; 4]));
}

#[test] fn analytic_sampled_paint_uses_device_pixel_centers_and_coverage() {
    struct CoordinatePaint;
    impl PaintSampler for CoordinatePaint {
        fn sample(&self, x: f32, y: f32) -> PremulSRGBA8 {
            PremulSRGBA8::new((x * 40.0) as _, (y * 40.0) as _, 0, u8::MAX).unwrap()
        }
    }

    let (path, mut pixels) = (rectangle(0.5, 0.0, 2.0, 1.0), [0; 8]);
    let mut target = PixmapMut::new(&mut pixels, 2, 1, 8).unwrap();
    let mut buffers = AnalyticBuffers::<4, 2>::new();
    render_paint_analytic(&path, Affine::identity(), &CoordinatePaint,
        AnalyticRenderOptions::default(), &mut target, &mut buffers.workspace()).unwrap();
    assert_eq!(target.pixel_bytes(0, 0), Some([10, 10, 0, 128]));
    assert_eq!(target.pixel_bytes(1, 0), Some([60, 20, 0, 255]));
}

#[test] fn analytic_stroke_runs_path_expansion_and_composition_without_allocation() {
    let mut builder = PathBuilder::new();
    builder.move_to((0.5, 0.5)).line_to((2.5, 0.5));
    let mut contours = [StrokeContour::default(); 1];
    let mut intersections = [AnalyticIntersection::default(); 4];
    let (mut row_coverage, mut pixels) = ([0.0; 3], [0; 12]);
    let (mut points, mut edges) = ([Point::default(); 2], [Edge::default(); 4]);
    render_stroke_solid_analytic(&builder.build(), Affine::identity(), RGBA::white(),
        AnalyticStrokeOptions::default(), &mut PixmapMut::new(&mut pixels, 3, 1, 12).unwrap(),
        &mut AnalyticStrokeWorkspace {
            points: &mut points, contours: &mut contours, edges: &mut edges,
            intersections: &mut intersections, row_coverage: &mut row_coverage,
            row_offsets: &mut [0; 2], edge_indices: &mut [0; 4],
        }).unwrap();
    assert_eq!(pixels, [128, 128, 128, 128, 255, 255, 255, 255, 128, 128, 128, 128]);
}

#[test] fn analytic_dashed_stroke_renders_alternating_on_intervals() {
    use crate::dash::{DashContour, DashPattern};

    let mut builder = PathBuilder::new();
    builder.move_to((0.5, 0.5)).line_to((4.5, 0.5));
    let mut pixels = [0; 20];
    let (mut points, mut dash_points) = ([Point::default(); 2], [Point::default(); 8]);
    let (mut contours, mut dash_contours) =
        ([StrokeContour::default(); 1], [DashContour::default(); 4]);
    let (mut edges, mut intersections, mut row_coverage) =
        ([Edge::default(); 8], [AnalyticIntersection::default(); 8], [0.0; 5]);
    render_stroke_paint_analytic_dashed(&builder.build(), Affine::identity(),
        &SolidPaint::new(RGBA::white()), AnalyticDashedStrokeOptions {
            flatten: FlattenOptions::default(), stroke: StrokeOptions::default(),
            dash: DashPattern::new(&[1.0, 1.0], 0.0).unwrap(),
        }, &mut PixmapMut::new(&mut pixels, 5, 1, 20).unwrap(),
        &mut AnalyticDashedStrokeWorkspace {
            stroke: AnalyticStrokeWorkspace {
                points: &mut points, contours: &mut contours, edges: &mut edges,
                intersections: &mut intersections, row_coverage: &mut row_coverage,
                row_offsets: &mut [0; 2], edge_indices: &mut [0; 8],
            },
            dash_points: &mut dash_points, dash_contours: &mut dash_contours,
        }).unwrap();
    let alpha: alloc::vec::Vec<_> = pixels.chunks_exact(4).map(|pixel| pixel[3]).collect();
    assert_eq!(alpha, [128, 128, 128, 128, 0]);
}

#[test] fn analytic_stroke_capacity_errors_leave_the_target_unchanged() {
    let mut builder = PathBuilder::new();
    builder.move_to((0.5, 0.5)).line_to((2.5, 0.5));
    let mut contours = [StrokeContour::default(); 1];
    let mut intersections = [AnalyticIntersection::default(); 4];
    let (mut row_coverage, mut pixels) = ([0.0; 3], [17; 12]);
    let mut points = [Point::default(); 1];
    let error = render_stroke_solid_analytic(&builder.build(), Affine::identity(),
        RGBA::white(), AnalyticStrokeOptions::default(),
        &mut PixmapMut::new(&mut pixels, 3, 1, 12).unwrap(),
        &mut AnalyticStrokeWorkspace {
            points: &mut points, contours: &mut contours, edges: &mut [],
            intersections: &mut intersections, row_coverage: &mut row_coverage,
            row_offsets: &mut [0; 2], edge_indices: &mut [],
        },
    );
    assert_eq!(error, Err(RenderError::StrokePointCapacity { needed_at_least: 2 }));
    assert_eq!(pixels, [17; 12]);
}

#[test] fn analytic_stroke_geometry_capacity_errors_leave_the_target_unchanged() {
    let mut builder = PathBuilder::new();
    builder.move_to((0.5, 0.5)).line_to((2.5, 0.5));
    let path = builder.build();
    let (mut points, mut intersections, mut row_coverage) = (
        [Point::default(); 2], [AnalyticIntersection::default(); 4], [0.0; 3],
    );
    let mut pixels = [17; 12];
    let error = render_stroke_solid_analytic(&path, Affine::identity(), RGBA::white(),
        AnalyticStrokeOptions::default(), &mut PixmapMut::new(&mut pixels, 3, 1, 12).unwrap(),
        &mut AnalyticStrokeWorkspace {
            points: &mut points, contours: &mut [], edges: &mut [],
            intersections: &mut intersections, row_coverage: &mut row_coverage,
            row_offsets: &mut [0; 2], edge_indices: &mut [],
        });
    assert_eq!(error, Err(RenderError::StrokeContourCapacity { needed_at_least: 1 }));
    assert_eq!(pixels, [17; 12]);

    let mut contours = [StrokeContour::default(); 1];
    let mut edges = [Edge::default(); 1];
    let error = render_stroke_solid_analytic(&path, Affine::identity(), RGBA::white(),
        AnalyticStrokeOptions::default(), &mut PixmapMut::new(&mut pixels, 3, 1, 12).unwrap(),
        &mut AnalyticStrokeWorkspace {
            points: &mut points, contours: &mut contours, edges: &mut edges,
            intersections: &mut intersections, row_coverage: &mut row_coverage,
            row_offsets: &mut [0; 2], edge_indices: &mut [0; 1],
        });
    assert_eq!(error, Err(RenderError::EdgeCapacity { needed_at_least: 2 }));
    assert_eq!(pixels, [17; 12]);
}

#[test] fn randomized_strokes_render_valid_premultiplied_pixels() {
    let mut seed = 0xC0FF_EE11_u32;
    let random = |seed: &mut u32| {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((*seed >> 8) as f32 / 0x00FF_FFFF as f32) * 12.0 - 2.0
    };
    for case in 0..128 {
        let mut builder = PathBuilder::new();
        let len = case % 8 + 1;
        let mut previous = (random(&mut seed), random(&mut seed));
        builder.move_to(previous);
        for index in 1..len {
            let point = if (case + index) % 5 == 0 {
                previous
            } else { (random(&mut seed), random(&mut seed)) };
            builder.line_to(point);
            previous = point;
        }
        if case & 1 != 0 { builder.close(); }

        let cap = [LineCap::Butt, LineCap::Round, LineCap::Square][case % 3];
        let join = [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel][case / 3 % 3];
        let stroke = StrokeOptions::new(0.25 + (case % 12) as f32 * 0.25).unwrap()
            .with_cap(cap).with_join(join);
        let mut pixels = [0; 8 * 8 * 4];
        let mut points = [Point::default(); 9];
        let mut contours = [StrokeContour::default(); 1];
        let mut edges = [Edge::default(); 512];
        let mut intersections = [AnalyticIntersection::default(); 512];
        let mut row_coverage = [0.0; 8];
        let mut target = PixmapMut::new(&mut pixels, 8, 8, 32).unwrap();
        render_stroke_solid_analytic(&builder.build(), Affine::identity(),
            RGBA::new(37, 149, 211, 173),
            AnalyticStrokeOptions { stroke, ..Default::default() }, &mut target,
            &mut AnalyticStrokeWorkspace {
                points: &mut points, contours: &mut contours, edges: &mut edges,
                intersections: &mut intersections, row_coverage: &mut row_coverage,
                row_offsets: &mut [0; 9], edge_indices: &mut [0; 512],
            }).unwrap();
        for y in 0..8 {
            for x in 0..8 {
                let [red, green, blue, alpha] = target.pixel_bytes(x, y).unwrap();
                assert!(red <= alpha && green <= alpha && blue <= alpha,
                    "case {case}, pixel ({x}, {y}) is not premultiplied");
            }
        }
    }
}

#[test] fn analytic_gradient_stroke_composes_through_rectangle_and_path_clips() {
    let mut builder = PathBuilder::new();
    builder.move_to((0.0, 0.5)).line_to((2.0, 0.5));
    let (path, stops) = (builder.build(), red_blue_stops());
    let gradient = LinearGradient::new((0.0, 0.0), (2.0, 0.0),
        GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
    let (mut points, mut edges) = ([Point::default(); 2], [Edge::default(); 4]);
    let (mut row_coverage, mut contours) = ([0.0; 2], [StrokeContour::default(); 1]);
    let mut intersections = [AnalyticIntersection::default(); 4];
    let (mut row_offsets, mut edge_indices) = ([0; 2], [0; 4]);
    let mut workspace = AnalyticStrokeWorkspace {
        points: &mut points, contours: &mut contours, edges: &mut edges,
        intersections: &mut intersections, row_coverage: &mut row_coverage,
        row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
    };

    let (mask_data, mut masked, mut clipped) = ([128, 255], [0; 8], [0; 8]);
    render_stroke_paint_analytic_clipped(&path, Affine::identity(), &gradient,
        Rect::from_ltrb(0.5, 0.0, 1.5, 1.0).unwrap(),
        AnalyticStrokeOptions::default(),
        &mut PixmapMut::new(&mut clipped, 2, 1, 8).unwrap(),
        &mut workspace).unwrap();
    assert_eq!(clipped, [113, 0, 69, 128, 69, 0, 113, 128]);

    render_stroke_paint_analytic_masked(&path, Affine::identity(), &gradient,
        CoverageMask::new(&mask_data, 2, 1, 2).unwrap(),
        AnalyticStrokeOptions::default(),
        &mut PixmapMut::new(&mut masked, 2, 1, 8).unwrap(),
        &mut workspace).unwrap();
    assert_eq!(masked, [113, 0, 69, 128, 137, 0, 225, 255]);
}

#[test] fn analytic_linear_gradient_renders_end_to_end() {
    let (path, mut pixels) = (rectangle(0.0, 0.0, 2.0, 1.0), [0; 8]);
    let stops = red_blue_stops();
    let gradient = LinearGradient::new((0.0, 0.0), (2.0, 0.0),
        GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
    let mut target = PixmapMut::new(&mut pixels, 2, 1, 8).unwrap();
    let mut buffers = AnalyticBuffers::<4, 2>::new();
    render_paint_analytic(&path, Affine::identity(), &gradient,
        AnalyticRenderOptions::default(), &mut target,
        &mut buffers.workspace()).unwrap();
    assert_eq!(target.pixel_bytes(0, 0), Some([225, 0, 137, 255]));
    assert_eq!(target.pixel_bytes(1, 0), Some([137, 0, 225, 255]));
}

#[test] fn analytic_radial_gradient_renders_end_to_end() {
    let (path, mut pixels) = (rectangle(0.0, 0.0, 3.0, 1.0), [0; 12]);
    let stops = red_blue_stops();
    let gradient = RadialGradient::new((1.5, 0.5), 1.5,
        GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
    let mut target = PixmapMut::new(&mut pixels, 3, 1, 12).unwrap();
    let mut buffers = AnalyticBuffers::<4, 3>::new();
    render_paint_analytic(&path, Affine::identity(), &gradient,
        AnalyticRenderOptions::default(), &mut target,
        &mut buffers.workspace()).unwrap();
    assert_eq!(target.pixel_bytes(0, 0), Some([156, 0, 213, 255]));
    assert_eq!(target.pixel_bytes(1, 0), Some([255, 0, 0, 255]));
    assert_eq!(target.pixel_bytes(2, 0), target.pixel_bytes(0, 0));
}

#[test] fn analytic_gradient_composes_through_rectangle_and_path_clips() {
    let path = rectangle(0.0, 0.0, 2.0, 1.0);
    let stops = red_blue_stops();
    let gradient = LinearGradient::new((0.0, 0.0), (2.0, 0.0),
        GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
    let mut buffers = AnalyticBuffers::<4, 2>::new();
    let mut workspace = buffers.workspace();

    let mut clipped_pixels = [0; 8];
    render_paint_analytic_clipped(&path, Affine::identity(), &gradient,
        Rect::from_ltrb(0.5, 0.0, 1.5, 1.0).unwrap(),
        AnalyticRenderOptions::default(),
        &mut PixmapMut::new(&mut clipped_pixels, 2, 1, 8).unwrap(),
        &mut workspace).unwrap();
    assert_eq!(clipped_pixels, [113, 0, 69, 128, 69, 0, 113, 128]);

    let (mask_data, mut masked_pixels) = ([128, 255], [0; 8]);
    render_paint_analytic_masked(&path, Affine::identity(), &gradient,
        CoverageMask::new(&mask_data, 2, 1, 2).unwrap(),
        AnalyticRenderOptions::default(),
        &mut PixmapMut::new(&mut masked_pixels, 2, 1, 8).unwrap(),
        &mut workspace).unwrap();
    assert_eq!(masked_pixels, [113, 0, 69, 128, 137, 0, 225, 255]);
}

#[test] fn analytic_rectangle_clip_multiplies_coverage_end_to_end() {
    let (path, mut pixels) = (rectangle(0.0, 0.0, 3.0, 2.0), [0; 3 * 2 * 4]);
    let mut target = PixmapMut::new(&mut pixels, 3, 2, 12).unwrap();
    let mut buffers = AnalyticBuffers::<4, 3>::new();
    render_solid_analytic_clipped(&path, Affine::identity(), RGBA::white(),
        Rect::from_ltrb(0.5, 0.25, 2.5, 1.0).unwrap(),
        AnalyticRenderOptions::default(), &mut target,
        &mut buffers.workspace()).unwrap();
    assert_eq!((target.pixel_bytes(0, 0), target.pixel_bytes(1, 0),
                target.pixel_bytes(2, 0)),
        (Some([96; 4]), Some([191; 4]), Some([96; 4]))
    );
    assert_eq!(target.pixel_bytes(1, 1), Some([0; 4]));
}

#[test] fn analytic_path_clip_uses_reusable_caller_owned_coverage() {
    let clip = rectangle(0.5, 0.0, 1.5, 1.0);
    let shape = rectangle(0.0, 0.0, 2.0, 1.0);
    let (mut mask_data, mut pixels) = ([17; 4], [0; 8]);
    let mut buffers = AnalyticBuffers::<4, 2>::new();
    let mut workspace = buffers.workspace();
    let mut mask = CoverageMaskMut::new(&mut mask_data, 2, 1, 4).unwrap();
    rasterize_path_clip_analytic(&clip, Affine::identity(),
        AnalyticRenderOptions::default(), &mut mask, &mut workspace).unwrap();
    assert_eq!(mask_data, [128, 128, 17, 17]);

    let mask = CoverageMask::new(&mask_data, 2, 1, 4).unwrap();
    render_solid_analytic_masked(&shape, Affine::identity(), RGBA::white(),
        mask, AnalyticRenderOptions::default(),
        &mut PixmapMut::new(&mut pixels, 2, 1, 8).unwrap(),
        &mut workspace).unwrap();
    assert_eq!(pixels, [128; 8]);
}
