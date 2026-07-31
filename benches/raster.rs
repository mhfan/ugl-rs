
use std::hint::black_box;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ugl_rs::{analytic::{AnalyticBinWorkspace, AnalyticIntersection, AnalyticWorkspace,
        analytic_bin_requirements, build_analytic_row_bins, rasterize_edges_analytic_binned},
    color::{EncodedPremulSRGBA8, LinearPremulRGBA, Srgb8Encoder,
        SRGB8_ENCODE_LUT_SIZE, SRGBA, RGBA},
    dash::{dash_polyline, DashContour, DashPattern, DashWorkspace},
    edge::{Edge, build_fill_edges}, flatten::FlattenOptions,
    raster::{CoverageSink, FillRule, Intersection},
    canvas::{AnalyticRenderOptions, AnalyticRenderWorkspace, AnalyticStrokeOptions,
        AnalyticStrokeWorkspace, PixmapMut, RenderOptions, RenderWorkspace,
        render_solid, render_solid_analytic, render_stroke_solid_analytic,
    }, canvas_linear::{LinearPixmapMut,
        render_paint_analytic as render_paint_linear_analytic,
        render_solid_analytic as render_solid_linear_analytic},
    geometry::{Affine, Path, PathBuilder, Point},
    sampler::{ConicAngleMode, ConicGradient, GradientStop, GradientStops, LinearGradient,
        LinearPaintSampler, PaintSampler, RadialGradient, SolidPaint, SpreadMode},
    stroke::{LineCap, LineJoin, StrokeContour, StrokeOptions, StrokePathWorkspace,
        flatten_stroke_path, stroke_polyline},
};
#[derive(Default)] struct RunCounter { runs: u32, pixels: u32 }
#[derive(Default)] struct SpanStatistics {
    runs: u32, pixels: u32, full_runs: u32, full_pixels: u32,
    maximum_len: u32, length_buckets: [u32; 6],
}
struct PointLinearSampler<'a, S>(&'a S);
struct CompositeLinearSampler<'a, S>(&'a S);

impl<S: LinearPaintSampler> LinearPaintSampler for PointLinearSampler<'_, S> {
    fn sample_linear(&self, x: f32, y: f32) -> LinearPremulRGBA<f32> {
        self.0.sample_linear(x, y)
    }
}

impl<S: LinearPaintSampler> LinearPaintSampler for CompositeLinearSampler<'_, S> {
    fn sample_linear(&self, x: f32, y: f32) -> LinearPremulRGBA<f32> {
        self.0.sample_linear(x, y)
    }
    fn sample_linear_span(&self, x: f32, y: f32, dx: f32, dy: f32, len: u32,
        emit: impl FnMut(LinearPremulRGBA<f32>)) {
        self.0.sample_linear_span(x, y, dx, dy, len, emit)
    }
}

impl CoverageSink for RunCounter {
    type Error = core::convert::Infallible;
    fn span(&mut self, _x: u32, _y: u32, len: u32, _coverage: u8) ->
        Result<(), Self::Error> {
        self.runs += 1;  self.pixels += len;  Ok(())
    }
}

impl CoverageSink for SpanStatistics {
    type Error = core::convert::Infallible;
    fn span(&mut self, _x: u32, _y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        self.runs += 1;  self.pixels += len;
        if coverage == u8::MAX {
            self.full_runs += 1;
            self.full_pixels += len;
        }
        self.maximum_len = self.maximum_len.max(len);
        self.length_buckets[match len {
            0..=1 => 0, 2..=3 => 1, 4..=7 => 2,
            8..=15 => 3, 16..=31 => 4, _ => 5,
        }] += 1;
        Ok(())
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

fn report_span_statistics(path: &Path) {
    if std::env::var_os("UGL_SPAN_STATS").is_none() { return; }
    let mut edges = Vec::with_capacity(EDGE_CAPACITY);
    build_fill_edges(path, Affine::identity(), FlattenOptions::default(),
        &mut |edge| {
            edges.push(edge);
            Ok::<_, core::convert::Infallible>(())
        }).unwrap();
    let requirements = analytic_bin_requirements(&edges, HEIGHT).unwrap();
    let (mut offsets, mut indices) =
        (vec![0; requirements.offsets], vec![0; requirements.indices]);
    let bins = build_analytic_row_bins(&edges, HEIGHT, AnalyticBinWorkspace {
        row_offsets: &mut offsets, edge_indices: &mut indices,
    }).unwrap();
    let (mut active, mut row, mut stats) = (
        vec![AnalyticIntersection::default(); edges.len()],
        vec![0.0; WIDTH as usize], SpanStatistics::default(),
    );
    rasterize_edges_analytic_binned(&edges, bins, WIDTH, HEIGHT, FillRule::NonZero,
        &mut AnalyticWorkspace {
            intersections: &mut active, row_coverage: &mut row,
        }, &mut stats).unwrap();
    let mean = stats.pixels as f64 / stats.runs as f64;
    eprintln!("span statistics: runs={}, pixels={}, mean_len={mean:.2}, max_len={}, \
        full_runs={} ({:.1}%), full_pixels={} ({:.1}%), \
        len[1,2-3,4-7,8-15,16-31,32+]={:?}",
        stats.runs, stats.pixels, stats.maximum_len,
        stats.full_runs, stats.full_runs as f64 * 100.0 / stats.runs as f64,
        stats.full_pixels, stats.full_pixels as f64 * 100.0 / stats.pixels as f64,
        stats.length_buckets);
}

fn benchmark_f32(c: &mut Criterion) {
    let path = rectangle_scene();
    report_span_statistics(&path);
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

    let mut linear_pixels =
        vec![LinearPremulRGBA::default(); WIDTH as usize * HEIGHT as usize];
    group.bench_function(BenchmarkId::new("analytic_linear_working", "64_rectangles"),
        |b| b.iter(|| {
            linear_pixels.fill(LinearPremulRGBA::default());
            let mut target =
                LinearPixmapMut::new(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
            render_solid_linear_analytic(&path, Affine::identity(),
                SRGBA::new(40, 120, 220, 192), AnalyticRenderOptions::default(),
                &mut target, &mut AnalyticRenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    row_coverage: &mut row_coverage,
                    row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
                }).unwrap();
            black_box(&linear_pixels);
        }));
    let gradient_stop_values = [GradientStop::new(0.0, RGBA::new(240, 20, 80, 32)),
                                GradientStop::new(1.0, RGBA::new(30, 60, 250, 224))];
    let mut gradient_ramp = vec![LinearPremulRGBA::default(); 1024];
    let gradient_stops =
        GradientStops::with_linear_ramp(&gradient_stop_values, &mut gradient_ramp).unwrap();
    let gradient = LinearGradient::new((0.0, 0.0), (WIDTH as _, HEIGHT as _),
        gradient_stops, SpreadMode::Pad).unwrap();
    group.bench_function(BenchmarkId::new("analytic_linear_gradient_point", "64_rectangles"),
        |b| b.iter(|| {
            linear_pixels.fill(LinearPremulRGBA::default());
            let mut target =
                LinearPixmapMut::new(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
            render_paint_linear_analytic(&path, Affine::identity(),
                &PointLinearSampler(&gradient), AnalyticRenderOptions::default(),
                &mut target, &mut AnalyticRenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    row_coverage: &mut row_coverage,
                    row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
                }).unwrap();
            black_box(&linear_pixels);
        }));
    let radial = RadialGradient::new((WIDTH as f32 * 0.5, HEIGHT as f32 * 0.5),
        WIDTH as f32 * 0.7, gradient_stops, SpreadMode::Pad).unwrap();
    group.bench_function(BenchmarkId::new(
        "analytic_linear_radial_concentric_point", "64_rectangles"), |b| b.iter(|| {
        linear_pixels.fill(LinearPremulRGBA::default());
        let mut target =
            LinearPixmapMut::new(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
        render_paint_linear_analytic(&path, Affine::identity(), &PointLinearSampler(&radial),
            AnalyticRenderOptions::default(), &mut target, &mut AnalyticRenderWorkspace {
                edges: &mut edges, intersections: &mut analytic_intersections,
                row_coverage: &mut row_coverage,
                row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
            }).unwrap();
        black_box(&linear_pixels);
    }));
    let conic = ConicGradient::new(
        (WIDTH as f32 * 0.5, HEIGHT as f32 * 0.5), 0.37, gradient_stops).unwrap();
    let conic_fast = ConicGradient::with_angle_mode(
        (WIDTH as f32 * 0.5, HEIGHT as f32 * 0.5), 0.37,
        gradient_stops, ConicAngleMode::Fast).unwrap();
    for (name, paint) in [("analytic_linear_conic", &conic),
                          ("analytic_linear_conic_fast", &conic_fast)] {
        group.bench_function(BenchmarkId::new(name, "64_rectangles"), |b| b.iter(|| {
            linear_pixels.fill(LinearPremulRGBA::default());
            let mut target =
                LinearPixmapMut::new(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
            render_paint_linear_analytic(&path, Affine::identity(), paint,
                AnalyticRenderOptions::default(), &mut target,
                &mut AnalyticRenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    row_coverage: &mut row_coverage,
                    row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
                }).unwrap();
            black_box(&linear_pixels);
        }));
    }
    group.bench_function(BenchmarkId::new(
        "analytic_linear_radial_concentric", "64_rectangles"), |b| b.iter(|| {
        linear_pixels.fill(LinearPremulRGBA::default());
        let mut target =
            LinearPixmapMut::new(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
        render_paint_linear_analytic(&path, Affine::identity(), &radial,
            AnalyticRenderOptions::default(), &mut target, &mut AnalyticRenderWorkspace {
                edges: &mut edges, intersections: &mut analytic_intersections,
                row_coverage: &mut row_coverage,
                row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
            }).unwrap();
        black_box(&linear_pixels);
    }));
    group.bench_function(BenchmarkId::new("analytic_linear_gradient", "64_rectangles"),
        |b| b.iter(|| {
            linear_pixels.fill(LinearPremulRGBA::default());
            let mut target =
                LinearPixmapMut::new(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
            render_paint_linear_analytic(&path, Affine::identity(), &gradient,
                AnalyticRenderOptions::default(), &mut target,
                &mut AnalyticRenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    row_coverage: &mut row_coverage,
                    row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
                }).unwrap();
            black_box(&linear_pixels);
        }));
    let opaque_stop_values = [GradientStop::new(0.0, RGBA::new(240, 20, 80, 255)),
                              GradientStop::new(1.0, RGBA::new(30, 60, 250, 255))];
    let mut opaque_ramp = vec![LinearPremulRGBA::default(); 1024];
    let opaque_gradient = LinearGradient::new((0.0, 0.0), (WIDTH as _, HEIGHT as _),
        GradientStops::with_linear_ramp(&opaque_stop_values, &mut opaque_ramp).unwrap(),
        SpreadMode::Pad).unwrap();
    group.bench_function(BenchmarkId::new(
        "analytic_linear_gradient_opaque_composite", "64_rectangles"), |b| b.iter(|| {
        linear_pixels.fill(LinearPremulRGBA::default());
        let mut target =
            LinearPixmapMut::new(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
        render_paint_linear_analytic(&path, Affine::identity(),
            &CompositeLinearSampler(&opaque_gradient), AnalyticRenderOptions::default(),
            &mut target, &mut AnalyticRenderWorkspace {
                edges: &mut edges, intersections: &mut analytic_intersections,
                row_coverage: &mut row_coverage,
                row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
            }).unwrap();
        black_box(&linear_pixels);
    }));
    group.bench_function(BenchmarkId::new(
        "analytic_linear_gradient_opaque", "64_rectangles"), |b| b.iter(|| {
        linear_pixels.fill(LinearPremulRGBA::default());
        let mut target =
            LinearPixmapMut::new(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
        render_paint_linear_analytic(&path, Affine::identity(), &opaque_gradient,
            AnalyticRenderOptions::default(), &mut target, &mut AnalyticRenderWorkspace {
                edges: &mut edges, intersections: &mut analytic_intersections,
                row_coverage: &mut row_coverage,
                row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
            }).unwrap();
        black_box(&linear_pixels);
    }));
    group.bench_function(BenchmarkId::new("analytic_linear_present_exact", "64_rectangles"),
        |b| b.iter(|| {
            linear_pixels.fill(LinearPremulRGBA::default());
            let mut target =
                LinearPixmapMut::new(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
            render_solid_linear_analytic(&path, Affine::identity(),
                SRGBA::new(40, 120, 220, 192), AnalyticRenderOptions::default(),
                &mut target, &mut AnalyticRenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    row_coverage: &mut row_coverage,
                    row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
                }).unwrap();
            target.encode_into(
                &mut PixmapMut::new(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap()).unwrap();
            black_box(&pixels);
        }));
    let mut transfer_lut = vec![0; SRGB8_ENCODE_LUT_SIZE];
    let encoder = Srgb8Encoder::new(&mut transfer_lut).unwrap();
    let mut dirty_tiles =
        vec![0; LinearPixmapMut::dirty_tile_words(WIDTH, HEIGHT).unwrap()];
    group.bench_function(BenchmarkId::new("analytic_linear_present_lut", "64_rectangles"),
        |b| b.iter(|| {
            linear_pixels.fill(LinearPremulRGBA::default());
            let mut target =
                LinearPixmapMut::new(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
            render_solid_linear_analytic(&path, Affine::identity(),
                SRGBA::new(40, 120, 220, 192), AnalyticRenderOptions::default(),
                &mut target, &mut AnalyticRenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    row_coverage: &mut row_coverage,
                    row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
                }).unwrap();
            target.encode_into_with(
                &mut PixmapMut::new(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap(),
                encoder).unwrap();
            black_box(&pixels);
        }));
    group.bench_function(
        BenchmarkId::new("analytic_linear_present_dirty_lut", "64_rectangles"),
        |b| b.iter(|| {
            linear_pixels.fill(LinearPremulRGBA::default());
            let mut target = LinearPixmapMut::with_dirty_tiles(
                &mut linear_pixels, WIDTH, HEIGHT, WIDTH, &mut dirty_tiles).unwrap();
            render_solid_linear_analytic(&path, Affine::identity(),
                SRGBA::new(40, 120, 220, 192), AnalyticRenderOptions::default(),
                &mut target, &mut AnalyticRenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    row_coverage: &mut row_coverage,
                    row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
                }).unwrap();
            target.encode_dirty_into_with(
                &mut PixmapMut::new(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap(),
                encoder).unwrap();
            black_box(&pixels);
        }));
    group.finish();
}

fn benchmark_linear_presentation(c: &mut Criterion) {
    let mut builder = PathBuilder::new();
    builder.move_to((4.25, 4.5)).line_to((26.75, 4.5))
        .line_to((26.75, 26.25)).line_to((4.25, 26.25));
    let path = builder.build();
    let mut linear_pixels =
        vec![LinearPremulRGBA::default(); WIDTH as usize * HEIGHT as usize];
    let mut encoded_pixels = vec![0; WIDTH as usize * HEIGHT as usize * 4];
    let mut transfer_lut = vec![0; SRGB8_ENCODE_LUT_SIZE];
    let encoder = Srgb8Encoder::new(&mut transfer_lut).unwrap();
    let mut dirty_tiles =
        vec![0; LinearPixmapMut::dirty_tile_words(WIDTH, HEIGHT).unwrap()];
    let (mut edges, mut intersections, mut row_coverage,
        mut row_offsets, mut edge_indices) = (
        vec![Edge::default(); 4], vec![AnalyticIntersection::default(); 4],
        vec![0.0; WIDTH as usize], vec![0; HEIGHT as usize + 1], vec![0; 4],
    );
    let mut group = c.benchmark_group("linear_present_rgba8888");
    group.throughput(Throughput::Elements((WIDTH as u64) * HEIGHT as u64));

    group.bench_function("sparse_full_frame_lut", |b| b.iter(|| {
        let mut target =
            LinearPixmapMut::new(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
        render_solid_linear_analytic(&path, Affine::identity(), SRGBA::white(),
            AnalyticRenderOptions::default(), &mut target, &mut AnalyticRenderWorkspace {
                edges: &mut edges, intersections: &mut intersections,
                row_coverage: &mut row_coverage,
                row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
            }).unwrap();
        target.encode_into_with(
            &mut PixmapMut::new(&mut encoded_pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap(),
            encoder).unwrap();
        black_box(&encoded_pixels);
    }));
    group.bench_function("sparse_dirty_tiles_lut", |b| b.iter(|| {
        let mut target = LinearPixmapMut::with_dirty_tiles(
            &mut linear_pixels, WIDTH, HEIGHT, WIDTH, &mut dirty_tiles).unwrap();
        render_solid_linear_analytic(&path, Affine::identity(), SRGBA::white(),
            AnalyticRenderOptions::default(), &mut target, &mut AnalyticRenderWorkspace {
                edges: &mut edges, intersections: &mut intersections,
                row_coverage: &mut row_coverage,
                row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
            }).unwrap();
        target.encode_dirty_into_with(
            &mut PixmapMut::new(&mut encoded_pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap(),
            encoder).unwrap();
        black_box(&encoded_pixels);
    }));
    group.finish();
}

fn benchmark_analytic_active(c: &mut Criterion) {
    let stable_edges = |count: usize| (0..count / 2).flat_map(|index| {
        let step = 248.0 / (count / 2) as f32;
        let x = index as f32 * step + 4.0;
        [Edge { upper: (x, 0.25).into(), lower: (x, 255.75).into(), winding: -1 },
         Edge { upper: (x + step * 0.5, 0.25).into(),
                lower: (x + step * 0.5, 255.75).into(), winding: 1 }]
    }).collect::<Vec<_>>();
    let mut scrambled = stable_edges(256);
    scrambled.reverse();
    let churn = (0..256).flat_map(|index| {
        let (column, row) = (index % 16, index / 16);
        let (x, y) = (column as f32 * 16.0 + 2.25, row as f32 * 16.0 + 2.5);
        [Edge { upper: (x, y).into(), lower: (x, y + 3.25).into(), winding: -1 },
         Edge { upper: (x + 8.5, y).into(),
                lower: (x + 8.5, y + 3.25).into(), winding: 1 }]
    }).collect::<Vec<_>>();
    let mut scrambled_churn = churn.clone();
    for row in scrambled_churn.chunks_mut(32) { row.reverse(); }
    let crossing = (0..32).map(|index| {
        let top = index as f32 * 7.5 + 8.0;
        let bottom = (31 - index) as f32 * 7.5 + 8.0;
        Edge { upper: (top, 0.25).into(), lower: (bottom, 255.75).into(),
               winding: if index & 1 == 0 { -1 } else { 1 } }
    }).collect::<Vec<_>>();
    let mut scenes = [16, 32, 64, 128, 256].into_iter()
        .map(|count| (format!("stable_{count}"), stable_edges(count)))
        .collect::<Vec<_>>();
    scenes.extend([
        ("scrambled_256".into(), scrambled), ("churn_512".into(), churn),
        ("scrambled_churn_512".into(), scrambled_churn), ("crossing_32".into(), crossing),
    ]);
    let mut group = c.benchmark_group("analytic_active");
    group.throughput(Throughput::Elements(WIDTH as u64 * HEIGHT as u64));
    for (name, edges) in scenes {
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

    let dash_points: Vec<_> = (0..64).map(|index|
        (index as f32 * 3.0 + 8.0,
         if index & 1 == 0 { 96.0 } else { 112.0 }).into()).collect();
    let pattern = DashPattern::new(&[6.0, 3.0, 1.5, 3.0], 2.0).unwrap();
    let (mut output, mut contours) =
        (vec![Point::default(); 512], vec![DashContour::default(); 256]);
    let mut dash_group = c.benchmark_group("stroke_dash");
    dash_group.throughput(Throughput::Elements(dash_points.len() as _));
    dash_group.bench_function("polyline_64", |b| b.iter(|| {
        let mut workspace = DashWorkspace {
            points: &mut output, contours: &mut contours,
        };
        let dashed = dash_polyline(&dash_points, false, pattern, &mut workspace).unwrap();
        let mut emitted = 0;
        for (points, closed) in dashed.contours() {
            stroke_polyline(points, closed, base, &mut |_| {
                emitted += 1; Ok::<_, core::convert::Infallible>(())
            }).unwrap();
        }
        black_box(emitted);
    }));
    dash_group.finish();
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

#[cfg(feature = "fixed")]
fn sample_fixed_checksum(sampler: &impl ugl_rs::sampler::FixedPaintSampler) -> u64 {
    let mut checksum = 0_u64;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let color = sampler.sample_fixed(x, y);
            checksum = color.to_array().into_iter().fold(checksum,
                |checksum, channel| checksum.wrapping_mul(257).wrapping_add(channel as _));
        }
}       checksum
}

fn sample_linear_checksum(sampler: &impl LinearPaintSampler) -> u64 {
    let mut checksum = 0_u64;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let color = sampler.sample_linear(x as f32 + 0.5, y as f32 + 0.5);
            checksum = color.to_array().into_iter().fold(checksum,
                |checksum, channel| checksum.rotate_left(7) ^ channel.to_bits() as u64);
        }
}       checksum
}

fn sample_linear_span_checksum(sampler: &impl LinearPaintSampler) -> u64 {
    let mut checksum = 0_u64;
    for y in 0..HEIGHT {
        sampler.sample_linear_span(0.5, y as f32 + 0.5, 1.0, 0.0, WIDTH,
            |color| checksum = color.to_array().into_iter().fold(checksum,
                |checksum, channel| checksum.rotate_left(7) ^ channel.to_bits() as u64));
    }       checksum
}

fn benchmark_paint(c: &mut Criterion) {
    let stop_values = [GradientStop::new( 0.0, RGBA::new(240, 20, 80,  32)),
                       GradientStop::new(0.35, RGBA::new(10, 220, 40, 160)),
                       GradientStop::new( 1.0, RGBA::new(30, 60, 250, 224)) ];
    let mut ramp = vec![EncodedPremulSRGBA8::zeroed(); 1024];
    let stops = GradientStops::with_ramp(&stop_values, &mut ramp).unwrap();
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

    let exact_stops = GradientStops::new(&stop_values).unwrap();
    let mut linear_ramp = vec![LinearPremulRGBA::default(); 1024];
    let linear_stops =
        GradientStops::with_linear_ramp(&stop_values, &mut linear_ramp).unwrap();
    let linear = LinearGradient::new((0.0, 0.0), (WIDTH as _, HEIGHT as _),
        linear_stops, SpreadMode::Pad).unwrap();
    let radial = RadialGradient::two_circle((96.0, 112.0), 8.0,
        (128.0, 128.0), 180.0, linear_stops, SpreadMode::Pad).unwrap();
    let concentric = RadialGradient::new(
        (128.0, 128.0), 180.0, linear_stops, SpreadMode::Pad).unwrap();
    let conic = ConicGradient::new((128.0, 128.0), 0.37, linear_stops).unwrap();
    let conic_fast = ConicGradient::with_angle_mode(
        (128.0, 128.0), 0.37, linear_stops, ConicAngleMode::Fast).unwrap();
    let mut group = c.benchmark_group("paint_sample_linear");
    group.throughput(Throughput::Elements((WIDTH as u64) * HEIGHT as u64));
    group.bench_function("solid",  |b| b.iter(|| black_box(sample_linear_checksum(&solid))));
    group.bench_function("linear_point",
        |b| b.iter(|| black_box(sample_linear_checksum(&linear))));
    group.bench_function("linear_span",
        |b| b.iter(|| black_box(sample_linear_span_checksum(&linear))));
    group.bench_function("radial", |b| b.iter(|| black_box(sample_linear_checksum(&radial))));
    group.bench_function("radial_concentric_point", |b| b.iter(||
        black_box(sample_linear_checksum(&PointLinearSampler(&concentric)))));
    group.bench_function("radial_concentric_span", |b| b.iter(||
        black_box(sample_linear_span_checksum(&concentric))));
    group.bench_function("conic",  |b| b.iter(|| black_box(sample_linear_checksum(&conic))));
    group.bench_function("conic_fast",
        |b| b.iter(|| black_box(sample_linear_checksum(&conic_fast))));
    group.finish();

    let exact = LinearGradient::new((0.0, 0.0), (WIDTH as _, HEIGHT as _),
        exact_stops, SpreadMode::Pad).unwrap();
    c.bench_function("paint_sample_linear_exact/linear",
        |b| b.iter(|| black_box(sample_linear_checksum(&exact))));
}

#[cfg(feature = "fixed")] fn benchmark_fixed(c: &mut Criterion) {
    use ugl_rs::{geometry::FixedScalar,
        canvas::{composite_solid_fixed_tiles, render_solid_fixed, render_solid_fixed_tiled},
        dash::{FixedDashPattern, dash_polyline_fixed},
        flatten_fixed::{FixedFlattenOptions, flatten_path_fixed},
        raster_fixed::{FixedCoverageRun, FixedCoverageStrip, FixedCoverageWorkspace,
            FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid,
            FIXED_STRIP_HEIGHT, prepare_lines, rasterize_lines, rasterize_lines_to_strips,
        },
        tile_fixed::{FixedCoverageTile, FixedCoverageTilePiece, FixedCoverageTileRun,
            FixedCoverageTileWorkspace, FixedDirectTilePiece, FixedDirectTileWorkspace,
            encode_fixed_coverage_tiles, fixed_tile_requirements, rasterize_lines_to_tiles,
        },
        sampler::{FixedAngle, FixedConicGradient, FixedLinearGradient, FixedRadialGradient},
        stroke::{StrokeContour, StrokePathWorkspace, flatten_stroke_path_fixed},
        stroke_fixed::{FixedStrokeOptions, stroke_polyline_fixed},
    };

    let stroke_points: Vec<_> = (0..64).map(|index|
        (FixedScalar::from_num(index * 3 + 8),
         FixedScalar::from_num(if index & 1 == 0 { 96 } else { 112 })).into()).collect();
    let stroke_options = FixedStrokeOptions::new(FixedScalar::from_num(3)).unwrap()
        .with_cap(LineCap::Square).with_join(LineJoin::Miter);
    let round_stroke_options = stroke_options
        .with_cap(LineCap::Round).with_join(LineJoin::Round);
    let mut stroke_edges = Vec::with_capacity(512);
    let mut stroke_group = c.benchmark_group("stroke_expand_fixed");
    stroke_group.throughput(Throughput::Elements(stroke_points.len() as _));
    stroke_group.bench_function("square_miter_64", |b| b.iter(|| {
        stroke_edges.clear();
        stroke_polyline_fixed(&stroke_points, false, stroke_options, &mut |edge| {
            stroke_edges.push(edge); Ok::<_, core::convert::Infallible>(())
        }).unwrap();
        black_box(&stroke_edges);
    }));
    stroke_group.bench_function("round_64", |b| b.iter(|| {
        stroke_edges.clear();
        stroke_polyline_fixed(&stroke_points, false, round_stroke_options, &mut |edge| {
            stroke_edges.push(edge); Ok::<_, core::convert::Infallible>(())
        }).unwrap();
        black_box(&stroke_edges);
    }));
    stroke_group.finish();

    let fixed_dash_lengths = [FixedScalar::from_num(6), FixedScalar::from_num(3),
        FixedScalar::from_num(1.5), FixedScalar::from_num(3)];
    let fixed_dash = FixedDashPattern::new(
        &fixed_dash_lengths, FixedScalar::from_num(2)).unwrap();
    let (mut fixed_dash_points, mut fixed_dash_contours) = (
        vec![(FixedScalar::ZERO, FixedScalar::ZERO).into(); 512],
        vec![DashContour::default(); 256],
    );
    let mut dash_group = c.benchmark_group("stroke_dash_fixed");
    dash_group.throughput(Throughput::Elements(stroke_points.len() as _));
    dash_group.bench_function("polyline_64", |b| b.iter(|| {
        stroke_edges.clear();
        let mut workspace = DashWorkspace {
            points: &mut fixed_dash_points, contours: &mut fixed_dash_contours,
        };
        let dashed = dash_polyline_fixed(&stroke_points, false, fixed_dash,
            &mut workspace).unwrap();
        for (points, closed) in dashed.contours() {
            stroke_polyline_fixed(points, closed, stroke_options, &mut |edge| {
                stroke_edges.push(edge); Ok::<_, core::convert::Infallible>(())
            }).unwrap();
        }
        black_box(&stroke_edges);
    }));
    dash_group.finish();

    let mut curve_builder = PathBuilder::new();
    curve_builder.move_to((FixedScalar::from_num(8), FixedScalar::from_num(128)));
    for index in 0..8 {
        let x = index * 28 + 8;
        curve_builder.cubic_to(
            (FixedScalar::from_num(x + 7), FixedScalar::from_num(32)),
            (FixedScalar::from_num(x + 21), FixedScalar::from_num(224)),
            (FixedScalar::from_num(x + 28), FixedScalar::from_num(128)));
    }
    let curve_path = curve_builder.build();
    let (mut stroke_path_points, mut stroke_path_contours) =
        (vec![(FixedScalar::ZERO, FixedScalar::ZERO).into(); 512],
         vec![StrokeContour::default(); 16]);
    let mut stroke_path_group = c.benchmark_group("stroke_path_fixed");
    stroke_path_group.throughput(Throughput::Elements(8));
    stroke_path_group.bench_function("cubic_8", |b| b.iter(|| {
        stroke_edges.clear();
        let mut workspace = StrokePathWorkspace {
            points: &mut stroke_path_points, contours: &mut stroke_path_contours,
        };
        let flattened = flatten_stroke_path_fixed(&curve_path, Affine::identity(),
            FixedFlattenOptions::default(), &mut workspace).unwrap();
        for (points, closed) in flattened.contours() {
            stroke_polyline_fixed(points, closed, stroke_options, &mut |edge| {
                stroke_edges.push(edge); Ok::<_, core::convert::Infallible>(())
            }).unwrap();
        }
        black_box(&stroke_edges);
    }));
    stroke_path_group.finish();

    let mut flatten_group = c.benchmark_group("flatten_fixed");
    flatten_group.throughput(Throughput::Elements(8));
    flatten_group.bench_function("cubic_8", |b| b.iter(|| {
        let mut line_count = 0_u32;
        flatten_path_fixed(&curve_path, Affine::identity(),
            FixedFlattenOptions::default(), &mut |_, _| {
            line_count += 1; Ok::<_, core::convert::Infallible>(())
        }).unwrap();
        black_box(line_count);
    }));
    flatten_group.finish();

    let stop_values = [GradientStop::new( 0.0, RGBA::new(240, 20, 80,  32)),
                       GradientStop::new(0.35, RGBA::new(10, 220, 40, 160)),
                       GradientStop::new( 1.0, RGBA::new(30, 60, 250, 224)) ];
    let mut ramp = vec![EncodedPremulSRGBA8::zeroed(); 1024];
    let stops = GradientStops::with_ramp(&stop_values, &mut ramp).unwrap();
    let ramp = stops.encoded_ramp().unwrap();
    let fixed = FixedScalar::from_num;
    let linear = FixedLinearGradient::new(
        (fixed(0), fixed(0)), (fixed(WIDTH), fixed(HEIGHT)),
        ramp, SpreadMode::Pad).unwrap();
    let radial = FixedRadialGradient::new(
        (fixed(WIDTH / 2), fixed(HEIGHT / 2)), fixed(180),
        ramp, SpreadMode::Pad).unwrap();
    let focal = FixedRadialGradient::two_circle(
        (fixed(96), fixed(112)), fixed(8), (fixed(128), fixed(128)), fixed(180),
        ramp, SpreadMode::Pad).unwrap();
    let conic = FixedConicGradient::new(
        (fixed(128), fixed(128)), FixedAngle::from_bits(0x0f12_3456), ramp).unwrap();
    let mut paint_group = c.benchmark_group("paint_sample_fixed");
    paint_group.throughput(Throughput::Elements((WIDTH as u64) * HEIGHT as u64));
    paint_group.bench_function("linear",
        |b| b.iter(|| black_box(sample_fixed_checksum(&linear))));
    paint_group.bench_function("radial_concentric",
        |b| b.iter(|| black_box(sample_fixed_checksum(&radial))));
    paint_group.bench_function("radial_two_circle",
        |b| b.iter(|| black_box(sample_fixed_checksum(&focal))));
    paint_group.bench_function("conic",
        |b| b.iter(|| black_box(sample_fixed_checksum(&conic))));
    paint_group.finish();

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
    benchmark_linear_presentation(c);
    benchmark_analytic_active(c);
    benchmark_stroke(c);
    benchmark_paint(c);
}

criterion_group!(raster, benchmarks);
criterion_main!(raster);
