
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use ugl_rs::{analytic::AnalyticIntersection, color::RGBA, edge::Edge, raster::Intersection,
    canvas::{AnalyticRenderOptions, AnalyticRenderWorkspace, PixmapMut, RenderOptions,
        RenderWorkspace, render_solid, render_solid_analytic,
    }, geometry::{Affine, Path, PathBuilder},
};
#[cfg(feature = "fixed")] use ugl_rs::raster::FillRule;
#[cfg(feature = "fixed")]
#[derive(Default)] struct RunCounter { runs: u32, pixels: u32 }

#[cfg(feature = "fixed")]
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
    group.bench_function(BenchmarkId::new("analytic", "64_rectangles"), |b| b.iter(|| {
        pixels.fill(0);
        let mut target = PixmapMut::new(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap();
        render_solid_analytic(&path, Affine::identity(), RGBA::new(40, 120, 220, 192),
            AnalyticRenderOptions::default(), &mut target, &mut AnalyticRenderWorkspace {
                edges: &mut edges, intersections: &mut analytic_intersections,
                row_coverage: &mut row_coverage,
            },
        ).unwrap();
        black_box(&pixels);
    }));
    group.finish();
}

#[cfg(feature = "fixed")] fn benchmark_fixed(c: &mut Criterion) {
    use ugl_rs::{canvas::render_solid_fixed, geometry::FixedScalar,
        raster_fixed::{FixedCoverageRun, FixedCoverageStrip, FixedCoverageWorkspace,
            FIXED_STRIP_HEIGHT, FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid,
            prepare_lines, rasterize_lines, rasterize_lines_to_strips,
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
        let (mut segments, mut trapezoids, mut row_area, mut pixels,
            mut strip_offsets, mut strip_indices, mut coverage_strips, mut coverage_runs) = (
            vec![FixedSegment::default(); line_count],
            vec![FixedTrapezoid::default(); line_count.div_ceil(2)],
            vec![0; WIDTH as usize], vec![0; WIDTH as usize * HEIGHT as usize * 4],
            vec![0; requirements.offsets], vec![0; requirements.indices],
            vec![FixedCoverageStrip::default();
                HEIGHT.div_ceil(FIXED_STRIP_HEIGHT) as usize],
            vec![FixedCoverageRun::default(); WIDTH as usize * HEIGHT as usize],
        );
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
            black_box((retained.strips().len(), retained.runs().len()));
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
    }
    group.finish();
}

fn  benchmarks(c: &mut Criterion) {
    #[cfg(feature = "fixed")] benchmark_fixed(c);
    benchmark_f32(c);
}

criterion_group!(raster, benchmarks);
criterion_main!(raster);
