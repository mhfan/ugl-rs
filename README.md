
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
binning and persistent active edges now operate on caller-owned sparse strip
storage. The fixed backend can optionally retain compact sparse coverage
strips for batching or caching while keeping the lower-memory streaming sink
as its default. An optional 16 × 16 tile prototype now classifies empty, full,
and boundary regions. Direct tile-major raster output, fuzzing, and broader
golden/benchmark scenes are still under development, so it is not yet suitable
as a production renderer.

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

- commit: `6c8190e`;
- platform: Darwin arm64;
- compiler: `rustc 1.97.1`, LLVM 22.1.6;
- Criterion parameters: 2 s warm-up, 2 s measurement, 20 samples.

| Backend | Time estimate | Reported interval | Throughput |
| --- | ---: | ---: | ---: |
| sampled `f32` | 15.546 ms | 15.466–15.645 ms | 4.22 Mpixel/s |
| analytic `f32` | 206.34 µs | 204.09–209.84 µs | 317.62 Mpixel/s |
| Q24.8 fixed | 229.64 µs | 229.19–230.18 µs | 285.38 Mpixel/s |

The additional fixed distribution scenes measure 45.76 µs for 16 sparse
rectangles and 185.37 µs for 256 short-edge rectangles. Before strip binning
and persistent active edges, the same scenes measured 65.34 µs and 621.64 µs,
respectively. These numbers are a regression reference for this machine, not a
cross-platform ranking.

The initial caller-owned scratch budgets are:

| Backend | Edge/segment storage | Strip/crossing storage | Row storage |
| --- | ---: | ---: | ---: |
| sampled `f32` | 128 `Edge` | 128 `Intersection` | 256 `f32` |
| analytic `f32` | 128 `Edge` | 128 `AnalyticIntersection` | 256 `f32` |
| Q24.8 fixed | 128 `FixedSegment` + 64 `FixedTrapezoid` | one `u32` offset per strip plus one `u32` per line/strip overlap | 256 `u64` |

Renderer allocation count inside the measured path is zero by API
construction: every mutable geometry, crossing, area, and destination buffer
is borrowed from the benchmark. Criterion's own allocations are outside that
contract.

The optional retained fixed output groups only non-empty 16-row strips. Each
strip descriptor is 12 bytes and each uniform non-zero coverage run is 12
bytes (`u32` x/length plus `u8` row/coverage). It therefore does not impose a
full-frame mask, and callers choose an explicit bounded run capacity.

A focused raster-only comparison after `0c625fc` used the same machine and
20-sample/2-second Criterion settings. `stream` sends runs to a counting sink;
`encode` writes retained strips; `encode + replay` also walks them through that
sink. These measurements exclude color compositing:

| Scene | stream | encode | encode + replay |
| --- | ---: | ---: | ---: |
| 64 fractional rectangles | 196.98 µs | 201.46 µs | 203.13 µs |
| 16 sparse rectangles | 42.18 µs | 42.69 µs | 42.63 µs |
| 256 short-edge rectangles | 172.48 µs | 176.26 µs | 173.92 µs |

The retained form currently costs roughly 1–3% to produce in these scenes.
It stays optional: MCU callers can stream directly, while desktop/batched
callers can spend bounded memory to decouple rasterization from compositing.

The tile prototype converts retained strips into tile-major data. Empty tiles
are omitted, full tiles store no fine runs, and boundary tiles use 4-byte
tile-local runs behind 16-byte descriptors. Conversion uses one caller-owned
8-byte scratch piece per run/tile overlap and sorts independently inside each
16-row strip.

Focused 20-sample/2-second measurements after `27477ca` include fixed
rasterization, strip retention, and tile conversion:

| Scene | stream baseline | tile encode | tile encode + replay |
| --- | ---: | ---: | ---: |
| 64 fractional rectangles | 196.98 µs | 316.85 µs | 309.55 µs |
| 16 sparse rectangles | 42.18 µs | 48.25 µs | 47.04 µs |
| 256 short-edge rectangles | 172.48 µs | 243.54 µs | 241.66 µs |

The conversion prototype is therefore not a default immediate-mode path:
row-major-to-tile-major sorting is too expensive in dense scenes.

The follow-up direct path now links fine runs by active tile column while each
16-row raster strip is produced, then compacts only the touched columns. It
uses one 8-byte linked piece per run/tile overlap in the current strip plus
three `u32` arrays per tile column; no whole-frame strip buffer or fine-piece
sort is required. A 1-second/10-sample follow-up measured:

| Scene | stream baseline | direct tile encode | old strip→tile encode |
| --- | ---: | ---: | ---: |
| 64 fractional rectangles | 196.98 µs | 240.73 µs | 316.85 µs |
| 16 sparse rectangles | 42.18 µs | 43.12 µs | 48.25 µs |
| 256 short-edge rectangles | 172.48 µs | 187.73 µs | 243.54 µs |

Direct emission removes most conversion overhead and sharply reduces peak
scratch, while streaming remains the MCU/minimum-latency default. The next
desktop experiment added a tile-aware solid compositor that consumes full
tiles without expanding them back through `CoverageSink`. It remains
byte-equivalent but did not beat immediate streaming:

| Scene | streaming solid | direct tiled solid |
| --- | ---: | ---: |
| 64 fractional rectangles | 229.64 µs | 291.27 µs |
| 16 sparse rectangles | 45.76 µs | 49.07 µs |
| 256 short-edge rectangles | 185.37 µs | 210.28 µs |
| 16 aligned 32×32 rectangles | 86.62 µs | 111.95 µs |

Even full-tile-heavy immediate rendering does not yet amortize tile
construction. The tiled compositor therefore remains an explicit batching or
cached-coverage path; it is not selected automatically.

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
