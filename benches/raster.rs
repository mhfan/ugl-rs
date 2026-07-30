
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use ugl_rs::{analytic::AnalyticIntersection, color::RGBA, edge::Edge, raster::Intersection,
    canvas::{AnalyticRenderOptions, AnalyticRenderWorkspace, PixmapMut, RenderOptions,
        RenderWorkspace, render_solid, render_solid_analytic,
    }, geometry::{Affine, Path, PathBuilder},
};
#[cfg(feature = "fixed")] use ugl_rs::raster::FillRule;

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
        raster_fixed::{FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid, prepare_lines},
    };

    let mut source_edges = Vec::with_capacity(EDGE_CAPACITY);
    for index in 0..SHAPES {
        let x = (index % 8) as f32 * 30.0 + 4.25;
        let y = (index / 8) as f32 * 30.0 + 4.5;
        let (left, right, top, bottom) = (
            FixedScalar::from_num(x), FixedScalar::from_num(x + 22.5),
            FixedScalar::from_num(y), FixedScalar::from_num(y + 21.75),
        );
        source_edges.extend([
            Edge { upper: (left, top).into(), lower: (left, bottom).into(), winding: -1 },
            Edge { upper: (right, top).into(), lower: (right, bottom).into(), winding: 1 },
        ]);
    }
    let mut lines = vec![FixedLine::default(); EDGE_CAPACITY];
    let line_count = prepare_lines(&source_edges, &mut lines).unwrap();
    let (mut segments, mut trapezoids, mut row_area, mut pixels) = (
        vec![FixedSegment::default(); EDGE_CAPACITY],
        vec![FixedTrapezoid::default(); EDGE_CAPACITY.div_ceil(2)],
        vec![0; WIDTH as usize], vec![0; WIDTH as usize * HEIGHT as usize * 4],
    );

    let mut group = c.benchmark_group("raster_rgba8888");
    group.throughput(Throughput::Elements((WIDTH as u64) * HEIGHT as u64));
    group.bench_function(BenchmarkId::new("fixed", "64_rectangles"), |b| b.iter(|| {
        pixels.fill(0);
        let mut target = PixmapMut::new(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap();
        render_solid_fixed(&lines[..line_count], RGBA::new(40, 120, 220, 192),
            FillRule::NonZero, &mut target, &mut FixedRasterWorkspace {
                segments: &mut segments, trapezoids: &mut trapezoids,
                row_area: &mut row_area,
            },
        ).unwrap();
        black_box(&pixels);
    }));
    group.finish();
}

fn  benchmarks(c: &mut Criterion) {
    #[cfg(feature = "fixed")] benchmark_fixed(c);
    benchmark_f32(c);
}

criterion_group!(raster, benchmarks);
criterion_main!(raster);
