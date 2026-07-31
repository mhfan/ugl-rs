use super::*;
use crate::{analytic::AnalyticIntersection, edge::Edge, geometry::{PathBuilder, Point},
    raster::CoverageMask, sampler::{GradientStop, GradientStops, LinearGradient, SpreadMode},
    stroke::StrokeContour,
};

struct Buffers {
    points: [Point; 8], contours: [StrokeContour; 2], edges: [Edge; 32],
    intersections: [AnalyticIntersection; 32], row_coverage: [f32; 4],
    row_offsets: [u32; 5], edge_indices: [u32; 32],
}

impl Buffers {
    fn new() -> Self { Self {
        points: [Point::default(); 8], contours: [StrokeContour::default(); 2],
        edges: [Edge::default(); 32],
        intersections: [AnalyticIntersection::default(); 32], row_coverage: [0.0; 4],
        row_offsets: [0; 5], edge_indices: [0; 32],
    } }

    fn workspace(&mut self) -> AnalyticStrokeWorkspace<'_> {
        AnalyticStrokeWorkspace {
            points: &mut self.points, contours: &mut self.contours,
            edges: &mut self.edges, intersections: &mut self.intersections,
            row_coverage: &mut self.row_coverage, row_offsets: &mut self.row_offsets,
            edge_indices: &mut self.edge_indices,
        }
    }
}

fn rectangle() -> Path {
    let mut builder = PathBuilder::new();
    builder.move_to((0.0, 0.0)).line_to((2.0, 0.0))
        .line_to((2.0, 2.0)).line_to((0.0, 2.0));
    builder.build()
}

#[test] fn context_fill_state_and_clip_match_low_level_pipeline() {
    let mut pixels = [0; 4 * 4 * 4];
    let mut target = PixmapMut::new(&mut pixels, 4, 4, 16).unwrap();
    let mut buffers = Buffers::new();
    let mut workspace = buffers.workspace();
    let mask_data = [
        255, 128, 0, 0,
        255, 128, 0, 0,
        0,   0,   0, 0,
        0,   0,   0, 0,
    ];
    let mut context = Context::new(&mut target, &mut workspace);
    context.set_color(SRGBA::new(255, 0, 0, 128))
        .set_transform(Affine::translate(1.0, 0.0))
        .set_clip_mask(CoverageMask::new(&mask_data, 4, 4, 4).unwrap());
    context.fill(&rectangle()).unwrap();
    assert_eq!(&pixels[..16], &[0, 0, 0, 0, 64, 0, 0, 64, 0, 0, 0, 0, 0, 0, 0, 0]);
}

#[test] fn context_stroke_and_custom_paint_share_current_state() {
    let mut builder = PathBuilder::new();
    builder.move_to((0.0, 1.0)).line_to((4.0, 1.0));
    let path = builder.build();
    let stops = [GradientStop::new(0.0, SRGBA::red()),
                 GradientStop::new(1.0, SRGBA::blue())];
    let gradient = LinearGradient::new((0.0, 0.0), (4.0, 0.0),
        GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
    let mut pixels = [0; 4 * 3 * 4];
    let mut target = PixmapMut::new(&mut pixels, 4, 3, 16).unwrap();
    let mut buffers = Buffers::new();
    let mut workspace = buffers.workspace();
    let mut context = Context::new(&mut target, &mut workspace);
    context.set_stroke(StrokeOptions::new(2.0).unwrap())
        .set_clip_rect(Rect::from_ltrb(1.0, 0.0, 3.0, 3.0).unwrap());
    context.stroke_with(&path, &gradient).unwrap();
    for y in 0..2 {
        let row = &pixels[y * 16..(y + 1) * 16];
        assert_eq!(&row[..4], &[0; 4]);
        assert!(row[7] != 0 && row[11] != 0);
        assert_eq!(&row[12..], &[0; 4]);
    }
    assert_eq!(&pixels[32..], &[0; 16]);
}
