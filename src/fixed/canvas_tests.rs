use super::*;
use crate::{analytic::{Cell as AnalyticCell, Intersection as AnalyticIntersection}, canvas::{
        RenderOptions as FloatRenderOptions, RenderWorkspace as FloatRenderWorkspace,
        rasterize_path_clip as rasterize_float_path_clip},
    color::{PremulRGBA, PremulSRGBA8, SRGBA as RGBA}, edge::Edge,
    geometry::{Affine, PathBuilder}, sampler::SpreadMode};

#[test] fn planners_return_exact_capacities_for_fill_stroke_and_dash() {
    use crate::{fixed::{Scalar, dash::Pattern}, stroke::StrokeContour};

    let fixed = |value: f32| Scalar::from_num(value);
    let mut builder = PathBuilder::new();
    builder.move_to((fixed(0.5), fixed(0.5))).line_to((fixed(3.5), fixed(0.5)))
        .line_to((fixed(3.5), fixed(3.5))).line_to((fixed(0.5), fixed(3.5)));
    let path = builder.build();
    let (mut edges, mut lines) =
        ([Edge::<Scalar>::default(); 32], [Line::default(); 32]);
    let fill = render_requirements(&path, RenderOptions::default(), (4, 4),
        &mut GeometryWorkspace { edges: &mut edges, lines: &mut lines }).unwrap();
    assert_eq!(fill, RenderRequirements {
        edges: 2, lines: 2, segments: 2, trapezoids: 1, row_area: 4,
        strip_offsets: 2, strip_indices: 2,
    });

    let mut line_builder = PathBuilder::new();
    line_builder.move_to((fixed(0.0), fixed(1.0))).line_to((fixed(4.0), fixed(1.0)));
    let line = line_builder.build();
    let (mut points, mut contours) = (
        [(Scalar::ZERO, Scalar::ZERO).into(); 8], [StrokeContour::default(); 4],
    );
    let stroke = stroke_requirements(&line, StrokePathOptions::default(), (4, 3),
        &mut StrokePlanningWorkspace {
            path: StrokePathWorkspace { points: &mut points, contours: &mut contours },
            geometry: GeometryWorkspace { edges: &mut edges, lines: &mut lines },
        }).unwrap();
    assert_eq!((stroke.points, stroke.contours, stroke.render.edges), (2, 1, 2));

    let lengths = [fixed(1.0), fixed(1.0)];
    let pattern = Pattern::new(&lengths, Scalar::ZERO).unwrap();
    let (mut dash_points, mut dash_contours) = (
        [(Scalar::ZERO, Scalar::ZERO).into(); 8], [DashContour::default(); 4],
    );
    let dashed = dashed_stroke_requirements(&line, DashedStrokePathOptions {
        path: StrokePathOptions::default(), dash: pattern,
    }, (4, 3), &mut DashedStrokeWorkspace {
        path: StrokePathWorkspace { points: &mut points, contours: &mut contours },
        dash_points: &mut dash_points, dash_contours: &mut dash_contours,
        geometry: GeometryWorkspace { edges: &mut edges, lines: &mut lines },
    }).unwrap();
    assert_eq!((dashed.stroke.points, dashed.stroke.contours), (2, 1));
    assert_eq!((dashed.dash_points, dashed.dash_contours), (4, 2));
    assert_eq!(dashed.stroke.render.edges, 4);
}

struct AnalyticBuffers<const EDGES: usize, const WIDTH: usize> {
    intersections: [AnalyticIntersection; EDGES],
    edges: [Edge; EDGES], cells: [AnalyticCell; WIDTH],
    row_offsets: [u32; 9], edge_indices: [u32; EDGES],
}

impl<const EDGES: usize, const WIDTH: usize> AnalyticBuffers<EDGES, WIDTH> {
    fn new() -> Self { Self {
        intersections: [AnalyticIntersection::default(); EDGES],
        edges: [Edge::default(); EDGES], cells: [AnalyticCell::default(); WIDTH],
        row_offsets: [0; 9], edge_indices: [0; EDGES],
    } }

    fn workspace(&mut self) -> FloatRenderWorkspace<'_> {
        FloatRenderWorkspace {
            edges: &mut self.edges, intersections: &mut self.intersections,
            cells: &mut self.cells, row_offsets: &mut self.row_offsets,
            edge_indices: &mut self.edge_indices,
        }
    }
}

#[test] fn path_clip_supports_curves_and_preserves_mask_on_geometry_error() {
    use crate::{fixed::Scalar,
        fixed::raster::{Line, Workspace, Segment, Trapezoid},
    };

    let fixed = Scalar::from_num;
    let mut builder = PathBuilder::new();
    builder.move_to((fixed(0.5), fixed(2.5)))
        .quad_to((fixed(2.0), fixed(-0.5)), (fixed(3.5), fixed(2.5)))
        .line_to((fixed(0.5), fixed(2.5))).close();
    let path = builder.build();
    let (mut edges, mut lines) = ([Edge::default(); 32], [Line::default(); 32]);
    let (mut segments, mut trapezoids, mut row_area) =
        ([Segment::default(); 32], [Trapezoid::default(); 16], [0; 4]);
    let (mut strip_offsets, mut strip_indices) = ([0; 2], [0; 32]);
    let mut mask_data = [17; 4 * 3 + 2];
    rasterize_path_clip(&path, RenderOptions::default(),
        &mut CoverageMaskMut::new(&mut mask_data, 4, 3, 4).unwrap(),
        &mut GeometryWorkspace { edges: &mut edges, lines: &mut lines },
        &mut Workspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }).unwrap();
    assert!(mask_data[..12].iter().any(|&coverage| coverage != 0));
    assert!(mask_data[..12].iter().any(|&coverage| coverage != u8::MAX));
    assert_eq!(mask_data[12..], [17, 17]);

    let mut reference_builder = PathBuilder::new();
    reference_builder.move_to((0.5, 2.5))
        .quad_to((2.0, -0.5), (3.5, 2.5)).line_to((0.5, 2.5)).close();
    let mut reference_data = [0; 12];
    let mut reference_buffers = AnalyticBuffers::<32, 4>::new();
    rasterize_float_path_clip(&reference_builder.build(), Affine::identity(),
        FloatRenderOptions::default(),
        &mut CoverageMaskMut::new(&mut reference_data, 4, 3, 4).unwrap(),
        &mut reference_buffers.workspace()).unwrap();
    for (fixed, reference) in mask_data[..12].iter().zip(reference_data) {
        assert!(fixed.abs_diff(reference) <= 2,
            "fixed={fixed}, reference={reference}");
    }

    let mut untouched = [23; 12];
    assert_eq!(rasterize_path_clip(&path, RenderOptions::default(),
        &mut CoverageMaskMut::new(&mut untouched, 4, 3, 4).unwrap(),
        &mut GeometryWorkspace { edges: &mut [], lines: &mut lines },
        &mut Workspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }), Err(RenderError::EdgeCapacity { needed_at_least: 1 }));
    assert_eq!(untouched, [23; 12]);
}


#[test] fn coverage_uses_shared_paint_and_clip_compositor() {
    use crate::{fixed::{Scalar, raster::{
            CoverageRun, CoverageStrip, CoverageWorkspace, Line,
            Workspace, Segment, Trapezoid, prepare_lines,
            rasterize_lines_to_strips,
        }},
        fixed::sampler::LinearGradient,
        fixed::tile::{CoverageTile, CoverageTileRun, DirectTilePiece,
            DirectTileWorkspace, rasterize_lines_to_tiles,
        },
    };

    let fixed = Scalar::from_num;
    let edges = [Edge { upper: (fixed(0.5), fixed(0.0)).into(),
                        lower: (fixed(0.5), fixed(1.0)).into(), winding: 1 },
                 Edge { upper: (fixed(1.5), fixed(0.0)).into(),
                        lower: (fixed(1.5), fixed(1.0)).into(), winding: -1 }];
    let mut trapezoids = [Trapezoid::default(); 1];
    let (mut segments, mut pixels) = ([Segment::default(); 2], [0; 8]);
    let (mut lines, mut row_area) = ([Line::default(); 2], [0; 2]);
    let (mut strip_offsets, mut strip_indices) = ([0; 2], [0; 2]);
    prepare_lines(&edges, &mut lines).unwrap();
    let mut target = Pixmap::from_buffer(&mut pixels, 2, 1, 8).unwrap();
    render_solid(&lines, RGBA::white(), FillRule::NonZero, &mut target,
        &mut Workspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }).unwrap();
    assert_eq!(target.pixel_bytes(0, 0), Some([128; 4]));
    assert_eq!(target.pixel_bytes(1, 0), Some([128; 4]));

    let (mut coverage_strips, mut coverage_runs) =
        ([CoverageStrip::default(); 1], [CoverageRun::default(); 2]);
    let strips = rasterize_lines_to_strips(&lines, 2, 1, FillRule::NonZero,
        &mut Workspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
        }, CoverageWorkspace {
            strips: &mut coverage_strips, runs: &mut coverage_runs,
        }).unwrap();
    let mut tiled_pixels = [0; 8];
    let mut tiled_target = Pixmap::from_buffer(&mut tiled_pixels, 2, 1, 8).unwrap();
    let (mut tiles, mut runs, mut pieces) =  ([CoverageTile::default(); 1],
        [CoverageTileRun::default(); 2], [DirectTilePiece::default(); 2]);
    render_solid_tiled(&lines, RGBA::white(), FillRule::NonZero, &mut tiled_target,
        &mut Workspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        },
        DirectTileWorkspace {
            tiles: &mut tiles, runs: &mut runs, pieces: &mut pieces,
            column_heads: &mut [0], column_tails: &mut [0], touched_columns: &mut [0],
        },
    ).unwrap();
    assert_eq!(tiled_pixels, pixels);

    let tiled = rasterize_lines_to_tiles(&lines, 2, 1, FillRule::NonZero,
        &mut Workspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        },
        DirectTileWorkspace {
            tiles: &mut tiles, runs: &mut runs, pieces: &mut pieces,
            column_heads: &mut [0], column_tails: &mut [0], touched_columns: &mut [0],
        },
    ).unwrap();
    let mut cached_pixels = [0; 8];
    composite_solid_tiles(tiled, RGBA::white(),
        &mut Pixmap::from_buffer(&mut cached_pixels, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(cached_pixels, pixels);
    let ramp = [PremulSRGBA8::new(255, 0, 0, 255).unwrap(),
                PremulSRGBA8::new(0, 0, 255, 255).unwrap()];
    let gradient = LinearGradient::new(
        (fixed(0.0), fixed(0.0)), (fixed(2.0), fixed(0.0)),
        &ramp, SpreadMode::Pad).unwrap();
    let mut native_pixels = [0; 8];
    render_paint(&lines, &gradient, FillRule::NonZero,
        &mut Pixmap::from_buffer(&mut native_pixels, 2, 1, 8).unwrap(),
        &mut Workspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
        }).unwrap();
    assert_eq!(native_pixels, [128, 0, 0, 128, 0, 0, 128, 128]);

    let mut native_retained = [0; 8];
    composite_paint_strips(strips, &gradient,
        &mut Pixmap::from_buffer(&mut native_retained, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(native_retained, native_pixels);
    native_retained.fill(0);
    composite_paint_strips_clipped(strips, &gradient,
        Rect::from_ltrb(0.5, 0.0, 1.0, 1.0).unwrap(),
        &mut Pixmap::from_buffer(&mut native_retained, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(native_retained, [64, 0, 0, 64, 0, 0, 0, 0]);
    native_retained.fill(0);
    composite_paint_strips_masked(strips, &gradient,
        CoverageMask::new(&[128, 255], 2, 1, 2).unwrap(),
        &mut Pixmap::from_buffer(&mut native_retained, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(native_retained, [64, 0, 0, 64, 0, 0, 128, 128]);
    native_retained.fill(0);
    composite_paint_tiles(tiled, &gradient,
        &mut Pixmap::from_buffer(&mut native_retained, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(native_retained, native_pixels);
    native_retained.fill(0);
    composite_paint_tiles_clipped(tiled, &gradient,
        Rect::from_ltrb(0.5, 0.0, 1.0, 1.0).unwrap(),
        &mut Pixmap::from_buffer(&mut native_retained, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(native_retained, [64, 0, 0, 64, 0, 0, 0, 0]);
    native_retained.fill(0);
    composite_paint_tiles_masked(tiled, &gradient,
        CoverageMask::new(&[128, 255], 2, 1, 2).unwrap(),
        &mut Pixmap::from_buffer(&mut native_retained, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(native_retained, [64, 0, 0, 64, 0, 0, 128, 128]);

    let mut mismatched_pixels = [17; 4];
    let error = composite_solid_tiles(tiled, RGBA::white(),
        &mut Pixmap::from_buffer(&mut mismatched_pixels, 1, 1, 4).unwrap());
    assert_eq!(error, Err(RenderError::CoverageDimensionsMismatch {
        coverage: (2, 1), target: (1, 1) }));
    assert_eq!(mismatched_pixels, [17; 4]);

    let mut untouched = [17; 8];
    assert!(render_paint(&lines, &gradient, FillRule::NonZero,
        &mut Pixmap::from_buffer(&mut untouched, 2, 1, 8).unwrap(),
        &mut Workspace { segments: &mut [],
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
        }).is_err());
    assert_eq!(untouched, [17; 8]);
}


#[test] fn stroke_renders_end_to_end_without_floating_point() {
    use crate::{fixed::Scalar,
        fixed::raster::{Line, Workspace, Segment, Trapezoid},
        fixed::stroke::Options as StrokeOptions,
    };

    let fixed = Scalar::from_num;
    let points = [(fixed(1), fixed(1)).into(), (fixed(3), fixed(1)).into()];
    let (mut edge_storage, mut line_storage) =
        ([Edge::default(); 2], [Line::default(); 2]);
    let (mut segments, mut trapezoids, mut row_area) =
        ([Segment::default(); 2], [Trapezoid::default(); 1], [0; 4]);
    let (mut strip_offsets, mut strip_indices) = ([0; 2], [0; 2]);
    let mut pixels = [0; 4 * 3 * 4];
    render_stroke_polyline(&points, false,
        StrokeOptions::new(fixed(2)).unwrap(), &SolidPaint::new(RGBA::white()),
        &mut Pixmap::from_buffer(&mut pixels, 4, 3, 16).unwrap(),
        &mut GeometryWorkspace {
            edges: &mut edge_storage, lines: &mut line_storage,
        },
        &mut Workspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }).unwrap();
    let target = Pixmap::from_buffer(&mut pixels, 4, 3, 16).unwrap();
    for y in 0..3 {
        for x in 0..4 {
            let expected = if y < 2 && (1..3).contains(&x) {
                (255, 255, 255, 255).into()
            } else { PremulRGBA::zeroed() };
            assert_eq!(target.pixel_bytes(x, y), Some(expected.to_array()),
                "pixel=({x}, {y})");
        }
    }
}


#[test] fn curved_stroke_path_uses_bounded_workspaces() {
    use crate::{fixed::Scalar, geometry::PathBuilder,
        fixed::raster::{Line, Workspace, Segment, Trapezoid},
        stroke::{StrokeContour, StrokePathWorkspace},
    };

    let fixed = Scalar::from_num;
    let mut builder = PathBuilder::new();
    builder.move_to((fixed(0), fixed(1)))
        .quad_to((fixed(1), fixed(-1)), (fixed(2), fixed(1)));
    let path = builder.build();
    let mut points = [(Scalar::ZERO, Scalar::ZERO).into(); 32];
    let mut contours = [StrokeContour::default(); 2];
    let (mut edges, mut lines) = ([Edge::default(); 128], [Line::default(); 128]);
    let (mut segments, mut trapezoids, mut row_area) =
        ([Segment::default(); 128], [Trapezoid::default(); 64], [0; 7]);
    let (mut strip_offsets, mut strip_indices) = ([0; 7], [0; 128]);
    let mut pixels = [0; 6 * 6 * 4];
    render_stroke_path(&path, &SolidPaint::new(RGBA::white()),
        StrokePathOptions {
            transform: Affine::translate(fixed(2), fixed(2)),
            ..StrokePathOptions::default()
        },
        &mut Pixmap::from_buffer(&mut pixels, 6, 6, 24).unwrap(),
        &mut StrokePathWorkspace { points: &mut points, contours: &mut contours },
        &mut GeometryWorkspace { edges: &mut edges, lines: &mut lines },
        &mut Workspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }).unwrap();
    assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));

    let mut untouched = [17; 6 * 6 * 4];
    assert_eq!(render_stroke_path(&path, &SolidPaint::new(RGBA::white()),
        StrokePathOptions::default(),
        &mut Pixmap::from_buffer(&mut untouched, 6, 6, 24).unwrap(),
        &mut StrokePathWorkspace { points: &mut [], contours: &mut contours },
        &mut GeometryWorkspace { edges: &mut edges, lines: &mut lines },
        &mut Workspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }), Err(RenderError::StrokePointCapacity { needed_at_least: 1 }));
    assert_eq!(untouched, [17; 6 * 6 * 4]);
}


#[test] fn dashed_path_matches_f32_reference_coverage() {
    use crate::{dash::DashContour, fixed::{Scalar, dash::Pattern},
        geometry::PathBuilder,
        fixed::raster::{Line, Workspace, Segment, Trapezoid},
        stroke::{StrokeContour, StrokePathWorkspace},
    };

    let fixed = Scalar::from_num;
    let mut builder = PathBuilder::new();
    builder.move_to((fixed(0.5), fixed(0.5))).line_to((fixed(4.5), fixed(0.5)));
    let pattern_lengths = [fixed(1.0), fixed(1.0)];
    let mut path_points = [(Scalar::ZERO, Scalar::ZERO).into(); 2];
    let mut path_contours = [StrokeContour::default(); 1];
    let mut dash_points = [(Scalar::ZERO, Scalar::ZERO).into(); 8];
    let mut dash_contours = [DashContour::default(); 4];
    let (mut edges, mut lines) = ([Edge::default(); 8], [Line::default(); 8]);
    let (mut segments, mut trapezoids, mut row_area) =
        ([Segment::default(); 8], [Trapezoid::default(); 4], [0; 6]);
    let (mut strip_offsets, mut strip_indices) = ([0; 2], [0; 8]);
    let mut pixels = [0; 20];
    render_dashed_stroke_path(&builder.build(),
        &SolidPaint::new(RGBA::white()), DashedStrokePathOptions {
            path: StrokePathOptions::default(),
            dash: Pattern::new(&pattern_lengths, Scalar::ZERO).unwrap(),
        }, &mut Pixmap::from_buffer(&mut pixels, 5, 1, 20).unwrap(),
        &mut DashedStrokeWorkspace {
            path: StrokePathWorkspace {
                points: &mut path_points, contours: &mut path_contours,
            },
            dash_points: &mut dash_points, dash_contours: &mut dash_contours,
            geometry: GeometryWorkspace { edges: &mut edges, lines: &mut lines },
        },
        &mut Workspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }).unwrap();
    let alpha: alloc::vec::Vec<_> = pixels.chunks_exact(4).map(|pixel| pixel[3]).collect();
    assert_eq!(alpha, [128, 128, 128, 128, 0]);
}


#[test] fn curved_path_renders_end_to_end() {
    use crate::{fixed::Scalar, geometry::PathBuilder,
        fixed::raster::{Line, Workspace, Segment, Trapezoid},
    };

    let fixed = Scalar::from_num;
    let mut builder = PathBuilder::new();
    builder.move_to((fixed(1), fixed(1)))
        .quad_to((fixed(2), fixed(0)), (fixed(3), fixed(1)))
        .line_to((fixed(3), fixed(3))).line_to((fixed(1), fixed(3))).close();
    let (mut edge_storage, mut line_storage) =
        ([Edge::default(); 32], [Line::default(); 32]);
    let (mut segments, mut trapezoids, mut row_area) =
        ([Segment::default(); 64], [Trapezoid::default(); 32], [0; 5]);
    let (mut strip_offsets, mut strip_indices) = ([0; 5], [0; 64]);
    let mut pixels = [0; 4 * 4 * 4];
    render_path(&builder.build(), &SolidPaint::new(RGBA::white()),
        RenderOptions::default(),
        &mut Pixmap::from_buffer(&mut pixels, 4, 4, 16).unwrap(),
        &mut GeometryWorkspace {
            edges: &mut edge_storage, lines: &mut line_storage,
        },
        &mut Workspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }).unwrap();
    let target = Pixmap::from_buffer(&mut pixels, 4, 4, 16).unwrap();
    assert_eq!(target.pixel_bytes(1, 1), Some([255; 4]));
    assert_eq!(target.pixel_bytes(2, 2), Some([255; 4]));
    assert_eq!(target.pixel_bytes(0, 0), Some([0; 4]));
}


#[test] fn full_tile_blending_matches_row_spans() {
    let (mut tiled, mut spanned) = ([17; 16 * 16 * 4], [17; 16 * 16 * 4]);
    let color = RGBA::new(40, 120, 220, 192).premul_encoded();
    Pixmap::from_buffer(&mut tiled, 16, 16, 64).unwrap().blend_solid_tile(0, 0, 16, 16, color);
    let mut target = Pixmap::from_buffer(&mut spanned, 16, 16, 64).unwrap();
    for y in 0..16 { target.blend_solid_span(0, y, 16, color, u8::MAX); }
    assert_eq!(tiled, spanned);
}
