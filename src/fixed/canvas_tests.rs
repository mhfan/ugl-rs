use super::*;
use crate::{analytic::AnalyticIntersection, canvas::{
        AnalyticRenderOptions, AnalyticRenderWorkspace, rasterize_path_clip_analytic,
        render_paint_analytic, render_paint_analytic_clipped,
        render_paint_analytic_masked},
    color::{PremulRGBA, PremulSRGBA8, RGBA as GenericRGBA, SRGBA as RGBA}, edge::Edge,
    geometry::{Affine, Path, PathBuilder}, sampler::SpreadMode};

fn rectangle(left: f32, top: f32, right: f32, bottom: f32) -> Path {
    let mut builder = PathBuilder::new();
    builder.move_to((left, top)).line_to((right, top))
           .line_to((right, bottom)).line_to((left, bottom));
    builder.build()
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
        AnalyticRenderWorkspace {
            edges: &mut self.edges, intersections: &mut self.intersections,
            row_coverage: &mut self.row_coverage, row_offsets: &mut self.row_offsets,
            edge_indices: &mut self.edge_indices,
        }
    }
}

#[test] fn fixed_path_clip_supports_curves_and_preserves_mask_on_geometry_error() {
    use crate::{geometry::FixedScalar,
        raster_fixed::{FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid},
    };

    let fixed = FixedScalar::from_num;
    let mut builder = PathBuilder::new();
    builder.move_to((fixed(0.5), fixed(2.5)))
        .quad_to((fixed(2.0), fixed(-0.5)), (fixed(3.5), fixed(2.5)))
        .line_to((fixed(0.5), fixed(2.5))).close();
    let path = builder.build();
    let (mut edges, mut lines) = ([Edge::default(); 32], [FixedLine::default(); 32]);
    let (mut segments, mut trapezoids, mut row_area) =
        ([FixedSegment::default(); 32], [FixedTrapezoid::default(); 16], [0; 4]);
    let (mut strip_offsets, mut strip_indices) = ([0; 2], [0; 32]);
    let mut mask_data = [17; 4 * 3 + 2];
    rasterize_path_clip_fixed(&path, FixedRenderOptions::default(),
        &mut CoverageMaskMut::new(&mut mask_data, 4, 3, 4).unwrap(),
        &mut FixedGeometryWorkspace { edges: &mut edges, lines: &mut lines },
        &mut FixedRasterWorkspace {
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
    rasterize_path_clip_analytic(&reference_builder.build(), Affine::identity(),
        AnalyticRenderOptions::default(),
        &mut CoverageMaskMut::new(&mut reference_data, 4, 3, 4).unwrap(),
        &mut reference_buffers.workspace()).unwrap();
    for (fixed, reference) in mask_data[..12].iter().zip(reference_data) {
        assert!(fixed.abs_diff(reference) <= 2,
            "fixed={fixed}, reference={reference}");
    }

    let mut untouched = [23; 12];
    assert_eq!(rasterize_path_clip_fixed(&path, FixedRenderOptions::default(),
        &mut CoverageMaskMut::new(&mut untouched, 4, 3, 4).unwrap(),
        &mut FixedGeometryWorkspace { edges: &mut [], lines: &mut lines },
        &mut FixedRasterWorkspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }), Err(RenderError::EdgeCapacity { needed_at_least: 1 }));
    assert_eq!(untouched, [23; 12]);
}


#[test] fn fixed_coverage_uses_shared_paint_and_clip_compositor() {
    use crate::{geometry::FixedScalar, raster_fixed::{
            FixedCoverageRun, FixedCoverageStrip, FixedCoverageWorkspace, FixedLine,
            FixedRasterWorkspace, FixedSegment, FixedTrapezoid, prepare_lines,
            rasterize_lines_to_strips,
        },
        sampler::FixedLinearGradient,
        tile_fixed::{FixedCoverageTile, FixedCoverageTileRun, FixedDirectTilePiece,
            FixedDirectTileWorkspace, rasterize_lines_to_tiles,
        },
    };

    let fixed = FixedScalar::from_num;
    let edges = [Edge { upper: (fixed(0.5), fixed(0.0)).into(),
                        lower: (fixed(0.5), fixed(1.0)).into(), winding: 1 },
                 Edge { upper: (fixed(1.5), fixed(0.0)).into(),
                        lower: (fixed(1.5), fixed(1.0)).into(), winding: -1 }];
    let mut trapezoids = [FixedTrapezoid::default(); 1];
    let (mut segments, mut pixels) = ([FixedSegment::default(); 2], [0; 8]);
    let (mut lines, mut row_area) = ([FixedLine::default(); 2], [0; 2]);
    let (mut strip_offsets, mut strip_indices) = ([0; 2], [0; 2]);
    prepare_lines(&edges, &mut lines).unwrap();
    let mut target = PixmapMut::new(&mut pixels, 2, 1, 8).unwrap();
    render_solid_fixed(&lines, RGBA::white(), FillRule::NonZero, &mut target,
        &mut FixedRasterWorkspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }).unwrap();
    assert_eq!(target.pixel_bytes(0, 0), Some([128; 4]));
    assert_eq!(target.pixel_bytes(1, 0), Some([128; 4]));

    struct CoordinatePaint;
    impl PaintSampler for CoordinatePaint {
        fn sample(&self, x: f32, y: f32) -> PremulSRGBA8 {
            PremulSRGBA8::new((x * 40.0) as _, (y * 40.0) as _, 0, u8::MAX).unwrap()
        }
    }
    let mut painted_pixels = [0; 8];
    render_paint_fixed(&lines, &CoordinatePaint, FillRule::NonZero,
        &mut PixmapMut::new(&mut painted_pixels, 2, 1, 8).unwrap(),
        &mut FixedRasterWorkspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
        }).unwrap();
    assert_eq!(painted_pixels, [10, 10, 0, 128, 30, 10, 0, 128]);
    let path = rectangle(0.5, 0.0, 1.5, 1.0);
    let mut analytic_pixels = [0; 8];
    render_paint_analytic(&path, Affine::identity(), &CoordinatePaint,
        AnalyticRenderOptions::default(),
        &mut PixmapMut::new(&mut analytic_pixels, 2, 1, 8).unwrap(),
        &mut AnalyticBuffers::<2, 2>::new().workspace()).unwrap();
    assert_eq!(painted_pixels, analytic_pixels);

    let mut clipped_pixels = [0; 8];
    render_paint_fixed_clipped(&lines, &CoordinatePaint,
        Rect::from_ltrb(0.5, 0.0, 1.0, 1.0).unwrap(), FillRule::NonZero,
        &mut PixmapMut::new(&mut clipped_pixels, 2, 1, 8).unwrap(),
        &mut FixedRasterWorkspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
        }).unwrap();
    assert_eq!(clipped_pixels, [5, 5, 0, 64, 0, 0, 0, 0]);
    analytic_pixels.fill(0);
    render_paint_analytic_clipped(&path, Affine::identity(), &CoordinatePaint,
        Rect::from_ltrb(0.5, 0.0, 1.0, 1.0).unwrap(),
        AnalyticRenderOptions::default(),
        &mut PixmapMut::new(&mut analytic_pixels, 2, 1, 8).unwrap(),
        &mut AnalyticBuffers::<2, 2>::new().workspace()).unwrap();
    assert_eq!(clipped_pixels, analytic_pixels);

    let mut masked_pixels = [0; 8];
    render_paint_fixed_masked(&lines, &CoordinatePaint,
        CoverageMask::new(&[128, 255], 2, 1, 2).unwrap(), FillRule::NonZero,
        &mut PixmapMut::new(&mut masked_pixels, 2, 1, 8).unwrap(),
        &mut FixedRasterWorkspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
        }).unwrap();
    assert_eq!(masked_pixels, [5, 5, 0, 64, 30, 10, 0, 128]);
    analytic_pixels.fill(0);
    render_paint_analytic_masked(&path, Affine::identity(), &CoordinatePaint,
        CoverageMask::new(&[128, 255], 2, 1, 2).unwrap(),
        AnalyticRenderOptions::default(),
        &mut PixmapMut::new(&mut analytic_pixels, 2, 1, 8).unwrap(),
        &mut AnalyticBuffers::<2, 2>::new().workspace()).unwrap();
    assert_eq!(masked_pixels, analytic_pixels);

    let (mut coverage_strips, mut coverage_runs) =
        ([FixedCoverageStrip::default(); 1], [FixedCoverageRun::default(); 2]);
    let strips = rasterize_lines_to_strips(&lines, 2, 1, FillRule::NonZero,
        &mut FixedRasterWorkspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
        }, FixedCoverageWorkspace {
            strips: &mut coverage_strips, runs: &mut coverage_runs,
        }).unwrap();
    let mut retained_pixels = [0; 8];
    composite_paint_fixed_strips(strips, &CoordinatePaint,
        &mut PixmapMut::new(&mut retained_pixels, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(retained_pixels, painted_pixels);
    retained_pixels.fill(0);
    composite_paint_fixed_strips_clipped(strips, &CoordinatePaint,
        Rect::from_ltrb(0.5, 0.0, 1.0, 1.0).unwrap(),
        &mut PixmapMut::new(&mut retained_pixels, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(retained_pixels, clipped_pixels);
    retained_pixels.fill(0);
    composite_paint_fixed_strips_masked(strips, &CoordinatePaint,
        CoverageMask::new(&[128, 255], 2, 1, 2).unwrap(),
        &mut PixmapMut::new(&mut retained_pixels, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(retained_pixels, masked_pixels);

    let mut tiled_pixels = [0; 8];
    let mut tiled_target = PixmapMut::new(&mut tiled_pixels, 2, 1, 8).unwrap();
    let (mut tiles, mut runs, mut pieces) =  ([FixedCoverageTile::default(); 1],
        [FixedCoverageTileRun::default(); 2], [FixedDirectTilePiece::default(); 2]);
    render_solid_fixed_tiled(&lines, RGBA::white(), FillRule::NonZero, &mut tiled_target,
        &mut FixedRasterWorkspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        },
        FixedDirectTileWorkspace {
            tiles: &mut tiles, runs: &mut runs, pieces: &mut pieces,
            column_heads: &mut [0], column_tails: &mut [0], touched_columns: &mut [0],
        },
    ).unwrap();
    assert_eq!(tiled_pixels, pixels);

    let mut painted_tiled_pixels = [0; 8];
    render_paint_fixed_tiled(&lines, &CoordinatePaint, FillRule::NonZero,
        &mut PixmapMut::new(&mut painted_tiled_pixels, 2, 1, 8).unwrap(),
        &mut FixedRasterWorkspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
        },
        FixedDirectTileWorkspace {
            tiles: &mut tiles, runs: &mut runs, pieces: &mut pieces,
            column_heads: &mut [0], column_tails: &mut [0], touched_columns: &mut [0],
        },
    ).unwrap();
    assert_eq!(painted_tiled_pixels, painted_pixels);

    let tiled = rasterize_lines_to_tiles(&lines, 2, 1, FillRule::NonZero,
        &mut FixedRasterWorkspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        },
        FixedDirectTileWorkspace {
            tiles: &mut tiles, runs: &mut runs, pieces: &mut pieces,
            column_heads: &mut [0], column_tails: &mut [0], touched_columns: &mut [0],
        },
    ).unwrap();
    let mut cached_pixels = [0; 8];
    composite_solid_fixed_tiles(tiled, RGBA::white(),
        &mut PixmapMut::new(&mut cached_pixels, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(cached_pixels, pixels);
    cached_pixels.fill(0);
    composite_paint_fixed_tiles_clipped(tiled, &CoordinatePaint,
        Rect::from_ltrb(0.5, 0.0, 1.0, 1.0).unwrap(),
        &mut PixmapMut::new(&mut cached_pixels, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(cached_pixels, clipped_pixels);
    cached_pixels.fill(0);
    composite_paint_fixed_tiles_masked(tiled, &CoordinatePaint,
        CoverageMask::new(&[128, 255], 2, 1, 2).unwrap(),
        &mut PixmapMut::new(&mut cached_pixels, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(cached_pixels, masked_pixels);

    let ramp = [PremulSRGBA8::new(255, 0, 0, 255).unwrap(),
                PremulSRGBA8::new(0, 0, 255, 255).unwrap()];
    let gradient = FixedLinearGradient::new(
        (fixed(0.0), fixed(0.0)), (fixed(2.0), fixed(0.0)),
        &ramp, SpreadMode::Pad).unwrap();
    let mut native_pixels = [0; 8];
    render_native_paint_fixed(&lines, &gradient, FillRule::NonZero,
        &mut PixmapMut::new(&mut native_pixels, 2, 1, 8).unwrap(),
        &mut FixedRasterWorkspace { segments: &mut segments,
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
        }).unwrap();
    assert_eq!(native_pixels, [128, 0, 0, 128, 0, 0, 128, 128]);

    let mut native_retained = [0; 8];
    composite_native_paint_fixed_strips(strips, &gradient,
        &mut PixmapMut::new(&mut native_retained, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(native_retained, native_pixels);
    native_retained.fill(0);
    composite_native_paint_fixed_strips_clipped(strips, &gradient,
        Rect::from_ltrb(0.5, 0.0, 1.0, 1.0).unwrap(),
        &mut PixmapMut::new(&mut native_retained, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(native_retained, [64, 0, 0, 64, 0, 0, 0, 0]);
    native_retained.fill(0);
    composite_native_paint_fixed_strips_masked(strips, &gradient,
        CoverageMask::new(&[128, 255], 2, 1, 2).unwrap(),
        &mut PixmapMut::new(&mut native_retained, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(native_retained, [64, 0, 0, 64, 0, 0, 128, 128]);
    native_retained.fill(0);
    composite_native_paint_fixed_tiles(tiled, &gradient,
        &mut PixmapMut::new(&mut native_retained, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(native_retained, native_pixels);
    native_retained.fill(0);
    composite_native_paint_fixed_tiles_clipped(tiled, &gradient,
        Rect::from_ltrb(0.5, 0.0, 1.0, 1.0).unwrap(),
        &mut PixmapMut::new(&mut native_retained, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(native_retained, [64, 0, 0, 64, 0, 0, 0, 0]);
    native_retained.fill(0);
    composite_native_paint_fixed_tiles_masked(tiled, &gradient,
        CoverageMask::new(&[128, 255], 2, 1, 2).unwrap(),
        &mut PixmapMut::new(&mut native_retained, 2, 1, 8).unwrap()).unwrap();
    assert_eq!(native_retained, [64, 0, 0, 64, 0, 0, 128, 128]);

    let mut mismatched_pixels = [17; 4];
    let error = composite_solid_fixed_tiles(tiled, RGBA::white(),
        &mut PixmapMut::new(&mut mismatched_pixels, 1, 1, 4).unwrap());
    assert_eq!(error, Err(RenderError::CoverageDimensionsMismatch {
        coverage: (2, 1), target: (1, 1) }));
    assert_eq!(mismatched_pixels, [17; 4]);

    let mut untouched = [17; 8];
    assert!(render_paint_fixed(&lines, &CoordinatePaint, FillRule::NonZero,
        &mut PixmapMut::new(&mut untouched, 2, 1, 8).unwrap(),
        &mut FixedRasterWorkspace { segments: &mut [],
            trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
        }).is_err());
    assert_eq!(untouched, [17; 8]);
}


#[test] fn native_fixed_stroke_renders_end_to_end_without_floating_point() {
    use crate::{geometry::FixedScalar,
        raster_fixed::{FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid},
        stroke_fixed::FixedStrokeOptions,
    };

    let fixed = FixedScalar::from_num;
    let points = [(fixed(1), fixed(1)).into(), (fixed(3), fixed(1)).into()];
    let (mut edge_storage, mut line_storage) =
        ([Edge::default(); 2], [FixedLine::default(); 2]);
    let (mut segments, mut trapezoids, mut row_area) =
        ([FixedSegment::default(); 2], [FixedTrapezoid::default(); 1], [0; 4]);
    let (mut strip_offsets, mut strip_indices) = ([0; 2], [0; 2]);
    let mut pixels = [0; 4 * 3 * 4];
    render_native_stroke_polyline_fixed(&points, false,
        FixedStrokeOptions::new(fixed(2)).unwrap(), &SolidPaint::new(RGBA::white()),
        &mut PixmapMut::new(&mut pixels, 4, 3, 16).unwrap(),
        &mut FixedGeometryWorkspace {
            edges: &mut edge_storage, lines: &mut line_storage,
        },
        &mut FixedRasterWorkspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }).unwrap();
    let target = PixmapMut::new(&mut pixels, 4, 3, 16).unwrap();
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


#[test] fn native_fixed_curved_stroke_path_uses_bounded_workspaces() {
    use crate::{geometry::{FixedScalar, PathBuilder},
        raster_fixed::{FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid},
        stroke::{StrokeContour, StrokePathWorkspace},
    };

    let fixed = FixedScalar::from_num;
    let mut builder = PathBuilder::new();
    builder.move_to((fixed(0), fixed(1)))
        .quad_to((fixed(1), fixed(-1)), (fixed(2), fixed(1)));
    let path = builder.build();
    let mut points = [(FixedScalar::ZERO, FixedScalar::ZERO).into(); 32];
    let mut contours = [StrokeContour::default(); 2];
    let (mut edges, mut lines) = ([Edge::default(); 128], [FixedLine::default(); 128]);
    let (mut segments, mut trapezoids, mut row_area) =
        ([FixedSegment::default(); 128], [FixedTrapezoid::default(); 64], [0; 7]);
    let (mut strip_offsets, mut strip_indices) = ([0; 7], [0; 128]);
    let mut pixels = [0; 6 * 6 * 4];
    render_native_stroke_path_fixed(&path, &SolidPaint::new(RGBA::white()),
        FixedStrokePathOptions {
            transform: Affine::translate(fixed(2), fixed(2)),
            ..FixedStrokePathOptions::default()
        },
        &mut PixmapMut::new(&mut pixels, 6, 6, 24).unwrap(),
        &mut StrokePathWorkspace { points: &mut points, contours: &mut contours },
        &mut FixedGeometryWorkspace { edges: &mut edges, lines: &mut lines },
        &mut FixedRasterWorkspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }).unwrap();
    assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));

    let mut untouched = [17; 6 * 6 * 4];
    assert_eq!(render_native_stroke_path_fixed(&path, &SolidPaint::new(RGBA::white()),
        FixedStrokePathOptions::default(),
        &mut PixmapMut::new(&mut untouched, 6, 6, 24).unwrap(),
        &mut StrokePathWorkspace { points: &mut [], contours: &mut contours },
        &mut FixedGeometryWorkspace { edges: &mut edges, lines: &mut lines },
        &mut FixedRasterWorkspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }), Err(RenderError::StrokePointCapacity { needed_at_least: 1 }));
    assert_eq!(untouched, [17; 6 * 6 * 4]);
}


#[test] fn native_fixed_dashed_path_matches_f32_reference_coverage() {
    use crate::{dash::{DashContour, FixedDashPattern},
        geometry::{FixedScalar, PathBuilder},
        raster_fixed::{FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid},
        stroke::{StrokeContour, StrokePathWorkspace},
    };

    let fixed = FixedScalar::from_num;
    let mut builder = PathBuilder::new();
    builder.move_to((fixed(0.5), fixed(0.5))).line_to((fixed(4.5), fixed(0.5)));
    let pattern_lengths = [fixed(1.0), fixed(1.0)];
    let mut path_points = [(FixedScalar::ZERO, FixedScalar::ZERO).into(); 2];
    let mut path_contours = [StrokeContour::default(); 1];
    let mut dash_points = [(FixedScalar::ZERO, FixedScalar::ZERO).into(); 8];
    let mut dash_contours = [DashContour::default(); 4];
    let (mut edges, mut lines) = ([Edge::default(); 8], [FixedLine::default(); 8]);
    let (mut segments, mut trapezoids, mut row_area) =
        ([FixedSegment::default(); 8], [FixedTrapezoid::default(); 4], [0; 6]);
    let (mut strip_offsets, mut strip_indices) = ([0; 2], [0; 8]);
    let mut pixels = [0; 20];
    render_native_stroke_path_dashed_fixed(&builder.build(),
        &SolidPaint::new(RGBA::white()), FixedDashedStrokePathOptions {
            path: FixedStrokePathOptions::default(),
            dash: FixedDashPattern::new(&pattern_lengths, FixedScalar::ZERO).unwrap(),
        }, &mut PixmapMut::new(&mut pixels, 5, 1, 20).unwrap(),
        &mut FixedDashedStrokeWorkspace {
            path: StrokePathWorkspace {
                points: &mut path_points, contours: &mut path_contours,
            },
            dash_points: &mut dash_points, dash_contours: &mut dash_contours,
            geometry: FixedGeometryWorkspace { edges: &mut edges, lines: &mut lines },
        },
        &mut FixedRasterWorkspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }).unwrap();
    let alpha: alloc::vec::Vec<_> = pixels.chunks_exact(4).map(|pixel| pixel[3]).collect();
    assert_eq!(alpha, [128, 128, 128, 128, 0]);
}


#[test] fn native_fixed_curved_path_renders_end_to_end() {
    use crate::{geometry::{FixedScalar, PathBuilder},
        raster_fixed::{FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid},
    };

    let fixed = FixedScalar::from_num;
    let mut builder = PathBuilder::new();
    builder.move_to((fixed(1), fixed(1)))
        .quad_to((fixed(2), fixed(0)), (fixed(3), fixed(1)))
        .line_to((fixed(3), fixed(3))).line_to((fixed(1), fixed(3))).close();
    let (mut edge_storage, mut line_storage) =
        ([Edge::default(); 32], [FixedLine::default(); 32]);
    let (mut segments, mut trapezoids, mut row_area) =
        ([FixedSegment::default(); 64], [FixedTrapezoid::default(); 32], [0; 5]);
    let (mut strip_offsets, mut strip_indices) = ([0; 5], [0; 64]);
    let mut pixels = [0; 4 * 4 * 4];
    render_native_path_fixed(&builder.build(), &SolidPaint::new(RGBA::white()),
        FixedRenderOptions::default(),
        &mut PixmapMut::new(&mut pixels, 4, 4, 16).unwrap(),
        &mut FixedGeometryWorkspace {
            edges: &mut edge_storage, lines: &mut line_storage,
        },
        &mut FixedRasterWorkspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        }).unwrap();
    let target = PixmapMut::new(&mut pixels, 4, 4, 16).unwrap();
    assert_eq!(target.pixel_bytes(1, 1), Some([255; 4]));
    assert_eq!(target.pixel_bytes(2, 2), Some([255; 4]));
    assert_eq!(target.pixel_bytes(0, 0), Some([0; 4]));
}


#[test] fn full_tile_blending_matches_row_spans() {
    let (mut tiled, mut spanned) = ([17; 16 * 16 * 4], [17; 16 * 16 * 4]);
    let color = GenericRGBA::<u8>::new(40, 120, 220, 192).premul();
    PixmapMut::new(&mut tiled, 16, 16, 64).unwrap().blend_solid_tile(0, 0, 16, 16, color);
    let mut target = PixmapMut::new(&mut spanned, 16, 16, 64).unwrap();
    for y in 0..16 { target.blend_solid_span(0, y, 16, color, u8::MAX); }
    assert_eq!(tiled, spanned);
}
