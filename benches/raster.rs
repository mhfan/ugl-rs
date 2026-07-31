
use std::hint::black_box;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ugl_rs::{analytic::{AnalyticBinWorkspace, AnalyticIntersection, AnalyticWorkspace,
        analytic_bin_requirements, build_analytic_row_bins, rasterize_edges_analytic_binned},
    color::RGBA, edge::Edge, raster::{FillRule, Intersection},
    canvas::{AnalyticRenderOptions, AnalyticRenderWorkspace, AnalyticStrokeOptions,
        AnalyticStrokeWorkspace, PixmapMut, RenderOptions, RenderWorkspace,
        render_solid, render_solid_analytic, render_stroke_solid_analytic,
    }, geometry::{Affine, Path, PathBuilder},
    sampler::{ConicGradient, GradientStop, GradientStops, LinearGradient, PaintSampler,
        RadialGradient, SolidPaint, SpreadMode},
    stroke::{LineCap, LineJoin, StrokeContour, StrokeOptions, StrokePathWorkspace,
        flatten_stroke_path, stroke_polyline},
};
#[derive(Default)] struct RunCounter { runs: u32, pixels: u32 }

impl ugl_rs::raster::CoverageSink for RunCounter {
    type Error = core::convert::Infallible;
    fn span(&mut self, _x: u32, _y: u32, len: u32, _coverage: u8) ->
        Result<(), Self::Error> {
        self.runs += 1;  self.pixels += len;  Ok(())
    }
}

const  WIDTH: u32 = 256;
const HEIGHT: u32 = 256;
const SHAPES: usize = 64;
const EDGE_CAPACITY: usize = SHAPES * 2;

fn rectangle_scene() -> Path {
    let mut path = PathBuilder::with_capacity(SHAPES * 5);
    for index in 0..SHAPES {
        let x = (index % 8) as f32 * 30.0 + 4.25;
        let y = (index / 8) as f32 * 30.0 + 4.5;
        path.move_to((x, y)).line_to((x + 22.5, y))
            .line_to((x + 22.5, y + 21.75)).line_to((x, y + 21.75));
    }   path.build()
}

fn benchmark_f32(c: &mut Criterion) {
    let path = rectangle_scene();
    let mut group = c.benchmark_group("raster_rgba8888");
    let mut pixels = vec![0; WIDTH as usize * HEIGHT as usize * 4];
    group.throughput(Throughput::Elements((WIDTH as u64) * HEIGHT as u64));

    let (mut edges, mut intersections, mut row_coverage) = (
        vec![Edge::default(); EDGE_CAPACITY],
        vec![Intersection::default(); EDGE_CAPACITY],
        vec![0.0; WIDTH as usize],
    );
    group.bench_function(BenchmarkId::new("sampled", "64_rectangles"), |b| b.iter(|| {
        pixels.fill(0);
        let mut target = PixmapMut::new(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap();
        render_solid(&path, Affine::identity(), RGBA::new(40, 120, 220, 192),
            RenderOptions::default(), &mut target, &mut RenderWorkspace {
                edges: &mut edges, intersections: &mut intersections,
                row_coverage: &mut row_coverage,
            },
        ).unwrap();
        black_box(&pixels);
    }));

    let mut analytic_intersections = vec![AnalyticIntersection::default(); EDGE_CAPACITY];
    let (mut analytic_offsets, mut analytic_indices) =
        (vec![0; HEIGHT as usize + 1], vec![0; EDGE_CAPACITY]);
    group.bench_function(BenchmarkId::new("analytic", "64_rectangles"), |b| b.iter(|| {
        pixels.fill(0);
        let mut target = PixmapMut::new(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap();
        render_solid_analytic(&path, Affine::identity(), RGBA::new(40, 120, 220, 192),
            AnalyticRenderOptions::default(), &mut target, &mut AnalyticRenderWorkspace {
                edges: &mut edges, intersections: &mut analytic_intersections,
                row_coverage: &mut row_coverage,
                row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
            },
        ).unwrap();
        black_box(&pixels);
    }));
    group.finish();
}

fn benchmark_analytic_active(c: &mut Criterion) {
    let stable = (0..64).flat_map(|index| {
        let x = index as f32 * 3.75 + 4.0;
        [Edge { upper: (x, 0.25).into(), lower: (x, 255.75).into(), winding: -1 },
         Edge { upper: (x + 2.0, 0.25).into(),
                lower: (x + 2.0, 255.75).into(), winding: 1 }]
    }).collect::<Vec<_>>();
    let churn = (0..256).flat_map(|index| {
        let (column, row) = (index % 16, index / 16);
        let (x, y) = (column as f32 * 16.0 + 2.25, row as f32 * 16.0 + 2.5);
        [Edge { upper: (x, y).into(), lower: (x, y + 3.25).into(), winding: -1 },
         Edge { upper: (x + 8.5, y).into(),
                lower: (x + 8.5, y + 3.25).into(), winding: 1 }]
    }).collect::<Vec<_>>();
    let crossing = (0..32).map(|index| {
        let top = index as f32 * 7.5 + 8.0;
        let bottom = (31 - index) as f32 * 7.5 + 8.0;
        Edge { upper: (top, 0.25).into(), lower: (bottom, 255.75).into(),
               winding: if index & 1 == 0 { -1 } else { 1 } }
    }).collect::<Vec<_>>();
    let mut group = c.benchmark_group("analytic_active");
    group.throughput(Throughput::Elements(WIDTH as u64 * HEIGHT as u64));
    for (name, edges) in [("stable_128", stable), ("churn_512", churn),
                          ("crossing_32", crossing)] {
        let requirements = analytic_bin_requirements(&edges, HEIGHT).unwrap();
        let (mut offsets, mut indices) =
            (vec![0; requirements.offsets], vec![0; requirements.indices]);
        let bins = build_analytic_row_bins(&edges, HEIGHT, AnalyticBinWorkspace {
            row_offsets: &mut offsets, edge_indices: &mut indices,
        }).unwrap();
        let (mut active, mut row) =
            (vec![AnalyticIntersection::default(); edges.len()], vec![0.0; WIDTH as usize]);
        group.bench_function(name, |b| b.iter(|| {
            let mut sink = RunCounter::default();
            rasterize_edges_analytic_binned(&edges, bins, WIDTH, HEIGHT, FillRule::NonZero,
                &mut AnalyticWorkspace {
                    intersections: &mut active, row_coverage: &mut row,
                }, &mut sink,
            ).unwrap();
            black_box((sink.runs, sink.pixels));
        }));
    }
    group.finish();
}

fn stroke_polyline_scene() -> Path {
    let mut path = PathBuilder::with_capacity(33);
    path.move_to((8.0, 128.0));
    for index in 1..=32 {
        let x = 8.0 + index as f32 * 7.5;
        let y = if index & 1 == 0 { 48.25 } else { 207.75 };
        path.line_to((x, y));
    }   path.build()
}

fn stroke_curve_scene() -> Path {
    let mut path = PathBuilder::with_capacity(9);
    path.move_to((8.0, 128.0));
    for index in 0..8 {
        let x = 8.0 + index as f32 * 30.0;
        let high = if index & 1 == 0 { 24.0 } else { 232.0 };
        let low  = if index & 1 == 0 { 232.0 } else { 24.0 };
        path.cubic_to((x + 10.0, high), (x + 20.0, low), (x + 30.0, 128.0));
    }   path.build()
}

fn stroke_requirements(path: &Path, options: AnalyticStrokeOptions) ->
    (usize, usize, usize) {
    let (mut points, mut contours) =
        (vec![Default::default(); 1024], vec![StrokeContour::default(); 16]);
    let mut workspace = StrokePathWorkspace {
        points: &mut points, contours: &mut contours,
    };
    let flattened = flatten_stroke_path(path, Affine::identity(), options.flatten,
        &mut workspace).unwrap();
    let (mut point_count, mut contour_count, mut edge_count) = (0, 0, 0);
    for (points, closed) in flattened.contours() {
        point_count += points.len();  contour_count += 1;
        stroke_polyline(points, closed, options.stroke, &mut |_| {
            edge_count += 1;  Ok::<_, core::convert::Infallible>(())
        }).unwrap();
    }
    (point_count, contour_count, edge_count)
}

fn benchmark_stroke(c: &mut Criterion) {
    let base = StrokeOptions::new(6.0).unwrap();
    let scenes = [
        ("butt_miter_polyline", stroke_polyline_scene(),
            AnalyticStrokeOptions { stroke: base, ..Default::default() }),
        ("round_polyline", stroke_polyline_scene(), AnalyticStrokeOptions {
            stroke: base.with_cap(LineCap::Round).with_join(LineJoin::Round),
            ..Default::default()
        }),
        ("butt_miter_curves", stroke_curve_scene(),
            AnalyticStrokeOptions { stroke: base, ..Default::default() }),
    ];
    let mut render_group = c.benchmark_group("stroke_rgba8888");
    render_group.throughput(Throughput::Elements(WIDTH as u64 * HEIGHT as u64));
    for (name, path, options) in scenes {
        let (point_count, contour_count, edge_count) =
            stroke_requirements(&path, options);
        let scratch = format!("{point_count}p_{contour_count}c_{edge_count}e");
        let (mut points, mut contours, mut edges, mut intersections,
            mut row_coverage, mut pixels) = (
            vec![Default::default(); point_count],
            vec![StrokeContour::default(); contour_count],
            vec![Edge::default(); edge_count],
            vec![AnalyticIntersection::default(); edge_count],
            vec![0.0; WIDTH as usize],
            vec![0; WIDTH as usize * HEIGHT as usize * 4],
        );
        let (mut row_offsets, mut edge_indices) =
            (vec![0; HEIGHT as usize + 1], vec![0; edge_count]);
        render_group.bench_function(BenchmarkId::new(name, scratch), |b| b.iter(|| {
            pixels.fill(0);
            render_stroke_solid_analytic(&path, Affine::identity(),
                RGBA::new(40, 120, 220, 192), options,
                &mut PixmapMut::new(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap(),
                &mut AnalyticStrokeWorkspace {
                    points: &mut points, contours: &mut contours, edges: &mut edges,
                    intersections: &mut intersections, row_coverage: &mut row_coverage,
                    row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
                }).unwrap();
            black_box(&pixels);
        }));
    }
    render_group.finish();

    let mut expand_group = c.benchmark_group("stroke_expand");
    let scenes = [
        ("butt_miter_polyline", stroke_polyline_scene(),
            AnalyticStrokeOptions { stroke: base, ..Default::default() }),
        ("round_polyline", stroke_polyline_scene(), AnalyticStrokeOptions {
            stroke: base.with_cap(LineCap::Round).with_join(LineJoin::Round),
            ..Default::default()
        }),
        ("butt_miter_curves", stroke_curve_scene(),
            AnalyticStrokeOptions { stroke: base, ..Default::default() }),
    ];
    for (name, path, options) in scenes {
        let (point_count, contour_count, edge_count) =
            stroke_requirements(&path, options);
        let scratch = format!("{point_count}p_{contour_count}c_{edge_count}e");
        let (mut points, mut contours) = (
            vec![Default::default(); point_count],
            vec![StrokeContour::default(); contour_count],
        );
        expand_group.throughput(Throughput::Elements(edge_count as _));
        expand_group.bench_function(BenchmarkId::new(name, scratch), |b| b.iter(|| {
            let mut workspace = StrokePathWorkspace {
                points: &mut points, contours: &mut contours,
            };
            let flattened = flatten_stroke_path(&path, Affine::identity(), options.flatten,
                &mut workspace).unwrap();
            let mut emitted = 0;
            for (points, closed) in flattened.contours() {
                stroke_polyline(points, closed, options.stroke, &mut |_| {
                    emitted += 1;  Ok::<_, core::convert::Infallible>(())
                }).unwrap();
            }
            black_box(emitted);
        }));
    }
    expand_group.finish();
}

fn sample_checksum(sampler: &impl PaintSampler) -> u64 {
    let mut checksum = 0_u64;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let color = sampler.sample(x as f32 + 0.5, y as f32 + 0.5);
            checksum = color.to_array().into_iter().fold(checksum,
                |checksum, channel| checksum.wrapping_mul(257).wrapping_add(channel as _));
        }
    }       checksum
}

fn benchmark_paint(c: &mut Criterion) {
    let stops = [GradientStop::new( 0.0, RGBA::new(240, 20, 80,  32)),
                 GradientStop::new(0.35, RGBA::new(10, 220, 40, 160)),
                 GradientStop::new( 1.0, RGBA::new(30, 60, 250, 224)) ];
    let stops = GradientStops::new(&stops).unwrap();
    let solid = SolidPaint::new(RGBA::new(40, 120, 220, 192));
    let linear = LinearGradient::new((0.0, 0.0), (WIDTH as _, HEIGHT as _),
        stops, SpreadMode::Pad).unwrap();
    let radial = RadialGradient::two_circle((96.0, 112.0), 8.0, (128.0, 128.0), 180.0,
        stops, SpreadMode::Pad).unwrap();
    let conic = ConicGradient::new((128.0, 128.0), 0.37, stops).unwrap();
    let mut group = c.benchmark_group("paint_sample_rgba8888");
    group.throughput(Throughput::Elements((WIDTH as u64) * HEIGHT as u64));
    group.bench_function("solid",  |b| b.iter(|| black_box(sample_checksum(&solid))));
    group.bench_function("linear", |b| b.iter(|| black_box(sample_checksum(&linear))));
    group.bench_function("radial", |b| b.iter(|| black_box(sample_checksum(&radial))));
    group.bench_function("conic",  |b| b.iter(|| black_box(sample_checksum(&conic))));
    group.finish();
}

#[cfg(feature = "fixed")] fn benchmark_fixed(c: &mut Criterion) {
    use ugl_rs::{geometry::FixedScalar,
        canvas::{composite_solid_fixed_tiles, render_solid_fixed, render_solid_fixed_tiled},
        raster_fixed::{FixedCoverageRun, FixedCoverageStrip, FixedCoverageWorkspace,
            FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid,
            FIXED_STRIP_HEIGHT, prepare_lines, rasterize_lines, rasterize_lines_to_strips,
        },
        tile_fixed::{FixedCoverageTile, FixedCoverageTilePiece, FixedCoverageTileRun,
            FixedCoverageTileWorkspace, FixedDirectTilePiece, FixedDirectTileWorkspace,
            encode_fixed_coverage_tiles, fixed_tile_requirements, rasterize_lines_to_tiles,
        },
    };

    let mut group = c.benchmark_group("raster_rgba8888");
    group.throughput(Throughput::Elements((WIDTH as u64) * HEIGHT as u64));
    let scenes = [
        ("64_rectangles", (0..64).map(|index| [
            (index % 8) as f32 * 30.0 + 4.25,
            (index / 8) as f32 * 30.0 + 4.5, 22.5, 21.75,
        ]).collect::<Vec<_>>()),
        ("sparse_16", (0..16).map(|index| [
            (index % 4) as f32 * 64.0 + 8.25,
            (index / 4) as f32 * 64.0 + 8.5, 4.5, 4.25,
        ]).collect()),
        ("short_edges_256", (0..256).map(|index| [
            (index % 16) as f32 * 16.0 + 2.25,
            (index / 16) as f32 * 16.0 + 2.5, 6.5, 4.25,
        ]).collect()),
        ("full_tiles_16", (0..16).map(|index| [
            (index % 4) as f32 * 64.0,
            (index / 4) as f32 * 64.0, 32.0, 32.0,
        ]).collect()),
    ];
    for (name, rectangles) in scenes {
        let mut source_edges = Vec::with_capacity(rectangles.len() * 2);
        for [x, y, width, height] in rectangles {
            let (left, right, top, bottom) = (
                FixedScalar::from_num(x), FixedScalar::from_num(x + width),
                FixedScalar::from_num(y), FixedScalar::from_num(y + height),
            );
            source_edges.extend([
                Edge { upper:  (left, top).into(), lower:  (left, bottom).into(), winding: -1 },
                Edge { upper: (right, top).into(), lower: (right, bottom).into(), winding: 1 },
            ]);
        }
        let mut lines = vec![FixedLine::default(); source_edges.len()];
        let line_count = prepare_lines(&source_edges, &mut lines).unwrap();
        let requirements =
            ugl_rs::raster_fixed::fixed_strip_requirements(&lines[..line_count], HEIGHT).unwrap();
        let tile_requirements = fixed_tile_requirements(WIDTH, HEIGHT).unwrap();
        let (mut segments, mut trapezoids, mut row_area, mut pixels,
            mut strip_offsets, mut strip_indices, mut coverage_strips, mut coverage_runs,
            mut coverage_tiles, mut coverage_tile_runs, mut coverage_tile_pieces,
            mut direct_tile_pieces, mut tile_heads, mut tile_tails, mut touched_tiles) = (
            vec![FixedSegment::default(); line_count],
            vec![FixedTrapezoid::default(); line_count.div_ceil(2)],
            vec![0; WIDTH as usize], vec![0; WIDTH as usize * HEIGHT as usize * 4],
            vec![0; requirements.offsets], vec![0; requirements.indices],
            vec![FixedCoverageStrip::default();
                HEIGHT.div_ceil(FIXED_STRIP_HEIGHT) as usize],
            vec![FixedCoverageRun::default(); WIDTH as usize * HEIGHT as usize],
            vec![FixedCoverageTile::default(); tile_requirements.tiles],
            vec![FixedCoverageTileRun::default(); tile_requirements.runs],
            vec![FixedCoverageTilePiece::default(); WIDTH as usize * HEIGHT as usize],
            vec![FixedDirectTilePiece::default(); tile_requirements.pieces],
            vec![0; tile_requirements.columns],
            vec![0; tile_requirements.columns],
            vec![0; tile_requirements.columns],
        );
        let (mut cached_tiles, mut cached_runs) = (
            vec![FixedCoverageTile::default(); coverage_tiles.len()],
            vec![FixedCoverageTileRun::default(); coverage_tile_runs.len()],
        );
        let cached = rasterize_lines_to_tiles(&lines[..line_count], WIDTH, HEIGHT,
            FillRule::NonZero, &mut FixedRasterWorkspace {
                segments: &mut segments, trapezoids: &mut trapezoids,
                row_area: &mut row_area,
                strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
            }, FixedDirectTileWorkspace {
                tiles: &mut cached_tiles, runs: &mut cached_runs,
                pieces: &mut direct_tile_pieces,
                column_heads: &mut tile_heads, column_tails: &mut tile_tails,
                touched_columns: &mut touched_tiles,
            }).unwrap();
        group.bench_function(BenchmarkId::new("fixed", name), |b| b.iter(|| {
            pixels.fill(0);
            let mut target = PixmapMut::new(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap();
            render_solid_fixed(&lines[..line_count], RGBA::new(40, 120, 220, 192),
                FillRule::NonZero, &mut target, &mut FixedRasterWorkspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                },
            ).unwrap();
            black_box(&pixels);
        }));
        group.bench_function(BenchmarkId::new("fixed_tiled", name), |b| b.iter(|| {
            pixels.fill(0);
            let mut target = PixmapMut::new(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap();
            render_solid_fixed_tiled(&lines[..line_count], RGBA::new(40, 120, 220, 192),
                FillRule::NonZero, &mut target, &mut FixedRasterWorkspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, FixedDirectTileWorkspace {
                    tiles: &mut coverage_tiles, runs: &mut coverage_tile_runs,
                    pieces: &mut direct_tile_pieces,
                    column_heads: &mut tile_heads, column_tails: &mut tile_tails,
                    touched_columns: &mut touched_tiles,
                },
            ).unwrap();
            black_box(&pixels);
        }));
        group.bench_function(BenchmarkId::new("fixed_tiled_cached", name), |b| b.iter(|| {
            pixels.fill(0);
            composite_solid_fixed_tiles(cached, RGBA::new(40, 120, 220, 192),
                &mut PixmapMut::new(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap()).unwrap();
            black_box(&pixels);
        }));
        group.bench_function(BenchmarkId::new("fixed_stream", name), |b| b.iter(|| {
            let mut sink = RunCounter::default();
            rasterize_lines(&lines[..line_count], WIDTH, HEIGHT, FillRule::NonZero,
                &mut FixedRasterWorkspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, &mut sink,
            ).unwrap();
            black_box((sink.runs, sink.pixels));
        }));
        group.bench_function(BenchmarkId::new("fixed_strip_encode", name), |b| b.iter(|| {
            let retained = rasterize_lines_to_strips(&lines[..line_count], WIDTH, HEIGHT,
                FillRule::NonZero, &mut FixedRasterWorkspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, FixedCoverageWorkspace {
                    strips: &mut coverage_strips, runs: &mut coverage_runs,
                },
            ).unwrap();
            black_box(retained.strips());
            black_box(retained.runs());
        }));
        group.bench_function(BenchmarkId::new("fixed_strip_replay", name), |b| b.iter(|| {
            let retained = rasterize_lines_to_strips(&lines[..line_count], WIDTH, HEIGHT,
                FillRule::NonZero, &mut FixedRasterWorkspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, FixedCoverageWorkspace {
                    strips: &mut coverage_strips, runs: &mut coverage_runs,
                },
            ).unwrap();
            let mut sink = RunCounter::default();  retained.replay(&mut sink).unwrap();
            black_box((sink.runs, sink.pixels));
        }));
        group.bench_function(BenchmarkId::new("fixed_tile_encode", name), |b| b.iter(|| {
            let retained = rasterize_lines_to_strips(&lines[..line_count], WIDTH, HEIGHT,
                FillRule::NonZero, &mut FixedRasterWorkspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, FixedCoverageWorkspace {
                    strips: &mut coverage_strips, runs: &mut coverage_runs,
                },
            ).unwrap();
            let tiled = encode_fixed_coverage_tiles(retained, FixedCoverageTileWorkspace {
                tiles: &mut coverage_tiles, runs: &mut coverage_tile_runs,
                pieces: &mut coverage_tile_pieces,
            }).unwrap();
            black_box(tiled.tiles());
            black_box(tiled.runs());
        }));
        group.bench_function(BenchmarkId::new("fixed_tile_replay", name), |b| b.iter(|| {
            let retained = rasterize_lines_to_strips(&lines[..line_count], WIDTH, HEIGHT,
                FillRule::NonZero, &mut FixedRasterWorkspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, FixedCoverageWorkspace {
                    strips: &mut coverage_strips, runs: &mut coverage_runs,
                },
            ).unwrap();
            let tiled = encode_fixed_coverage_tiles(retained, FixedCoverageTileWorkspace {
                tiles: &mut coverage_tiles, runs: &mut coverage_tile_runs,
                pieces: &mut coverage_tile_pieces,
            }).unwrap();
            let mut sink = RunCounter::default();  tiled.replay(&mut sink).unwrap();
            black_box((sink.runs, sink.pixels));
        }));
        group.bench_function(BenchmarkId::new("fixed_tile_direct", name), |b| b.iter(|| {
            let tiled = rasterize_lines_to_tiles(&lines[..line_count], WIDTH, HEIGHT,
                FillRule::NonZero, &mut FixedRasterWorkspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, FixedDirectTileWorkspace {
                    tiles: &mut coverage_tiles, runs: &mut coverage_tile_runs,
                    pieces: &mut direct_tile_pieces,
                    column_heads: &mut tile_heads, column_tails: &mut tile_tails,
                    touched_columns: &mut touched_tiles,
                },
            ).unwrap();
            black_box(tiled.tiles());
            black_box(tiled.runs());
        }));
        group.bench_function(BenchmarkId::new("fixed_tile_direct_replay", name),
            |b| b.iter(|| {
            let tiled = rasterize_lines_to_tiles(&lines[..line_count], WIDTH, HEIGHT,
                FillRule::NonZero, &mut FixedRasterWorkspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, FixedDirectTileWorkspace {
                    tiles: &mut coverage_tiles, runs: &mut coverage_tile_runs,
                    pieces: &mut direct_tile_pieces,
                    column_heads: &mut tile_heads, column_tails: &mut tile_tails,
                    touched_columns: &mut touched_tiles,
                },
            ).unwrap();
            let mut sink = RunCounter::default();  tiled.replay(&mut sink).unwrap();
            black_box((sink.runs, sink.pixels));
        }));
    }
    group.finish();
}

fn  benchmarks(c: &mut Criterion) {
    #[cfg(feature = "fixed")] benchmark_fixed(c);
    benchmark_f32(c);
    benchmark_analytic_active(c);
    benchmark_stroke(c);
    benchmark_paint(c);
}

criterion_group!(raster, benchmarks);
criterion_main!(raster);
