
use std::hint::black_box;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ugl_rs::{common::{color::{PremulSRGBA8, LinearPremulRGBA, SRGBA, SRGBA as RGBA},
        dash::{DashContour, DashWorkspace}, edge::{Edge, build_fill_edges},
        geometry::{Affine, Path, PathBuilder, Point}, raster::{CoverageSink, FillRule},
        stroke::{LineCap, LineJoin, StrokeContour, StrokePathWorkspace}, Pixmap, SolidPaint,
        SpreadMode},
    float::{analytic::{BinWorkspace as AnalyticBinWorkspace,
        Cell as AnalyticCell, CellWorkspace as AnalyticCellWorkspace,
        Intersection as AnalyticIntersection, Workspace as AnalyticWorkspace,
        bin_requirements, build_row_bins, rasterize_edges_binned, rasterize_edges_cells},
        dash::{dash_polyline, DashPattern}, raster::Intersection,
        canvas::{RenderOptions, RenderWorkspace, SampledRenderOptions,
            SampledRenderWorkspace, StrokePathOptions, StrokeWorkspace,
            render_solid, render_solid_sampled, render_stroke_solid},
        linear::{LinearPixmap, Srgb8Encoder, SRGB8_ENCODE_LUT_SIZE,
            render_paint as render_paint_linear, render_solid as render_solid_linear},
        sampler::{ConicAngleMode, ConicGradient, GradientStop, GradientStops,
            LinearGradient, LinearPaintSampler, PaintSampler, RadialGradient},
        stroke::{StrokeOptions, flatten_stroke_path, stroke_polyline}},
    flatten::FlattenOptions,
};
#[derive(Default)] struct RunCounter { runs: u32, pixels: u32 }
#[derive(Default)] struct SpanStatistics {
    runs: u32, pixels: u32, full_runs: u32, full_pixels: u32,
    maximum_len: u32, length_buckets: [u32; 6],
}
struct PointLinearSampler<'a, S>(&'a S);
struct CompositeLinearSampler<'a, S>(&'a S);
struct SolidBufferSink<'a> {
    pixels: &'a mut [u8], stride: usize, color: [u8; 4],
}

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

impl CoverageSink for SolidBufferSink<'_> {
    type Error = core::convert::Infallible;
    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        let scale = |a, b| (a as u16 * b as u16 + 127).div_euclid(255) as u8;
        let alpha = scale(self.color[3], coverage);
        let source = self.color.map(|channel| scale(channel, coverage));
        let start = y as usize * self.stride + x as usize * 4;
        let end = start + len as usize * 4;
        for pixel in self.pixels[start..end].chunks_exact_mut(4) {
            for channel in 0..3 {
                pixel[channel] = source[channel]
                    .saturating_add(scale(pixel[channel], u8::MAX - alpha));
            }
            pixel[3] = alpha.saturating_add(scale(pixel[3], u8::MAX - alpha));
        }
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

fn triangle_scene() -> Path {
    let mut path = PathBuilder::with_capacity(SHAPES * 4);
    for index in 0..SHAPES {
        let x = (index % 8) as f32 * 30.0 + 4.25;
        let y = (index / 8) as f32 * 30.0 + 4.5;
        path.move_to((x, y + 21.5)).line_to((x + 11.25, y))
            .line_to((x + 22.5, y + 21.5));
    }   path.build()
}

fn fill_curve_scene() -> Path {
    let mut path = PathBuilder::with_capacity(9);
    path.move_to((8.0, 128.0));
    for index in 0..8 {
        let x = 8.0 + index as f32 * 30.0;
        let y = if index & 1 == 0 { 112.0 } else { 144.0 };
        path.cubic_to((x + 10.0, y), (x + 20.0, y), (x + 30.0, 128.0));
    }   path.build()
}

fn benchmark_fill_stages(c: &mut Criterion) {
    let mut group = c.benchmark_group("fill_stages_f32");
    for (name, path) in [("triangles_64", triangle_scene()),
                         ("cubics_8", fill_curve_scene())] {
        let mut edges = Vec::new();
        build_fill_edges(&path, Affine::identity(), FlattenOptions::default(),
            &mut |edge| {
                edges.push(edge); Ok::<_, core::convert::Infallible>(())
            }).unwrap();
        let edge_count = edges.len();
        group.throughput(Throughput::Elements(edge_count as _));

        let mut built_edges = Vec::with_capacity(edge_count);
        group.bench_function(BenchmarkId::new("edge_build", name), |b| b.iter(|| {
            built_edges.clear();
            build_fill_edges(&path, Affine::identity(), FlattenOptions::default(),
                &mut |edge| {
                    built_edges.push(edge); Ok::<_, core::convert::Infallible>(())
                }).unwrap();
            black_box(built_edges.len());
        }));

        let requirements = bin_requirements(&edges, HEIGHT).unwrap();
        let (mut offsets, mut indices) =
            (vec![0; requirements.offsets], vec![0; requirements.indices]);
        group.bench_function(BenchmarkId::new("row_binning", name), |b| b.iter(|| {
            black_box(build_row_bins(&edges, HEIGHT, AnalyticBinWorkspace {
                row_offsets: &mut offsets, edge_indices: &mut indices,
            }).unwrap());
        }));

        let bins = build_row_bins(&edges, HEIGHT, AnalyticBinWorkspace {
            row_offsets: &mut offsets, edge_indices: &mut indices,
        }).unwrap();
        let (mut active, mut cells) = (
            vec![AnalyticIntersection::default(); edge_count],
            vec![AnalyticCell::default(); WIDTH as usize],
        );
        group.bench_function(BenchmarkId::new("coverage_cells", name), |b| b.iter(|| {
            let mut sink = RunCounter::default();
            rasterize_edges_cells(&edges, bins, WIDTH, HEIGHT, FillRule::NonZero,
                &mut AnalyticCellWorkspace {
                    intersections: &mut active, cells: &mut cells,
                }, &mut sink).unwrap();
            black_box((sink.runs, sink.pixels));
        }));
    }
    group.finish();
}

fn report_span_statistics(path: &Path) {
    if std::env::var_os("UGL_SPAN_STATS").is_none() { return; }
    let mut edges = Vec::with_capacity(EDGE_CAPACITY);
    build_fill_edges(path, Affine::identity(), FlattenOptions::default(),
        &mut |edge| {
            edges.push(edge);
            Ok::<_, core::convert::Infallible>(())
        }).unwrap();
    let requirements = bin_requirements(&edges, HEIGHT).unwrap();
    let (mut offsets, mut indices) =
        (vec![0; requirements.offsets], vec![0; requirements.indices]);
    let bins = build_row_bins(&edges, HEIGHT, AnalyticBinWorkspace {
        row_offsets: &mut offsets, edge_indices: &mut indices,
    }).unwrap();
    let (mut active, mut row, mut stats) = (
        vec![AnalyticIntersection::default(); edges.len()],
        vec![0.0; WIDTH as usize], SpanStatistics::default(),
    );
    rasterize_edges_binned(&edges, bins, WIDTH, HEIGHT, FillRule::NonZero,
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

    let (mut edges, mut intersections, mut sampled_row_coverage) = (
        vec![Edge::default(); EDGE_CAPACITY],
        vec![Intersection::default(); EDGE_CAPACITY],
        vec![0.0; WIDTH as usize],
    );
    group.bench_function(BenchmarkId::new("sampled", "64_rectangles"), |b| b.iter(|| {
        pixels.fill(0);
        let mut target = Pixmap::from_buffer(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap();
        render_solid_sampled(&path, Affine::identity(), RGBA::new(40, 120, 220, 192),
            SampledRenderOptions::default(), &mut target, &mut SampledRenderWorkspace {
                edges: &mut edges, intersections: &mut intersections,
                row_coverage: &mut sampled_row_coverage,
            },
        ).unwrap();
        black_box(&pixels);
    }));

    let mut analytic_intersections = vec![AnalyticIntersection::default(); EDGE_CAPACITY];
    let mut analytic_cells = vec![AnalyticCell::default(); WIDTH as usize];
    let (mut analytic_offsets, mut analytic_indices) =
        (vec![0; HEIGHT as usize + 1], vec![0; EDGE_CAPACITY]);
    group.bench_function(BenchmarkId::new("analytic", "64_rectangles"), |b| b.iter(|| {
        pixels.fill(0);
        let mut target = Pixmap::from_buffer(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap();
        render_solid(&path, Affine::identity(), RGBA::new(40, 120, 220, 192),
            RenderOptions::default(), &mut target, &mut RenderWorkspace {
                edges: &mut edges, intersections: &mut analytic_intersections,
                cells: &mut analytic_cells,
                row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
            },
        ).unwrap();
        black_box(&pixels);
    }));

    let mut prepared_edges = Vec::with_capacity(EDGE_CAPACITY);
    build_fill_edges(&path, Affine::identity(), FlattenOptions::default(),
        &mut |edge| {
            prepared_edges.push(edge);
            Ok::<_, core::convert::Infallible>(())
        }).unwrap();
    let requirements = bin_requirements(&prepared_edges, HEIGHT).unwrap();
    let (mut coverage_offsets, mut coverage_indices) =
        (vec![0; requirements.offsets], vec![0; requirements.indices]);
    let bins = build_row_bins(&prepared_edges, HEIGHT, AnalyticBinWorkspace {
        row_offsets: &mut coverage_offsets, edge_indices: &mut coverage_indices,
    }).unwrap();
    let (mut coverage_active, mut coverage_cells) = (
        vec![AnalyticIntersection::default(); prepared_edges.len()],
        vec![AnalyticCell::default(); WIDTH as usize],
    );
    group.bench_function(BenchmarkId::new("analytic_coverage", "64_rectangles"),
        |b| b.iter(|| {
        let mut sink = RunCounter::default();
        rasterize_edges_cells(&prepared_edges, bins, WIDTH, HEIGHT, FillRule::NonZero,
            &mut AnalyticCellWorkspace {
                intersections: &mut coverage_active, cells: &mut coverage_cells,
            }, &mut sink).unwrap();
        black_box((sink.runs, sink.pixels));
    }));

    let mut linear_pixels =
        vec![LinearPremulRGBA::default(); WIDTH as usize * HEIGHT as usize];
    group.bench_function(BenchmarkId::new("analytic_linear_working", "64_rectangles"),
        |b| b.iter(|| {
            linear_pixels.fill(LinearPremulRGBA::default());
            let mut target =
                LinearPixmap::from_buffer(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
            render_solid_linear(&path, Affine::identity(),
                SRGBA::new(40, 120, 220, 192), RenderOptions::default(),
                &mut target, &mut RenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    cells: &mut analytic_cells,
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
                LinearPixmap::from_buffer(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
            render_paint_linear(&path, Affine::identity(),
                &PointLinearSampler(&gradient), RenderOptions::default(),
                &mut target, &mut RenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    cells: &mut analytic_cells,
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
            LinearPixmap::from_buffer(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
        render_paint_linear(&path, Affine::identity(), &PointLinearSampler(&radial),
            RenderOptions::default(), &mut target, &mut RenderWorkspace {
                edges: &mut edges, intersections: &mut analytic_intersections,
                cells: &mut analytic_cells,
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
                LinearPixmap::from_buffer(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
            render_paint_linear(&path, Affine::identity(), paint,
                RenderOptions::default(), &mut target,
                &mut RenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    cells: &mut analytic_cells,
                    row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
                }).unwrap();
            black_box(&linear_pixels);
        }));
    }
    group.bench_function(BenchmarkId::new(
        "analytic_linear_radial_concentric", "64_rectangles"), |b| b.iter(|| {
        linear_pixels.fill(LinearPremulRGBA::default());
        let mut target =
            LinearPixmap::from_buffer(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
        render_paint_linear(&path, Affine::identity(), &radial,
            RenderOptions::default(), &mut target, &mut RenderWorkspace {
                edges: &mut edges, intersections: &mut analytic_intersections,
                cells: &mut analytic_cells,
                row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
            }).unwrap();
        black_box(&linear_pixels);
    }));
    group.bench_function(BenchmarkId::new("analytic_linear_gradient", "64_rectangles"),
        |b| b.iter(|| {
            linear_pixels.fill(LinearPremulRGBA::default());
            let mut target =
                LinearPixmap::from_buffer(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
            render_paint_linear(&path, Affine::identity(), &gradient,
                RenderOptions::default(), &mut target,
                &mut RenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    cells: &mut analytic_cells,
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
            LinearPixmap::from_buffer(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
        render_paint_linear(&path, Affine::identity(),
            &CompositeLinearSampler(&opaque_gradient), RenderOptions::default(),
            &mut target, &mut RenderWorkspace {
                edges: &mut edges, intersections: &mut analytic_intersections,
                cells: &mut analytic_cells,
                row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
            }).unwrap();
        black_box(&linear_pixels);
    }));
    group.bench_function(BenchmarkId::new(
        "analytic_linear_gradient_opaque", "64_rectangles"), |b| b.iter(|| {
        linear_pixels.fill(LinearPremulRGBA::default());
        let mut target =
            LinearPixmap::from_buffer(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
        render_paint_linear(&path, Affine::identity(), &opaque_gradient,
            RenderOptions::default(), &mut target, &mut RenderWorkspace {
                edges: &mut edges, intersections: &mut analytic_intersections,
                cells: &mut analytic_cells,
                row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
            }).unwrap();
        black_box(&linear_pixels);
    }));
    group.bench_function(BenchmarkId::new("analytic_linear_present_exact", "64_rectangles"),
        |b| b.iter(|| {
            linear_pixels.fill(LinearPremulRGBA::default());
            let mut target =
                LinearPixmap::from_buffer(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
            render_solid_linear(&path, Affine::identity(),
                SRGBA::new(40, 120, 220, 192), RenderOptions::default(),
                &mut target, &mut RenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    cells: &mut analytic_cells,
                    row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
                }).unwrap();
            target.encode_into(
                &mut Pixmap::from_buffer(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap()).unwrap();
            black_box(&pixels);
        }));
    let mut transfer_lut = vec![0; SRGB8_ENCODE_LUT_SIZE];
    let encoder = Srgb8Encoder::new(&mut transfer_lut).unwrap();
    let mut dirty_tiles =
        vec![0; LinearPixmap::dirty_tile_words(WIDTH, HEIGHT).unwrap()];
    group.bench_function(BenchmarkId::new("analytic_linear_present_lut", "64_rectangles"),
        |b| b.iter(|| {
            linear_pixels.fill(LinearPremulRGBA::default());
            let mut target =
                LinearPixmap::from_buffer(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
            render_solid_linear(&path, Affine::identity(),
                SRGBA::new(40, 120, 220, 192), RenderOptions::default(),
                &mut target, &mut RenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    cells: &mut analytic_cells,
                    row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
                }).unwrap();
            target.encode_into_with(
                &mut Pixmap::from_buffer(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap(),
                encoder).unwrap();
            black_box(&pixels);
        }));
    group.bench_function(
        BenchmarkId::new("analytic_linear_present_dirty_lut", "64_rectangles"),
        |b| b.iter(|| {
            linear_pixels.fill(LinearPremulRGBA::default());
            let mut target = LinearPixmap::with_dirty_tiles(
                &mut linear_pixels, WIDTH, HEIGHT, WIDTH, &mut dirty_tiles).unwrap();
            render_solid_linear(&path, Affine::identity(),
                SRGBA::new(40, 120, 220, 192), RenderOptions::default(),
                &mut target, &mut RenderWorkspace {
                    edges: &mut edges, intersections: &mut analytic_intersections,
                    cells: &mut analytic_cells,
                    row_offsets: &mut analytic_offsets, edge_indices: &mut analytic_indices,
                }).unwrap();
            target.encode_dirty_into_with(
                &mut Pixmap::from_buffer(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap(),
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
        vec![0; LinearPixmap::dirty_tile_words(WIDTH, HEIGHT).unwrap()];
    let (mut edges, mut intersections, mut cells,
        mut row_offsets, mut edge_indices) = (
        vec![Edge::default(); 4], vec![AnalyticIntersection::default(); 4],
        vec![AnalyticCell::default(); WIDTH as usize],
        vec![0; HEIGHT as usize + 1], vec![0; 4],
    );
    let mut group = c.benchmark_group("linear_present_rgba8888");
    group.throughput(Throughput::Elements((WIDTH as u64) * HEIGHT as u64));

    group.bench_function("sparse_full_frame_lut", |b| b.iter(|| {
        let mut target =
            LinearPixmap::from_buffer(&mut linear_pixels, WIDTH, HEIGHT, WIDTH).unwrap();
        render_solid_linear(&path, Affine::identity(), SRGBA::white(),
            RenderOptions::default(), &mut target, &mut RenderWorkspace {
                edges: &mut edges, intersections: &mut intersections,
                cells: &mut cells,
                row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
            }).unwrap();
        target.encode_into_with(
            &mut Pixmap::from_buffer(&mut encoded_pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap(),
            encoder).unwrap();
        black_box(&encoded_pixels);
    }));
    group.bench_function("sparse_dirty_tiles_lut", |b| b.iter(|| {
        let mut target = LinearPixmap::with_dirty_tiles(
            &mut linear_pixels, WIDTH, HEIGHT, WIDTH, &mut dirty_tiles).unwrap();
        render_solid_linear(&path, Affine::identity(), SRGBA::white(),
            RenderOptions::default(), &mut target, &mut RenderWorkspace {
                edges: &mut edges, intersections: &mut intersections,
                cells: &mut cells,
                row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
            }).unwrap();
        target.encode_dirty_into_with(
            &mut Pixmap::from_buffer(&mut encoded_pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap(),
            encoder).unwrap();
        black_box(&encoded_pixels);
    }));
    group.finish();
}

fn benchmark_active(c: &mut Criterion) {
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
        let requirements = bin_requirements(&edges, HEIGHT).unwrap();
        let (mut offsets, mut indices) =
            (vec![0; requirements.offsets], vec![0; requirements.indices]);
        let bins = build_row_bins(&edges, HEIGHT, AnalyticBinWorkspace {
            row_offsets: &mut offsets, edge_indices: &mut indices,
        }).unwrap();
        let (mut active, mut row) =
            (vec![AnalyticIntersection::default(); edges.len()], vec![0.0; WIDTH as usize]);
        group.bench_function(&name, |b| b.iter(|| {
            let mut sink = RunCounter::default();
            rasterize_edges_binned(&edges, bins, WIDTH, HEIGHT, FillRule::NonZero,
                &mut AnalyticWorkspace {
                    intersections: &mut active, row_coverage: &mut row,
                }, &mut sink,
            ).unwrap();
            black_box((sink.runs, sink.pixels));
        }));
        let (mut cell_active, mut cells) = (
            vec![AnalyticIntersection::default(); edges.len()],
            vec![AnalyticCell::default(); WIDTH as usize],
        );
        group.bench_function(format!("{name}_cells"), |b| b.iter(|| {
            let mut sink = RunCounter::default();
            rasterize_edges_cells(&edges, bins, WIDTH, HEIGHT, FillRule::NonZero,
                &mut AnalyticCellWorkspace {
                    intersections: &mut cell_active, cells: &mut cells,
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
        let y = if index & 1 == 0 { 112.0 } else { 144.0 };
        path.cubic_to((x + 10.0, y), (x + 20.0, y), (x + 30.0, 128.0));
    }   path.build()
}

fn comparison_polyline_scene() -> Path {
    let mut path = PathBuilder::with_capacity(33);
    path.move_to((8.0, 128.0));
    for index in 1..=32 {
        let y = if index & 1 == 0 { 96.0 } else { 160.0 };
        path.line_to((8.0 + index as f32 * 7.5, y));
    }   path.build()
}

fn stroke_requirements(path: &Path, options: StrokePathOptions) ->
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
            StrokePathOptions { stroke: base, ..Default::default() }),
        ("round_polyline", stroke_polyline_scene(), StrokePathOptions {
            stroke: base.with_cap(LineCap::Round).with_join(LineJoin::Round),
            ..Default::default()
        }),
        ("butt_miter_curves", stroke_curve_scene(),
            StrokePathOptions { stroke: base, ..Default::default() }),
    ];
    let mut render_group = c.benchmark_group("stroke_rgba8888");
    render_group.throughput(Throughput::Elements(WIDTH as u64 * HEIGHT as u64));
    for (name, path, options) in scenes {
        let (point_count, contour_count, edge_count) =
            stroke_requirements(&path, options);
        let scratch = format!("{point_count}p_{contour_count}c_{edge_count}e");
        let (mut points, mut contours, mut edges, mut intersections,
            mut cells, mut pixels) = (
            vec![Default::default(); point_count],
            vec![StrokeContour::default(); contour_count],
            vec![Edge::default(); edge_count],
            vec![AnalyticIntersection::default(); edge_count],
            vec![AnalyticCell::default(); WIDTH as usize],
            vec![0; WIDTH as usize * HEIGHT as usize * 4],
        );
        let (mut row_offsets, mut edge_indices) =
            (vec![0; HEIGHT as usize + 1], vec![0; edge_count]);
        render_group.bench_function(BenchmarkId::new(name, scratch), |b| b.iter(|| {
            pixels.fill(0);
            render_stroke_solid(&path, Affine::identity(),
                RGBA::new(40, 120, 220, 192), options,
                &mut Pixmap::from_buffer(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap(),
                &mut StrokeWorkspace {
                    points: &mut points, contours: &mut contours, edges: &mut edges,
                    intersections: &mut intersections, cells: &mut cells,
                    row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
                }).unwrap();
            black_box(&pixels);
        }));
    }
    render_group.finish();

    let mut expand_group = c.benchmark_group("stroke_expand");
    let scenes = [
        ("butt_miter_polyline", stroke_polyline_scene(),
            StrokePathOptions { stroke: base, ..Default::default() }),
        ("round_polyline", stroke_polyline_scene(), StrokePathOptions {
            stroke: base.with_cap(LineCap::Round).with_join(LineJoin::Round),
            ..Default::default()
        }),
        ("butt_miter_curves", stroke_curve_scene(),
            StrokePathOptions { stroke: base, ..Default::default() }),
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

    let (path, options) = (stroke_curve_scene(), StrokePathOptions {
        stroke: base, ..Default::default()
    });
    let (point_count, contour_count, edge_count) = stroke_requirements(&path, options);
    let scratch = format!("{point_count}p_{contour_count}c_{edge_count}e");
    let mut stages = c.benchmark_group("stroke_stages_f32");
    stages.throughput(Throughput::Elements(edge_count as _));

    let (mut flatten_points, mut flatten_contours) = (
        vec![Point::default(); point_count], vec![StrokeContour::default(); contour_count]);
    stages.bench_function(BenchmarkId::new("flatten", &scratch), |b| b.iter(|| {
        let mut workspace = StrokePathWorkspace {
            points: &mut flatten_points, contours: &mut flatten_contours,
        };
        let flattened = flatten_stroke_path(&path, Affine::identity(), options.flatten,
            &mut workspace).unwrap();
        black_box((flattened.point_count(), flattened.contour_count()));
    }));

    let (mut outline_points, mut outline_contours) = (
        vec![Point::default(); point_count], vec![StrokeContour::default(); contour_count]);
    let mut outline_workspace = StrokePathWorkspace {
        points: &mut outline_points, contours: &mut outline_contours,
    };
    let flattened = flatten_stroke_path(&path, Affine::identity(), options.flatten,
        &mut outline_workspace).unwrap();
    stages.bench_function(BenchmarkId::new("outline", &scratch), |b| b.iter(|| {
        let mut emitted = 0;
        for (points, closed) in flattened.contours() {
            stroke_polyline(points, closed, options.stroke, &mut |_| {
                emitted += 1; Ok::<_, core::convert::Infallible>(())
            }).unwrap();
        }
        black_box(emitted);
    }));

    let mut edges = Vec::with_capacity(edge_count);
    for (points, closed) in flattened.contours() {
        stroke_polyline(points, closed, options.stroke, &mut |edge| {
            edges.push(edge); Ok::<_, core::convert::Infallible>(())
        }).unwrap();
    }
    let requirements = bin_requirements(&edges, HEIGHT).unwrap();
    let (mut offsets, mut indices) = (
        vec![0; requirements.offsets], vec![0; requirements.indices]);
    stages.bench_function(BenchmarkId::new("row_binning", &scratch), |b| b.iter(|| {
        black_box(build_row_bins(&edges, HEIGHT, AnalyticBinWorkspace {
            row_offsets: &mut offsets, edge_indices: &mut indices,
        }).unwrap());
    }));

    let bins = build_row_bins(&edges, HEIGHT, AnalyticBinWorkspace {
        row_offsets: &mut offsets, edge_indices: &mut indices,
    }).unwrap();
    let (mut active, mut row) = (
        vec![AnalyticIntersection::default(); edge_count], vec![0.0; WIDTH as usize]);
    stages.bench_function(BenchmarkId::new("coverage", &scratch), |b| b.iter(|| {
        let mut sink = RunCounter::default();
        rasterize_edges_binned(&edges, bins, WIDTH, HEIGHT, FillRule::NonZero,
            &mut AnalyticWorkspace {
                intersections: &mut active, row_coverage: &mut row,
            }, &mut sink).unwrap();
        black_box((sink.runs, sink.pixels));
    }));
    let (mut cell_active, mut cells) = (
        vec![AnalyticIntersection::default(); edge_count],
        vec![AnalyticCell::default(); WIDTH as usize],
    );
    stages.bench_function(BenchmarkId::new("coverage_cells", &scratch), |b| b.iter(|| {
        let mut sink = RunCounter::default();
        rasterize_edges_cells(&edges, bins, WIDTH, HEIGHT, FillRule::NonZero,
            &mut AnalyticCellWorkspace {
                intersections: &mut cell_active, cells: &mut cells,
            }, &mut sink).unwrap();
        black_box((sink.runs, sink.pixels));
    }));
    let color = SRGBA::new(40, 120, 220, 192).premul_encoded().to_array();
    let mut blend_pixels = vec![0; WIDTH as usize * HEIGHT as usize * 4];
    stages.bench_function(BenchmarkId::new("coverage_blend", &scratch), |b| b.iter(|| {
        blend_pixels.fill(0);
        let mut sink = SolidBufferSink {
            pixels: &mut blend_pixels, stride: WIDTH as usize * 4, color,
        };
        rasterize_edges_binned(&edges, bins, WIDTH, HEIGHT, FillRule::NonZero,
            &mut AnalyticWorkspace {
                intersections: &mut active, row_coverage: &mut row,
            }, &mut sink).unwrap();
        black_box(&blend_pixels);
    }));
    stages.bench_function(BenchmarkId::new("coverage_cells_blend", &scratch), |b| b.iter(|| {
        blend_pixels.fill(0);
        let mut sink = SolidBufferSink {
            pixels: &mut blend_pixels, stride: WIDTH as usize * 4, color,
        };
        rasterize_edges_cells(&edges, bins, WIDTH, HEIGHT, FillRule::NonZero,
            &mut AnalyticCellWorkspace {
                intersections: &mut cell_active, cells: &mut cells,
            }, &mut sink).unwrap();
        black_box(&blend_pixels);
    }));
    stages.finish();

    let dash_points: Vec<_> = (0..64).map(|index|
        (index as f32 * 3.0 + 8.0,
         if index & 1 == 0 { 96.0 } else { 112.0 }).into()).collect();
    let pattern = DashPattern::new(&[6.0, 3.0, 1.5, 3.0], 2.0).unwrap();
    let (mut output, mut contours) =
        (vec![Point::default(); 512], vec![DashContour::default(); 256]);
    let mut dash_group = c.benchmark_group("stroke_dash");
    dash_group.throughput(Throughput::Elements(dash_points.len() as _));
    dash_group.bench_function("polyline_64_decompose", |b| b.iter(|| {
        let mut workspace = DashWorkspace {
            points: &mut output, contours: &mut contours,
        };
        let dashed = dash_polyline(&dash_points, false, pattern, &mut workspace).unwrap();
        black_box((dashed.point_count(), dashed.contour_count()));
    }));
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

fn benchmark_stroke_coverage(c: &mut Criterion) {
    let base = StrokeOptions::new(6.0).unwrap();
    let scenes = [
        ("polyline_32", comparison_polyline_scene(),
            StrokePathOptions { stroke: base, ..Default::default() }),
        ("polyline_round_32", comparison_polyline_scene(), StrokePathOptions {
            stroke: base.with_cap(LineCap::Round).with_join(LineJoin::Round),
            ..Default::default()
        }),
        ("cubics_8", stroke_curve_scene(),
            StrokePathOptions { stroke: base, ..Default::default() }),
    ];
    let mut group = c.benchmark_group("stroke_coverage_f32");
    for (name, path, options) in scenes {
        let (point_count, contour_count, edge_count) = stroke_requirements(&path, options);
        let (mut points, mut contours) = (
            vec![Point::default(); point_count],
            vec![StrokeContour::default(); contour_count],
        );
        let mut workspace = StrokePathWorkspace {
            points: &mut points, contours: &mut contours,
        };
        let flattened = flatten_stroke_path(
            &path, Affine::identity(), options.flatten, &mut workspace).unwrap();
        let mut edges = Vec::with_capacity(edge_count);
        for (points, closed) in flattened.contours() {
            stroke_polyline(points, closed, options.stroke, &mut |edge| {
                edges.push(edge); Ok::<_, core::convert::Infallible>(())
            }).unwrap();
        }
        let requirements = bin_requirements(&edges, HEIGHT).unwrap();
        let (mut offsets, mut indices) =
            (vec![0; requirements.offsets], vec![0; requirements.indices]);
        let bins = build_row_bins(&edges, HEIGHT, AnalyticBinWorkspace {
            row_offsets: &mut offsets, edge_indices: &mut indices,
        }).unwrap();
        let (mut active, mut cells) = (
            vec![AnalyticIntersection::default(); edges.len()],
            vec![AnalyticCell::default(); WIDTH as usize],
        );
        group.throughput(Throughput::Elements(edges.len() as _));
        group.bench_function(name, |b| b.iter(|| {
            let mut sink = RunCounter::default();
            rasterize_edges_cells(&edges, bins, WIDTH, HEIGHT, FillRule::NonZero,
                &mut AnalyticCellWorkspace {
                    intersections: &mut active, cells: &mut cells,
                }, &mut sink).unwrap();
            black_box((sink.runs, sink.pixels));
        }));
    }
    group.finish();
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

fn sample_span_checksum(sampler: &impl PaintSampler) -> u64 {
    let mut checksum = 0_u64;
    for y in 0..HEIGHT {
        sampler.sample_span(0.5, y as f32 + 0.5, 1.0, 0.0, WIDTH,
            |color| checksum = color.to_array().into_iter().fold(checksum,
                |checksum, channel| checksum.wrapping_mul(257).wrapping_add(channel as _)));
    }   checksum
}

#[cfg(feature = "fixed")]
fn sample_fixed_checksum(sampler: &impl ugl_rs::fixed::sampler::PaintSampler) -> u64 {
    let mut checksum = 0_u64;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let color = sampler.sample(x, y);
            checksum = color.to_array().into_iter().fold(checksum,
                |checksum, channel| checksum.wrapping_mul(257).wrapping_add(channel as _));
        }
}       checksum
}

#[cfg(feature = "fixed")]
fn sample_fixed_span_checksum(sampler: &impl ugl_rs::fixed::sampler::PaintSampler) -> u64 {
    let mut checksum = 0_u64;
    for y in 0..HEIGHT {
        sampler.sample_span(0, y, WIDTH,
            |color| checksum = color.to_array().into_iter().fold(checksum,
                |checksum, channel| checksum.wrapping_mul(257).wrapping_add(channel as _)));
    }   checksum
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
    let mut ramp = vec![PremulSRGBA8::zeroed(); 1024];
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
    group.bench_function("linear_span",
        |b| b.iter(|| black_box(sample_span_checksum(&linear))));
    group.bench_function("radial", |b| b.iter(|| black_box(sample_checksum(&radial))));
    let concentric = RadialGradient::new(
        (128.0, 128.0), 112.0, stops, SpreadMode::Pad).unwrap();
    group.bench_function("radial_concentric_point",
        |b| b.iter(|| black_box(sample_checksum(&concentric))));
    group.bench_function("radial_concentric_span",
        |b| b.iter(|| black_box(sample_span_checksum(&concentric))));
    group.bench_function("conic",  |b| b.iter(|| black_box(sample_checksum(&conic))));
    group.bench_function("conic_span",
        |b| b.iter(|| black_box(sample_span_checksum(&conic))));
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
    use ugl_rs::{fixed::{
        canvas::{composite_solid_tiles, render_solid, render_solid_tiled},
        dash::{Pattern as DashPattern, dash_polyline},
        flatten::{Options as FlattenOptions, flatten_path},
        raster::{CoverageRun, CoverageStrip, CoverageWorkspace,
            Line, Workspace, Segment, Trapezoid, STRIP_HEIGHT,
            prepare_lines, rasterize_lines, rasterize_lines_to_strips},
        sampler::{Angle, ConicAngleMode, ConicGradient, LinearGradient, RadialGradient},
        stroke::{Options as StrokeOptions, flatten_path as flatten_stroke_path,
            stroke_polyline},
        tile::{CoverageTile, CoverageTilePiece, CoverageTileRun,
            CoverageTileWorkspace, DirectTilePiece, DirectTileWorkspace,
            encode_coverage_tiles, requirements as tile_requirements,
            rasterize_lines_to_tiles},
        }, fixed::Scalar,
        common::stroke::{StrokeContour, StrokePathWorkspace},
    };

    let stroke_points: Vec<_> = (0..64).map(|index|
        (Scalar::from_num(index * 3 + 8),
         Scalar::from_num(if index & 1 == 0 { 96 } else { 112 })).into()).collect();
    let stroke_options = StrokeOptions::new(Scalar::from_num(3)).unwrap()
        .with_cap(LineCap::Square).with_join(LineJoin::Miter);
    let round_stroke_options = stroke_options
        .with_cap(LineCap::Round).with_join(LineJoin::Round);
    let mut stroke_edges = Vec::with_capacity(512);
    let mut stroke_group = c.benchmark_group("stroke_expand_fixed");
    stroke_group.throughput(Throughput::Elements(stroke_points.len() as _));
    stroke_group.bench_function("square_miter_64", |b| b.iter(|| {
        stroke_edges.clear();
        stroke_polyline(&stroke_points, false, stroke_options, &mut |edge| {
            stroke_edges.push(edge); Ok::<_, core::convert::Infallible>(())
        }).unwrap();
        black_box(&stroke_edges);
    }));
    stroke_group.bench_function("round_64", |b| b.iter(|| {
        stroke_edges.clear();
        stroke_polyline(&stroke_points, false, round_stroke_options, &mut |edge| {
            stroke_edges.push(edge); Ok::<_, core::convert::Infallible>(())
        }).unwrap();
        black_box(&stroke_edges);
    }));
    stroke_group.finish();

    let comparison_points: Vec<_> = (0..=32).map(|index| (
        Scalar::from_num(8.0 + index as f32 * 7.5),
        Scalar::from_num(if index == 0 { 128.0 }
            else if index & 1 == 0 { 96.0 } else { 160.0 }),
    ).into()).collect();
    let comparison_options = StrokeOptions::new(Scalar::from_num(6)).unwrap();
    let comparison_round = comparison_options
        .with_cap(LineCap::Round).with_join(LineJoin::Round);
    let comparison_round_4 = comparison_round.with_round_segments(4).unwrap();
    let mut coverage_group = c.benchmark_group("stroke_coverage_fixed");
    for (name, options) in [("polyline_32", comparison_options),
                            ("polyline_round_32", comparison_round),
                            ("polyline_round_4_32", comparison_round_4)] {
        let mut edges = Vec::with_capacity(256);
        stroke_polyline(&comparison_points, false, options, &mut |edge| {
            edges.push(edge); Ok::<_, core::convert::Infallible>(())
        }).unwrap();
        let mut lines = vec![Line::default(); edges.len()];
        let line_count = prepare_lines(&edges, &mut lines).unwrap();
        let requirements = ugl_rs::fixed::raster::strip_requirements(
            &lines[..line_count], HEIGHT).unwrap();
        let (mut segments, mut trapezoids, mut row_area, mut offsets, mut indices) = (
            vec![Segment::default(); line_count],
            vec![Trapezoid::default(); line_count.div_ceil(2)],
            vec![0; WIDTH as usize], vec![0; requirements.offsets],
            vec![0; requirements.indices],
        );
        coverage_group.throughput(Throughput::Elements(line_count as _));
        coverage_group.bench_function(name, |b| b.iter(|| {
            let mut sink = RunCounter::default();
            rasterize_lines(&lines[..line_count], WIDTH, HEIGHT, FillRule::NonZero,
                &mut Workspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut offsets, strip_indices: &mut indices,
                }, &mut sink).unwrap();
            black_box((sink.runs, sink.pixels));
        }));
    }
    coverage_group.finish();

    let fixed_dash_lengths = [Scalar::from_num(6), Scalar::from_num(3),
        Scalar::from_num(1.5), Scalar::from_num(3)];
    let fixed_dash = DashPattern::new(
        &fixed_dash_lengths, Scalar::from_num(2)).unwrap();
    let (mut fixed_dash_points, mut fixed_dash_contours) = (
        vec![(Scalar::ZERO, Scalar::ZERO).into(); 512],
        vec![DashContour::default(); 256],
    );
    let mut dash_group = c.benchmark_group("stroke_dash_fixed");
    dash_group.throughput(Throughput::Elements(stroke_points.len() as _));
    dash_group.bench_function("polyline_64_decompose", |b| b.iter(|| {
        let mut workspace = DashWorkspace {
            points: &mut fixed_dash_points, contours: &mut fixed_dash_contours,
        };
        let dashed = dash_polyline(&stroke_points, false, fixed_dash,
            &mut workspace).unwrap();
        black_box((dashed.point_count(), dashed.contour_count()));
    }));
    dash_group.bench_function("polyline_64", |b| b.iter(|| {
        stroke_edges.clear();
        let mut workspace = DashWorkspace {
            points: &mut fixed_dash_points, contours: &mut fixed_dash_contours,
        };
        let dashed = dash_polyline(&stroke_points, false, fixed_dash,
            &mut workspace).unwrap();
        for (points, closed) in dashed.contours() {
            stroke_polyline(points, closed, stroke_options, &mut |edge| {
                stroke_edges.push(edge); Ok::<_, core::convert::Infallible>(())
            }).unwrap();
        }
        black_box(&stroke_edges);
    }));
    dash_group.finish();

    let mut curve_builder = PathBuilder::new();
    curve_builder.move_to((Scalar::from_num(8), Scalar::from_num(128)));
    for index in 0..8 {
        let x = index * 28 + 8;
        curve_builder.cubic_to(
            (Scalar::from_num(x + 7), Scalar::from_num(32)),
            (Scalar::from_num(x + 21), Scalar::from_num(224)),
            (Scalar::from_num(x + 28), Scalar::from_num(128)));
    }
    let curve_path = curve_builder.build();
    let (mut stroke_path_points, mut stroke_path_contours) =
        (vec![(Scalar::ZERO, Scalar::ZERO).into(); 512],
         vec![StrokeContour::default(); 16]);
    let mut stroke_path_group = c.benchmark_group("stroke_path_fixed");
    stroke_path_group.throughput(Throughput::Elements(8));
    stroke_path_group.bench_function("cubic_8", |b| b.iter(|| {
        stroke_edges.clear();
        let mut workspace = StrokePathWorkspace {
            points: &mut stroke_path_points, contours: &mut stroke_path_contours,
        };
        let flattened = flatten_stroke_path(&curve_path, Affine::identity(),
            FlattenOptions::default(), &mut workspace).unwrap();
        for (points, closed) in flattened.contours() {
            stroke_polyline(points, closed, stroke_options, &mut |edge| {
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
        flatten_path(&curve_path, Affine::identity(),
            FlattenOptions::default(), &mut |_, _| {
            line_count += 1; Ok::<_, core::convert::Infallible>(())
        }).unwrap();
        black_box(line_count);
    }));
    flatten_group.finish();

    let stop_values = [GradientStop::new( 0.0, RGBA::new(240, 20, 80,  32)),
                       GradientStop::new(0.35, RGBA::new(10, 220, 40, 160)),
                       GradientStop::new( 1.0, RGBA::new(30, 60, 250, 224)) ];
    let mut ramp = vec![PremulSRGBA8::zeroed(); 1024];
    let stops = GradientStops::with_ramp(&stop_values, &mut ramp).unwrap();
    let ramp = stops.encoded_ramp().unwrap();
    let fixed = Scalar::from_num;
    let linear = LinearGradient::new(
        (fixed(0), fixed(0)), (fixed(WIDTH), fixed(HEIGHT)),
        ramp, SpreadMode::Pad).unwrap();
    let radial = RadialGradient::new(
        (fixed(WIDTH / 2), fixed(HEIGHT / 2)), fixed(180),
        ramp, SpreadMode::Pad).unwrap();
    let focal = RadialGradient::two_circle(
        (fixed(96), fixed(112)), fixed(8), (fixed(128), fixed(128)), fixed(180),
        ramp, SpreadMode::Pad).unwrap();
    let conic = ConicGradient::new(
        (fixed(128), fixed(128)), Angle::from_bits(0x0f12_3456), ramp).unwrap();
    let conic_fast = ConicGradient::with_angle_mode(
        (fixed(128), fixed(128)), Angle::from_bits(0x0f12_3456), ramp,
        ConicAngleMode::Fast).unwrap();
    let mut paint_group = c.benchmark_group("paint_sample_fixed");
    paint_group.throughput(Throughput::Elements((WIDTH as u64) * HEIGHT as u64));
    paint_group.bench_function("linear",
        |b| b.iter(|| black_box(sample_fixed_checksum(&linear))));
    paint_group.bench_function("linear_span",
        |b| b.iter(|| black_box(sample_fixed_span_checksum(&linear))));
    paint_group.bench_function("radial_concentric",
        |b| b.iter(|| black_box(sample_fixed_checksum(&radial))));
    paint_group.bench_function("radial_concentric_span",
        |b| b.iter(|| black_box(sample_fixed_span_checksum(&radial))));
    paint_group.bench_function("radial_two_circle",
        |b| b.iter(|| black_box(sample_fixed_checksum(&focal))));
    paint_group.bench_function("conic",
        |b| b.iter(|| black_box(sample_fixed_checksum(&conic))));
    paint_group.bench_function("conic_span",
        |b| b.iter(|| black_box(sample_fixed_span_checksum(&conic))));
    paint_group.bench_function("conic_fast_span",
        |b| b.iter(|| black_box(sample_fixed_span_checksum(&conic_fast))));
    paint_group.bench_function("conic_fast_point",
        |b| b.iter(|| black_box(sample_fixed_checksum(&conic_fast))));
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
                Scalar::from_num(x), Scalar::from_num(x + width),
                Scalar::from_num(y), Scalar::from_num(y + height),
            );
            source_edges.extend([
                Edge { upper:  (left, top).into(), lower:  (left, bottom).into(), winding: -1 },
                Edge { upper: (right, top).into(), lower: (right, bottom).into(), winding: 1 },
            ]);
        }
        let mut lines = vec![Line::default(); source_edges.len()];
        let line_count = prepare_lines(&source_edges, &mut lines).unwrap();
        let strip_requirements =
            ugl_rs::fixed::raster::strip_requirements(&lines[..line_count], HEIGHT).unwrap();
        let tile_requirements = tile_requirements(WIDTH, HEIGHT).unwrap();
        let (mut segments, mut trapezoids, mut row_area, mut pixels,
            mut strip_offsets, mut strip_indices, mut coverage_strips, mut coverage_runs,
            mut coverage_tiles, mut coverage_tile_runs, mut coverage_tile_pieces,
            mut direct_tile_pieces, mut tile_heads, mut tile_tails, mut touched_tiles) = (
            vec![Segment::default(); line_count],
            vec![Trapezoid::default(); line_count.div_ceil(2)],
            vec![0; WIDTH as usize], vec![0; WIDTH as usize * HEIGHT as usize * 4],
            vec![0; strip_requirements.offsets], vec![0; strip_requirements.indices],
            vec![CoverageStrip::default();
                HEIGHT.div_ceil(STRIP_HEIGHT) as usize],
            vec![CoverageRun::default(); WIDTH as usize * HEIGHT as usize],
            vec![CoverageTile::default(); tile_requirements.tiles],
            vec![CoverageTileRun::default(); tile_requirements.runs],
            vec![CoverageTilePiece::default(); WIDTH as usize * HEIGHT as usize],
            vec![DirectTilePiece::default(); tile_requirements.pieces],
            vec![0; tile_requirements.columns],
            vec![0; tile_requirements.columns],
            vec![0; tile_requirements.columns],
        );
        let (mut cached_tiles, mut cached_runs) = (
            vec![CoverageTile::default(); coverage_tiles.len()],
            vec![CoverageTileRun::default(); coverage_tile_runs.len()],
        );
        let cached = rasterize_lines_to_tiles(&lines[..line_count], WIDTH, HEIGHT,
            FillRule::NonZero, &mut Workspace {
                segments: &mut segments, trapezoids: &mut trapezoids,
                row_area: &mut row_area,
                strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
            }, DirectTileWorkspace {
                tiles: &mut cached_tiles, runs: &mut cached_runs,
                pieces: &mut direct_tile_pieces,
                column_heads: &mut tile_heads, column_tails: &mut tile_tails,
                touched_columns: &mut touched_tiles,
            }).unwrap();
        group.bench_function(BenchmarkId::new("fixed", name), |b| b.iter(|| {
            pixels.fill(0);
            let mut target = Pixmap::from_buffer(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap();
            render_solid(&lines[..line_count], RGBA::new(40, 120, 220, 192),
                FillRule::NonZero, &mut target, &mut Workspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                },
            ).unwrap();
            black_box(&pixels);
        }));
        group.bench_function(BenchmarkId::new("fixed_tiled", name), |b| b.iter(|| {
            pixels.fill(0);
            let mut target = Pixmap::from_buffer(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap();
            render_solid_tiled(&lines[..line_count], RGBA::new(40, 120, 220, 192),
                FillRule::NonZero, &mut target, &mut Workspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, DirectTileWorkspace {
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
            composite_solid_tiles(cached, RGBA::new(40, 120, 220, 192),
                &mut Pixmap::from_buffer(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap()).unwrap();
            black_box(&pixels);
        }));
        group.bench_function(BenchmarkId::new("fixed_stream", name), |b| b.iter(|| {
            let mut sink = RunCounter::default();
            rasterize_lines(&lines[..line_count], WIDTH, HEIGHT, FillRule::NonZero,
                &mut Workspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, &mut sink,
            ).unwrap();
            black_box((sink.runs, sink.pixels));
        }));
        group.bench_function(BenchmarkId::new("fixed_strip_encode", name), |b| b.iter(|| {
            let retained = rasterize_lines_to_strips(&lines[..line_count], WIDTH, HEIGHT,
                FillRule::NonZero, &mut Workspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, CoverageWorkspace {
                    strips: &mut coverage_strips, runs: &mut coverage_runs,
                },
            ).unwrap();
            black_box(retained.strips());
            black_box(retained.runs());
        }));
        group.bench_function(BenchmarkId::new("fixed_strip_replay", name), |b| b.iter(|| {
            let retained = rasterize_lines_to_strips(&lines[..line_count], WIDTH, HEIGHT,
                FillRule::NonZero, &mut Workspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, CoverageWorkspace {
                    strips: &mut coverage_strips, runs: &mut coverage_runs,
                },
            ).unwrap();
            let mut sink = RunCounter::default();  retained.replay(&mut sink).unwrap();
            black_box((sink.runs, sink.pixels));
        }));
        group.bench_function(BenchmarkId::new("fixed_tile_encode", name), |b| b.iter(|| {
            let retained = rasterize_lines_to_strips(&lines[..line_count], WIDTH, HEIGHT,
                FillRule::NonZero, &mut Workspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, CoverageWorkspace {
                    strips: &mut coverage_strips, runs: &mut coverage_runs,
                },
            ).unwrap();
            let tiled = encode_coverage_tiles(retained, CoverageTileWorkspace {
                tiles: &mut coverage_tiles, runs: &mut coverage_tile_runs,
                pieces: &mut coverage_tile_pieces,
            }).unwrap();
            black_box(tiled.tiles());
            black_box(tiled.runs());
        }));
        group.bench_function(BenchmarkId::new("fixed_tile_replay", name), |b| b.iter(|| {
            let retained = rasterize_lines_to_strips(&lines[..line_count], WIDTH, HEIGHT,
                FillRule::NonZero, &mut Workspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, CoverageWorkspace {
                    strips: &mut coverage_strips, runs: &mut coverage_runs,
                },
            ).unwrap();
            let tiled = encode_coverage_tiles(retained, CoverageTileWorkspace {
                tiles: &mut coverage_tiles, runs: &mut coverage_tile_runs,
                pieces: &mut coverage_tile_pieces,
            }).unwrap();
            let mut sink = RunCounter::default();  tiled.replay(&mut sink).unwrap();
            black_box((sink.runs, sink.pixels));
        }));
        group.bench_function(BenchmarkId::new("fixed_tile_direct", name), |b| b.iter(|| {
            let tiled = rasterize_lines_to_tiles(&lines[..line_count], WIDTH, HEIGHT,
                FillRule::NonZero, &mut Workspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, DirectTileWorkspace {
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
                FillRule::NonZero, &mut Workspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area,
                    strip_offsets: &mut strip_offsets, strip_indices: &mut strip_indices,
                }, DirectTileWorkspace {
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

    let mut triangle_edges = Vec::with_capacity(SHAPES * 2);
    for index in 0..SHAPES {
        let (x, y) = (
            Scalar::from_num((index % 8) as f32 * 30.0 + 4.25),
            Scalar::from_num((index / 8) as f32 * 30.0 + 4.5),
        );
        let (left, apex, right) = (
            Point::from((x, y + Scalar::from_num(21.5))),
            Point::from((x + Scalar::from_num(11.25), y)),
            Point::from((x + Scalar::from_num(22.5), y + Scalar::from_num(21.5))),
        );
        triangle_edges.extend([
            Edge { upper: apex, lower: left, winding: -1 },
            Edge { upper: apex, lower: right, winding: 1 },
        ]);
    }
    let mut lines = vec![Line::default(); triangle_edges.len()];
    let line_count = prepare_lines(&triangle_edges, &mut lines).unwrap();
    let requirements = ugl_rs::fixed::raster::strip_requirements(
        &lines[..line_count], HEIGHT).unwrap();
    let (mut segments, mut trapezoids, mut row_area, mut offsets, mut indices) = (
        vec![Segment::default(); line_count],
        vec![Trapezoid::default(); line_count.div_ceil(2)],
        vec![0; WIDTH as usize], vec![0; requirements.offsets],
        vec![0; requirements.indices],
    );
    group.bench_function(BenchmarkId::new("fixed_stream", "triangles_64"), |b| b.iter(|| {
        let mut sink = RunCounter::default();
        rasterize_lines(&lines[..line_count], WIDTH, HEIGHT, FillRule::NonZero,
            &mut Workspace {
                segments: &mut segments, trapezoids: &mut trapezoids,
                row_area: &mut row_area,
                strip_offsets: &mut offsets, strip_indices: &mut indices,
            }, &mut sink).unwrap();
        black_box((sink.runs, sink.pixels));
    }));
    group.finish();
}

fn  benchmarks(c: &mut Criterion) {
    #[cfg(feature = "fixed")] benchmark_fixed(c);
    benchmark_f32(c);
    benchmark_linear_presentation(c);
    benchmark_active(c);
    benchmark_fill_stages(c);
    benchmark_stroke(c);
    benchmark_stroke_coverage(c);
    benchmark_paint(c);
}

criterion_group!(raster, benchmarks);
criterion_main!(raster);
