//! Stable external-comparison runner. Keep this scene in sync with
//! `benches/blend2d/blend2d_bench.cpp`.

use std::{env, fs, hint::black_box, process::ExitCode, time::Instant};
use ugl_rs::{
    analytic::Intersection,
    canvas::{Pixmap, RenderOptions, RenderWorkspace, render_solid},
    color::SRGBA,
    edge::Edge,
    geometry::{Affine, Path, PathBuilder},
};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 256;
const SHAPES: usize = 64;
const EDGE_CAPACITY: usize = SHAPES * 2;

fn scene() -> Path {
    let mut path = PathBuilder::with_capacity(SHAPES * 5);
    for index in 0..SHAPES {
        let x = (index % 8) as f32 * 30.0 + 4.25;
        let y = (index / 8) as f32 * 30.0 + 4.5;
        path.move_to((x, y)).line_to((x + 22.5, y))
            .line_to((x + 22.5, y + 21.75)).line_to((x, y + 21.75));
    }
    path.build()
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

fn compare(reference: &[u8], actual: &[u8]) -> Result<(), String> {
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
    println!("Blend2D_vs_ugl-rs,{changed_pixels},{},{:.6},{:.6},{maximum_error}",
        actual.len() / 4, changed_pixels as f64 * 400.0 / actual.len() as f64,
        total_error as f64 / actual.len() as f64);
    Ok(())
}

fn run() -> Result<(), String> {
    let warmup = argument("--warmup", 200)?;
    let iterations = argument("--iterations", 2_000)?;
    let samples = argument("--samples", 9)?;
    if iterations == 0 || samples == 0 {
        return Err("--iterations and --samples must be positive".into());
    }

    let path = scene();
    let mut pixels = vec![0; WIDTH as usize * HEIGHT as usize * 4];
    let mut edges = vec![Edge::default(); EDGE_CAPACITY];
    let mut intersections = vec![Intersection::default(); EDGE_CAPACITY];
    let mut row_coverage = vec![0.0; WIDTH as usize];
    let mut row_offsets = vec![0; HEIGHT as usize + 1];
    let mut edge_indices = vec![0; EDGE_CAPACITY];

    let mut timings = {
        let mut render = || -> Result<(), String> {
            pixels.fill(0);
            let mut target = Pixmap::from_buffer(&mut pixels, WIDTH, HEIGHT, WIDTH * 4)
                .map_err(|error| format!("target: {error:?}"))?;
            render_solid(&path, Affine::identity(), SRGBA::new(40, 120, 220, 192),
                RenderOptions::default(), &mut target, &mut RenderWorkspace {
                    edges: &mut edges, intersections: &mut intersections,
                    row_coverage: &mut row_coverage, row_offsets: &mut row_offsets,
                    edge_indices: &mut edge_indices,
                }).map_err(|error| format!("render: {error:?}"))
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

    if let Some(path) = path_argument("--output")? {
        fs::write(path, &pixels).map_err(|error| format!("write output: {error}"))?;
    }

    println!("renderer,scene,width,height,samples,iterations,min_ns,median_ns,max_ns,checksum");
    println!("ugl-rs,fill_rectangles_64,{WIDTH},{HEIGHT},{samples},{iterations},\
        {:.3},{:.3},{:.3},{}", timings[0], timings[timings.len() / 2],
        timings[timings.len() - 1], checksum(&pixels));
    if let Some(path) = path_argument("--compare")? {
        let reference = fs::read(path).map_err(|error| format!("read reference: {error}"))?;
        compare(&reference, &pixels)?;
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => { eprintln!("compare_blend2d: {error}"); ExitCode::FAILURE }
    }
}
