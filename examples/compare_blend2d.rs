//! Stable external-comparison runner. Keep this scene in sync with
//! `benches/blend2d/blend2d_bench.cpp`.

use std::{env, fs, hint::black_box, process::ExitCode, time::Instant};
use ugl_rs::{
    analytic::{Cell, Intersection},
    canvas::{Pixmap, RenderOptions, RenderWorkspace, StrokePathOptions, StrokeWorkspace,
        rasterize_path_clip, render_paint, render_solid, render_solid_clipped,
        render_solid_masked, render_stroke_solid},
    color::{PremulSRGBA8, SRGBA},
    edge::Edge,
    geometry::{Affine, Path, PathBuilder, Rect},
    raster::{CoverageMask, CoverageMaskMut},
    sampler::{ConicAngleMode, ConicGradient, GradientStop, GradientStops,
        LinearGradient, RadialGradient, SpreadMode},
    stroke::{LineCap, LineJoin, StrokeContour, StrokeOptions},
};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 256;
const SHAPES: usize = 64;
const EDGE_CAPACITY: usize = 4096;

#[derive(Clone, Copy)] enum Operation {
    Fill, FillClipped, FillGradient, FillRadial, FillConic,
    FillMasked, FillMaskedSparse, BuildMask,
    Stroke { round: bool },
}

fn rectangles(count: usize) -> Path {
    let mut path = PathBuilder::with_capacity(count * 5);
    for index in 0..count {
        let x = (index % 8) as f32 * 30.0 + 4.25;
        let y = (index / 8) as f32 * 30.0 + 4.5;
        path.move_to((x, y)).line_to((x + 22.5, y))
            .line_to((x + 22.5, y + 21.75)).line_to((x, y + 21.75));
    }
    path.build()
}

fn large_rectangle() -> Path {
    let mut path = PathBuilder::with_capacity(5);
    path.move_to((16.25, 20.5)).line_to((239.5, 20.5))
        .line_to((239.5, 235.25)).line_to((16.25, 235.25));
    path.build()
}

fn triangles() -> Path {
    let mut path = PathBuilder::with_capacity(SHAPES * 4);
    for index in 0..SHAPES {
        let x = (index % 8) as f32 * 30.0 + 4.25;
        let y = (index / 8) as f32 * 30.0 + 4.5;
        path.move_to((x, y + 21.5)).line_to((x + 11.25, y))
            .line_to((x + 22.5, y + 21.5));
    }
    path.build()
}

fn polyline() -> Path {
    let mut path = PathBuilder::with_capacity(33);
    path.move_to((8.0, 128.0));
    for index in 1..=32 {
        let y = if index & 1 == 0 { 96.0 } else { 160.0 };
        path.line_to((8.0 + index as f32 * 7.5, y));
    }
    path.build()
}

fn curves() -> Path {
    let mut path = PathBuilder::with_capacity(9);
    path.move_to((8.0, 128.0));
    for index in 0..8 {
        let x = 8.0 + index as f32 * 30.0;
        let y = if index & 1 == 0 { 112.0 } else { 144.0 };
        path.cubic_to((x + 10.0, y), (x + 20.0, y), (x + 30.0, 128.0));
    }
    path.build()
}

fn mask_path(radius: f32) -> Path {
    let mut path = PathBuilder::with_capacity(6);
    let k = radius * 0.552_284_7;
    path.move_to((128.0 + radius, 128.0))
        .cubic_to((128.0 + radius, 128.0 + k),
            (128.0 + k, 128.0 + radius), (128.0, 128.0 + radius))
        .cubic_to((128.0 - k, 128.0 + radius),
            (128.0 - radius, 128.0 + k), (128.0 - radius, 128.0))
        .cubic_to((128.0 - radius, 128.0 - k),
            (128.0 - k, 128.0 - radius), (128.0, 128.0 - radius))
        .cubic_to((128.0 + k, 128.0 - radius),
            (128.0 + radius, 128.0 - k), (128.0 + radius, 128.0));
    path.build()
}

fn scene() -> Result<(&'static str, Path, Operation), String> {
    match path_argument("--scene")?.as_deref().unwrap_or("fill_rectangles_64") {
        "fill_rectangles_1" => Ok(("fill_rectangles_1", rectangles(1), Operation::Fill)),
        "fill_rectangles_16" => Ok(("fill_rectangles_16", rectangles(16), Operation::Fill)),
        "fill_rectangles_64" => Ok(("fill_rectangles_64", rectangles(64), Operation::Fill)),
        "fill_rectangle_large" => Ok(("fill_rectangle_large", large_rectangle(),
            Operation::Fill)),
        "fill_rectangle_linear_gradient" => Ok(("fill_rectangle_linear_gradient",
            large_rectangle(), Operation::FillGradient)),
        "fill_rectangle_radial_gradient" => Ok(("fill_rectangle_radial_gradient",
            large_rectangle(), Operation::FillRadial)),
        "fill_rectangle_conic_gradient" => Ok(("fill_rectangle_conic_gradient",
            large_rectangle(), Operation::FillConic)),
        "fill_rectangle_path_mask" => Ok(("fill_rectangle_path_mask",
            large_rectangle(), Operation::FillMasked)),
        "fill_rectangle_path_mask_sparse" => Ok(("fill_rectangle_path_mask_sparse",
            large_rectangle(), Operation::FillMaskedSparse)),
        "build_path_mask" => Ok(("build_path_mask", mask_path(100.0), Operation::BuildMask)),
        "fill_triangles_64" => Ok(("fill_triangles_64", triangles(), Operation::Fill)),
        "fill_cubics_8" => Ok(("fill_cubics_8", curves(), Operation::Fill)),
        "fill_cubics_8_clip_rect" => Ok(("fill_cubics_8_clip_rect", curves(),
            Operation::FillClipped)),
        "stroke_cubics_8" => Ok(("stroke_cubics_8", curves(),
            Operation::Stroke { round: false })),
        "stroke_polyline_32" => Ok(("stroke_polyline_32", polyline(),
            Operation::Stroke { round: false })),
        "stroke_polyline_round_32" => Ok(("stroke_polyline_round_32", polyline(),
            Operation::Stroke { round: true })),
        name => Err(format!("unknown scene: {name}")),
    }
}

fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, &byte|
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3))
}

fn argument(name: &str, default: u32) -> Result<u32, String> {
    let Some(position) = env::args().position(|value| value == name) else {
        return Ok(default);
    };
    env::args().nth(position + 1).ok_or_else(|| format!("missing value after {name}"))?
        .parse().map_err(|_| format!("invalid integer after {name}"))
}

fn path_argument(name: &str) -> Result<Option<String>, String> {
    let Some(position) = env::args().position(|value| value == name) else {
        return Ok(None);
    };
    env::args().nth(position + 1).map(Some)
        .ok_or_else(|| format!("missing value after {name}"))
}

fn compare(label: &str, scene: &str, reference: &[u8], actual: &[u8]) -> Result<(), String> {
    if reference.len() != actual.len() {
        return Err(format!("image sizes differ: {} != {}", reference.len(), actual.len()));
    }
    let mut changed_pixels = 0_usize;
    let (mut total_error, mut maximum_error) = (0_u64, 0_u8);
    for (expected, rendered) in reference.chunks_exact(4).zip(actual.chunks_exact(4)) {
        let mut changed = false;
        for (&expected, &rendered) in expected.iter().zip(rendered) {
            let error = expected.abs_diff(rendered);
            total_error += u64::from(error);
            maximum_error = maximum_error.max(error);
            changed |= error != 0;
        }
        changed_pixels += usize::from(changed);
    }
    println!("image_diff,changed_pixels,total_pixels,changed_percent,mean_abs_channel_error,\
        max_abs_channel_error");
    println!("{label}:{scene},{changed_pixels},{},{:.6},{:.6},{maximum_error}",
        actual.len() / 4, changed_pixels as f64 * 400.0 / actual.len() as f64,
        total_error as f64 / actual.len() as f64);
    Ok(())
}

fn run_f32() -> Result<(), String> {
    let warmup = argument("--warmup", 500)?;
    let iterations = argument("--iterations", 5_000)?;
    let samples = argument("--samples", 9)?;
    if iterations == 0 || samples == 0 {
        return Err("--iterations and --samples must be positive".into());
    }

    let (scene, path, operation) = scene()?;
    let mut pixels = vec![0; WIDTH as usize * HEIGHT as usize * 4];
    let mut edges = vec![Edge::default(); EDGE_CAPACITY];
    let mut intersections = vec![Intersection::default(); EDGE_CAPACITY];
    let mut cells = vec![Cell::default(); WIDTH as usize];
    let mut row_offsets = vec![0; HEIGHT as usize + 1];
    let mut edge_indices = vec![0; EDGE_CAPACITY];
    let mut stroke_points = vec![Default::default(); 2048];
    let mut stroke_contours = vec![StrokeContour::default(); 16];
    let stop_values = [
        GradientStop::new(0.0, SRGBA::new(0, 0, 0, 32)),
        GradientStop::new(1.0, SRGBA::new(0, 0, 0, 224)),
    ];
    let (mut ramp, mut radial_ramp, mut conic_ramp) =
        ([PremulSRGBA8::default(); 256], [PremulSRGBA8::default(); 256],
         [PremulSRGBA8::default(); 256]);
    let gradient_stops = GradientStops::with_ramp(&stop_values, &mut ramp).unwrap();
    let gradient = LinearGradient::new((16.0, 128.0), (240.0, 128.0),
        gradient_stops, SpreadMode::Pad).unwrap();
    let radial = RadialGradient::new((128.0, 128.0), 112.0,
        GradientStops::with_ramp(&stop_values, &mut radial_ramp).unwrap(),
        SpreadMode::Pad).unwrap();
    let conic = ConicGradient::with_angle_mode((128.0, 128.0), 0.0,
        GradientStops::with_ramp(&stop_values, &mut conic_ramp).unwrap(),
        ConicAngleMode::Fast).unwrap();
    let mut mask_data = vec![0; WIDTH as usize * HEIGHT as usize];
    if matches!(operation, Operation::FillMasked | Operation::FillMaskedSparse) {
        let radius = if matches!(operation, Operation::FillMaskedSparse) { 24.0 } else { 100.0 };
        rasterize_path_clip(&mask_path(radius), Affine::identity(), RenderOptions::default(),
            &mut CoverageMaskMut::new(&mut mask_data, WIDTH, HEIGHT, WIDTH).unwrap(),
            &mut RenderWorkspace {
                edges: &mut edges, intersections: &mut intersections, cells: &mut cells,
                row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
            }).map_err(|error| format!("mask: {error:?}"))?;
    }

    let mut timings = {
        let mut render = || -> Result<(), String> {
            if matches!(operation, Operation::BuildMask) {
                return rasterize_path_clip(&path, Affine::identity(),
                    RenderOptions::default(),
                    &mut CoverageMaskMut::new(&mut mask_data, WIDTH, HEIGHT, WIDTH).unwrap(),
                    &mut RenderWorkspace {
                        edges: &mut edges, intersections: &mut intersections,
                        cells: &mut cells, row_offsets: &mut row_offsets,
                        edge_indices: &mut edge_indices,
                    }).map_err(|error| format!("mask: {error:?}"));
            }
            pixels.fill(0);
            let mut target = Pixmap::from_buffer(&mut pixels, WIDTH, HEIGHT, WIDTH * 4)
                .map_err(|error| format!("target: {error:?}"))?;
            match operation {
                Operation::Fill => render_solid(&path, Affine::identity(),
                    SRGBA::new(40, 120, 220, 192), RenderOptions::default(), &mut target,
                    &mut RenderWorkspace {
                        edges: &mut edges, intersections: &mut intersections,
                        cells: &mut cells, row_offsets: &mut row_offsets,
                        edge_indices: &mut edge_indices,
                    }),
                Operation::FillClipped => render_solid_clipped(&path, Affine::identity(),
                    SRGBA::new(40, 120, 220, 192),
                    Rect::from_ltrb(48.0, 104.0, 208.0, 152.0).unwrap(),
                    RenderOptions::default(), &mut target, &mut RenderWorkspace {
                        edges: &mut edges, intersections: &mut intersections,
                        cells: &mut cells, row_offsets: &mut row_offsets,
                        edge_indices: &mut edge_indices,
                    }),
                Operation::FillGradient => render_paint(&path, Affine::identity(),
                    &gradient, RenderOptions::default(), &mut target,
                    &mut RenderWorkspace {
                        edges: &mut edges, intersections: &mut intersections,
                        cells: &mut cells, row_offsets: &mut row_offsets,
                        edge_indices: &mut edge_indices,
                    }),
                Operation::FillRadial => render_paint(&path, Affine::identity(),
                    &radial, RenderOptions::default(), &mut target,
                    &mut RenderWorkspace {
                        edges: &mut edges, intersections: &mut intersections,
                        cells: &mut cells, row_offsets: &mut row_offsets,
                        edge_indices: &mut edge_indices,
                    }),
                Operation::FillConic => render_paint(&path, Affine::identity(),
                    &conic, RenderOptions::default(), &mut target,
                    &mut RenderWorkspace {
                        edges: &mut edges, intersections: &mut intersections,
                        cells: &mut cells, row_offsets: &mut row_offsets,
                        edge_indices: &mut edge_indices,
                    }),
                Operation::FillMasked | Operation::FillMaskedSparse =>
                    render_solid_masked(&path, Affine::identity(),
                    SRGBA::new(40, 120, 220, 192),
                    CoverageMask::new(&mask_data, WIDTH, HEIGHT, WIDTH).unwrap(),
                    RenderOptions::default(), &mut target, &mut RenderWorkspace {
                        edges: &mut edges, intersections: &mut intersections,
                        cells: &mut cells, row_offsets: &mut row_offsets,
                        edge_indices: &mut edge_indices,
                    }),
                Operation::BuildMask => unreachable!(),
                Operation::Stroke { round } => render_stroke_solid(&path, Affine::identity(),
                    SRGBA::new(40, 120, 220, 192), StrokePathOptions {
                        stroke: if round {
                            StrokeOptions::new(6.0).expect("valid comparison stroke")
                                .with_cap(LineCap::Round).with_join(LineJoin::Round)
                        } else {
                            StrokeOptions::new(6.0).expect("valid comparison stroke")
                        },
                        ..Default::default()
                    }, &mut target, &mut StrokeWorkspace {
                        points: &mut stroke_points, contours: &mut stroke_contours,
                        edges: &mut edges, intersections: &mut intersections,
                        cells: &mut cells, row_offsets: &mut row_offsets,
                        edge_indices: &mut edge_indices,
                    }),
            }.map_err(|error| format!("render: {error:?}"))
        };

        for _ in 0..warmup { render()?; }
        let mut timings = Vec::with_capacity(samples as usize);
        for _ in 0..samples {
            let started = Instant::now();
            for _ in 0..iterations { render()?; black_box(()); }
            timings.push(started.elapsed().as_nanos() as f64 / f64::from(iterations));
        }
        timings
    };
    timings.sort_by(f64::total_cmp);

    if matches!(operation, Operation::BuildMask) {
        for (pixel, &coverage) in pixels.chunks_exact_mut(4).zip(&mask_data) {
            pixel.fill(coverage);
        }
    }

    if let Some(path) = path_argument("--output")? {
        fs::write(path, &pixels).map_err(|error| format!("write output: {error}"))?;
    }

    println!("renderer,scene,width,height,samples,iterations,min_ns,median_ns,max_ns,checksum");
    println!("ugl-rs,{scene},{WIDTH},{HEIGHT},{samples},{iterations},\
        {:.3},{:.3},{:.3},{}", timings[0], timings[timings.len() / 2],
        timings[timings.len() - 1], checksum(&pixels));
    if let Some(path) = path_argument("--compare")? {
        let reference = fs::read(path).map_err(|error| format!("read reference: {error}"))?;
        compare("Blend2D_vs_ugl-rs-f32", scene, &reference, &pixels)?;
    }
    Ok(())
}

#[cfg(feature = "fixed")]
fn fixed_path(scene: &str) -> Path<ugl_rs::fixed::Scalar> {
    use ugl_rs::fixed::Scalar;
    if scene == "build_path_mask" { return fixed_mask_path(100.0); }
    let fixed = Scalar::from_num;
    let mut path = PathBuilder::new();
    match scene {
        "fill_rectangles_1" | "fill_rectangles_16" | "fill_rectangles_64" =>
        for index in 0..match scene {
            "fill_rectangles_1" => 1, "fill_rectangles_16" => 16, _ => SHAPES,
        } {
            let (x, y) = (fixed((index % 8) as f32 * 30.0 + 4.25),
                fixed((index / 8) as f32 * 30.0 + 4.5));
            path.move_to((x, y)).line_to((x + fixed(22.5), y))
                .line_to((x + fixed(22.5), y + fixed(21.75)))
                .line_to((x, y + fixed(21.75)));
        },
        "fill_rectangle_large" | "fill_rectangle_linear_gradient" |
        "fill_rectangle_radial_gradient" |
        "fill_rectangle_conic_gradient" |
        "fill_rectangle_path_mask" | "fill_rectangle_path_mask_sparse" => {
            path.move_to((fixed(16.25), fixed(20.5)))
                .line_to((fixed(239.5), fixed(20.5)))
                .line_to((fixed(239.5), fixed(235.25)))
                .line_to((fixed(16.25), fixed(235.25)));
        }
        "fill_triangles_64" => for index in 0..SHAPES {
            let (x, y) = (fixed((index % 8) as f32 * 30.0 + 4.25),
                fixed((index / 8) as f32 * 30.0 + 4.5));
            path.move_to((x, y + fixed(21.5))).line_to((x + fixed(11.25), y))
                .line_to((x + fixed(22.5), y + fixed(21.5)));
        },
        "stroke_polyline_32" | "stroke_polyline_round_32" => {
            path.move_to((fixed(8.0), fixed(128.0)));
            for index in 1..=32 {
                let y = if index & 1 == 0 { fixed(96.0) } else { fixed(160.0) };
                path.line_to((fixed(8.0 + index as f32 * 7.5), y));
            }
        }
        _ => {
            path.move_to((fixed(8.0), fixed(128.0)));
            for index in 0..8 {
                let x = fixed((8 + index * 30) as f32);
                let y = if index & 1 == 0 { fixed(112.0) } else { fixed(144.0) };
                path.cubic_to((x + fixed(10.0), y), (x + fixed(20.0), y),
                    (x + fixed(30.0), fixed(128.0)));
            }
        }
    }
    path.build()
}

#[cfg(feature = "fixed")]
fn fixed_mask_path(radius: f32) -> Path<ugl_rs::fixed::Scalar> {
    use ugl_rs::fixed::Scalar;
    let fixed = Scalar::from_num;
    let mut path = PathBuilder::with_capacity(6);
    let (radius, k) = (fixed(radius), fixed(radius * 0.552_284_7));
    let center = fixed(128.0);
    path.move_to((center + radius, center))
        .cubic_to((center + radius, center + k),
            (center + k, center + radius), (center, center + radius))
        .cubic_to((center - k, center + radius),
            (center - radius, center + k), (center - radius, center))
        .cubic_to((center - radius, center - k),
            (center - k, center - radius), (center, center - radius))
        .cubic_to((center + k, center - radius),
            (center + radius, center - k), (center + radius, center));
    path.build()
}

#[cfg(feature = "fixed")]
fn run_fixed() -> Result<(), String> {
    use ugl_rs::{
        fixed::{Scalar,
            canvas::{GeometryWorkspace, RenderOptions, StrokePathOptions, render_path,
                rasterize_path_clip as rasterize_fixed_path_clip, render_path_clipped,
                render_path_masked, render_stroke_path},
            raster::{Line, Segment, Trapezoid, Workspace},
            sampler::{Angle, ConicAngleMode as FixedConicAngleMode,
                ConicGradient as FixedConicGradient,
                LinearGradient as FixedLinearGradient,
                RadialGradient as FixedRadialGradient},
            stroke::Options as FixedStrokeOptions,
        },
        sampler::SolidPaint,
        stroke::{StrokeContour, StrokePathWorkspace},
    };

    let warmup = argument("--warmup", 500)?;
    let iterations = argument("--iterations", 5_000)?;
    let samples = argument("--samples", 9)?;
    if iterations == 0 || samples == 0 {
        return Err("--iterations and --samples must be positive".into());
    }
    let (scene, _, operation) = scene()?;
    let path = fixed_path(scene);
    let paint = SolidPaint::new(SRGBA::new(40, 120, 220, 192));
    let stop_values = [
        GradientStop::new(0.0, SRGBA::new(0, 0, 0, 32)),
        GradientStop::new(1.0, SRGBA::new(0, 0, 0, 224)),
    ];
    let mut gradient_ramp = [PremulSRGBA8::default(); 256];
    let gradient_stops = GradientStops::with_ramp(&stop_values, &mut gradient_ramp)
        .unwrap();
    let gradient = FixedLinearGradient::new(
        (Scalar::from_num(16), Scalar::from_num(128)),
        (Scalar::from_num(240), Scalar::from_num(128)),
        gradient_stops.encoded_ramp().unwrap(), SpreadMode::Pad).unwrap();
    let radial = FixedRadialGradient::new(
        (Scalar::from_num(128), Scalar::from_num(128)), Scalar::from_num(112),
        gradient_stops.encoded_ramp().unwrap(), SpreadMode::Pad).unwrap();
    let conic = FixedConicGradient::with_angle_mode(
        (Scalar::from_num(128), Scalar::from_num(128)), Angle::ZERO,
        gradient_stops.encoded_ramp().unwrap(), FixedConicAngleMode::Fast).unwrap();
    let mut pixels = vec![0; WIDTH as usize * HEIGHT as usize * 4];
    let (mut edges, mut lines) = (
        vec![Default::default(); EDGE_CAPACITY], vec![Line::default(); EDGE_CAPACITY]);
    let (mut segments, mut trapezoids) = (
        vec![Segment::default(); EDGE_CAPACITY], vec![Trapezoid::default(); EDGE_CAPACITY]);
    let (mut row_area, mut strip_offsets, mut strip_indices) = (
        vec![0; WIDTH as usize], vec![0; HEIGHT as usize + 1],
        vec![0; EDGE_CAPACITY]);
    let mut stroke_points = vec![Default::default(); 2048];
    let mut stroke_contours = vec![StrokeContour::default(); 16];
    let mut mask_data = vec![0; WIDTH as usize * HEIGHT as usize];
    if matches!(operation, Operation::FillMasked | Operation::FillMaskedSparse) {
        let mut geometry = GeometryWorkspace { edges: &mut edges, lines: &mut lines };
        let mut raster = Workspace {
            segments: &mut segments, trapezoids: &mut trapezoids,
            row_area: &mut row_area, strip_offsets: &mut strip_offsets,
            strip_indices: &mut strip_indices,
        };
        let radius = if matches!(operation, Operation::FillMaskedSparse) { 24.0 } else { 100.0 };
        rasterize_fixed_path_clip(&fixed_mask_path(radius), RenderOptions::default(),
            &mut CoverageMaskMut::new(&mut mask_data, WIDTH, HEIGHT, WIDTH).unwrap(),
            &mut geometry, &mut raster).map_err(|error| format!("mask: {error:?}"))?;
    }

    let mut timings = {
        let mut render = || -> Result<(), String> {
            if matches!(operation, Operation::BuildMask) {
                let mut geometry = GeometryWorkspace { edges: &mut edges, lines: &mut lines };
                let mut raster = Workspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area, strip_offsets: &mut strip_offsets,
                    strip_indices: &mut strip_indices,
                };
                return rasterize_fixed_path_clip(&path, RenderOptions::default(),
                    &mut CoverageMaskMut::new(&mut mask_data, WIDTH, HEIGHT, WIDTH).unwrap(),
                    &mut geometry, &mut raster).map_err(|error| format!("mask: {error:?}"));
            }
            pixels.fill(0);
            let mut target = Pixmap::from_buffer(&mut pixels, WIDTH, HEIGHT, WIDTH * 4)
                .map_err(|error| format!("target: {error:?}"))?;
            let mut geometry = GeometryWorkspace { edges: &mut edges, lines: &mut lines };
            let mut raster = Workspace {
                segments: &mut segments, trapezoids: &mut trapezoids,
                row_area: &mut row_area, strip_offsets: &mut strip_offsets,
                strip_indices: &mut strip_indices,
            };
            match operation {
                Operation::Fill => render_path(&path, &paint, RenderOptions::default(),
                    &mut target, &mut geometry, &mut raster),
                Operation::FillClipped => render_path_clipped(&path, &paint,
                    Rect::from_ltrb(48.0, 104.0, 208.0, 152.0).unwrap(),
                    RenderOptions::default(), &mut target, &mut geometry, &mut raster),
                Operation::FillGradient => render_path(&path, &gradient,
                    RenderOptions::default(), &mut target, &mut geometry, &mut raster),
                Operation::FillRadial => render_path(&path, &radial,
                    RenderOptions::default(), &mut target, &mut geometry, &mut raster),
                Operation::FillConic => render_path(&path, &conic,
                    RenderOptions::default(), &mut target, &mut geometry, &mut raster),
                Operation::FillMasked | Operation::FillMaskedSparse =>
                    render_path_masked(&path, &paint,
                    CoverageMask::new(&mask_data, WIDTH, HEIGHT, WIDTH).unwrap(),
                    RenderOptions::default(), &mut target, &mut geometry, &mut raster),
                Operation::BuildMask => unreachable!(),
                Operation::Stroke { round } => render_stroke_path(&path, &paint,
                    StrokePathOptions {
                    stroke: if round {
                        FixedStrokeOptions::new(Scalar::from_num(6))
                            .expect("valid comparison stroke")
                            .with_cap(LineCap::Round).with_join(LineJoin::Round)
                    } else {
                        FixedStrokeOptions::new(Scalar::from_num(6))
                            .expect("valid comparison stroke")
                    }, ..Default::default()
                }, &mut target, &mut StrokePathWorkspace {
                    points: &mut stroke_points, contours: &mut stroke_contours,
                }, &mut geometry, &mut raster),
            }.map_err(|error| format!("render: {error:?}"))
        };
        for _ in 0..warmup { render()?; }
        let mut timings = Vec::with_capacity(samples as usize);
        for _ in 0..samples {
            let started = Instant::now();
            for _ in 0..iterations { render()?; black_box(()); }
            timings.push(started.elapsed().as_nanos() as f64 / f64::from(iterations));
        }
        timings
    };
    timings.sort_by(f64::total_cmp);

    if matches!(operation, Operation::BuildMask) {
        for (pixel, &coverage) in pixels.chunks_exact_mut(4).zip(&mask_data) {
            pixel.fill(coverage);
        }
    }

    if let Some(path) = path_argument("--output")? {
        fs::write(path, &pixels).map_err(|error| format!("write output: {error}"))?;
    }
    println!("renderer,scene,width,height,samples,iterations,min_ns,median_ns,max_ns,checksum");
    println!("ugl-rs-fixed,{scene},{WIDTH},{HEIGHT},{samples},{iterations},\
        {:.3},{:.3},{:.3},{}", timings[0], timings[timings.len() / 2],
        timings[timings.len() - 1], checksum(&pixels));
    for (argument, label) in [("--compare", "Blend2D_vs_ugl-rs-fixed"),
                              ("--compare-f32", "ugl-rs-f32_vs_fixed")] {
        if let Some(path) = path_argument(argument)? {
            let reference = fs::read(path).map_err(|error| format!("read reference: {error}"))?;
            compare(label, scene, &reference, &pixels)?;
        }
    }
    Ok(())
}

fn run() -> Result<(), String> {
    if path_argument("--backend")?.as_deref().unwrap_or("f32") == "fixed" {
        #[cfg(feature = "fixed")] return run_fixed();
        #[cfg(not(feature = "fixed"))] return Err("fixed feature is disabled".into());
    }
    run_f32()
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => { eprintln!("compare_blend2d: {error}"); ExitCode::FAILURE }
    }
}
