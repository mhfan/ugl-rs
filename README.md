
![Build status](https://github.com/mhfan/ugl-rs/actions/workflows/rust-ci.yml/badge.svg)
[![codecov](https://codecov.io/gh/mhfan/ugl-rs/graph/badge.svg)](https://codecov.io/gh/mhfan/ugl-rs)
[![Crates.io](https://img.shields.io/crates/v/ugl-rs.svg)](https://crates.io/crates/ugl-rs)
[![dependency status](https://deps.rs/repo/github/mhfan/ugl-rs/status.svg)](https://deps.rs/repo/github/mhfan/ugl-rs)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](https://opensource.org/licenses/MIT)

# ugl-rs

`ugl-rs` aims to become an industrial-quality, deterministic, pure-Rust 2D
software rasterization core for embedded and otherwise constrained systems.
It is inspired by [**micro{gl}**](https://github.com/micro-gl/micro-gl), but is
designed around Rust ownership, explicit failure, caller-owned memory, and
testable rendering semantics rather than as a line-by-line port.

The intended niche is deliberately narrower than Blend2D, tiny-skia, Skia, or Vello:

- CPU-only rendering without requiring a GPU or FPU;
- a `no_std` core with optional allocation and no-allocation rasterization;
- caller-provided destination and scratch memory;
- a floating-point reference backend and a bounded fixed-point backend;
- deterministic output, bounded resource use, and no data-dependent panics;
- high-quality path filling, stroking, clipping, gradients, sampling, blending,
  and alpha compositing.

The project now has an allocation-free path-to-pixel vertical slice with
sampled and analytic `f32` rasterizers, premultiplied source-over, caller-owned
scratch storage, and an early Q24.8 fixed-point backend. Production fixed edge
binning, fuzzing, and broader golden/benchmark scenes are still under
development, so it is not yet suitable as a production renderer.

The current MSRV is Rust 1.93. CI checks MSRV and stable builds, independent
feature combinations, 32-bit Linux, and a Cortex-M target without an FPU.

## Direction

The first complete rendering path is intentionally small:

```text
Path
  -> curve flattening
  -> directed edges
  -> scan conversion and pixel coverage
  -> solid paint
  -> source-over compositing
  -> caller-owned RGBA8888 buffer
```

The `f32` implementation is the behavioral reference. Geometry containers are
generic over coordinates so that fixed-point values can reuse the scene
representation, but rasterization algorithms remain concrete until their
required operations, intermediate widths, rounding rules, and overflow
behavior are understood. This avoids premature numeric abstraction while
preserving the fixed-point migration path.

The normative rendering contract, architecture boundaries, implementation
order, and milestones are maintained in [DESIGN.md](DESIGN.md). External
renderer research and explicit adoption decisions are tracked in
[RESEARCH.md](RESEARCH.md).

## Benchmarking

Run the scalar rasterizer comparison with:

```text
cargo bench --bench raster --all-features
```

The baseline scene contains 64 fractional rectangles in a 256 × 256
premultiplied RGBA8888 target. Path construction, fixed-line preparation, and
all heap allocation happen before Criterion starts each measured iteration.
The measured loop clears the destination and performs scan conversion plus
source-over compositing.

Current development baseline, measured on 2026-07-30:

- commit: `5f98d43`;
- platform: Darwin arm64;
- compiler: `rustc 1.97.1`, LLVM 22.1.6;
- Criterion parameters: 1 s warm-up, 1 s measurement, 10 samples.

| Backend | Time estimate | Reported interval | Throughput |
| --- | ---: | ---: | ---: |
| sampled `f32` | 15.820 ms | 15.625–16.031 ms | 4.14 Mpixel/s |
| analytic `f32` | 213.10 µs | 205.48–235.39 µs | 307.53 Mpixel/s |
| Q24.8 fixed | 315.43 µs | 312.93–319.83 µs | 207.77 Mpixel/s |

These short-run numbers are a regression reference for this machine, not a
cross-platform ranking. Published comparisons should use longer measurements
and record CPU model, power state, compiler, and commit.

The initial caller-owned scratch budgets are:

| Backend | Edge/segment storage | Crossing storage | Row storage |
| --- | ---: | ---: | ---: |
| sampled `f32` | 128 `Edge` | 128 `Intersection` | 256 `f32` |
| analytic `f32` | 128 `Edge` | 128 `AnalyticIntersection` | 256 `f32` |
| Q24.8 fixed | 128 `FixedSegment` + 64 `FixedTrapezoid` | none | 256 `u64` |

Renderer allocation count inside the measured path is zero by API
construction: every mutable geometry, crossing, area, and destination buffer
is borrowed from the benchmark. Criterion's own allocations are outside that
contract.

## Non-goals for the core

The initial core does not include window-system integration, SVG parsing, image
decoding, text shaping, a GUI framework, or a 3D renderer. These can be separate
integration layers after the 2D rasterization core is correct and stable.

## References

* <https://2d.graphics>
* <https://github.com/savage13/agg>
* <https://github.com/blend2d/blend2d>
* <https://github.com/linebender/color>
* <https://github.com/linebender/peniko>
