
![Build status](https://github.com/mhfan/ugl-rs/actions/workflows/rust-ci.yml/badge.svg)
[![codecov](https://codecov.io/gh/mhfan/ugl-rs/graph/badge.svg)](https://codecov.io/gh/mhfan/ugl-rs)
[![Crates.io](https://img.shields.io/crates/v/ugl-rs.svg)](https://crates.io/crates/ugl-rs)
[![dependency status](https://deps.rs/repo/github/mhfan/ugl-rs/status.svg)](https://deps.rs/repo/github/mhfan/ugl-rs)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](https://opensource.org/licenses/MIT)

# ugl-rs

`ugl-rs` is a pre-release, deterministic, pure-Rust 2D software rasterization
core aimed at embedded and otherwise constrained systems. Its goal is
industrial quality, but broader fuzzing, golden-scene comparison, API
stabilization, and real-device validation are still required before production
use.
It is inspired by [**micro{gl}**](https://github.com/micro-gl/micro-gl), but is
designed around Rust ownership, explicit failure, caller-owned memory, and
testable rendering semantics rather than as a line-by-line port.

The intended niche is deliberately narrower than Blend2D, tiny-skia, Skia, or Vello:

- CPU-only rendering without requiring a GPU or FPU;
- a `no_std` core with optional allocation and no-allocation rasterization;
- caller-provided destination and scratch memory;
- an exact-area `f32` production path, a sampled reference rasterizer, and a
  bounded Q24.8 backend;
- deterministic output, bounded resource use, and no data-dependent panics;
- high-quality path filling, stroking, clipping, gradients, sampling, blending,
  and alpha compositing.

The implemented vertical slice covers allocation-free path filling, stroking,
dashing, rectangular and arbitrary-path clipping, gradients, and source-over
composition. Both main backends borrow destination and scratch storage:

- `float::canvas::render_*` functions use exact-area analytic `f32`
  coverage with sparse row bins;
- `float::canvas::render_*_sampled` is the slower supersampled reference used for
  differential testing;
- `fixed::*` provides checked Q24.8 geometry, native fixed paint, sparse-strip
  rasterization, and optional retained strip/tile coverage.

The fixed streaming path is the minimum-memory default. Retained strips and
16 × 16 tiles are explicit batching/caching options; current measurements do
not justify selecting tile construction for immediate rendering.

The current MSRV is Rust 1.93. CI checks MSRV and stable builds, independent
feature combinations, 32-bit Linux, and a Cortex-M target without an FPU.

## Current status

| Area | Status |
| --- | --- |
| `f32` fill and clipping | Exact-area production path plus sampled reference; allocation-free |
| Paint and color | Solid and gradient samplers; encoded compatibility and linear-light paths |
| Stroke | Allocation-free f32/fixed dashes, caps, joins, and path stroke pipelines implemented |
| Fixed point | Q24.8 transformed path fill/stroke, sparse strips/tiles, clipping, native fixed gradients, and all fixed caps/joins implemented |
| Facade | Owning f32 `Canvas` and `fixed::Canvas`, plus parallel `CanvasRef` APIs for bounded scratch |
| Production readiness | Pre-release: API stabilization, broader fuzzing/goldens, code-size work, and real-device validation remain |

The f32 dash reference accepts finite, strictly positive alternating on/off
lengths and a finite phase. Odd-length arrays repeat twice before the parity
cycle restarts. Each path contour restarts the normalized phase, and a dash
crossing a closed-contour seam is merged so that the seam receives a join
rather than two caps. Decomposition and stroke expansion use caller-owned
point, contour, and edge storage.

The fixed counterpart preserves the same contract with Q24.8 pattern lengths
and phase. It uses integer square roots and widened rational interpolation;
dash-state accumulation stays in integer subpixels and does not require an FPU.
Both implementations locate every cut from the original segment and cumulative
distance instead of repeatedly advancing rounded cut points. The f32 backend
returns `DashPrecisionExhausted` when a requested dash is too short to advance
at the segment's current magnitude rather than looping or silently dropping it.
`dash_requirements` and `fixed::dash::requirements` return exact point/contour
capacities. Both decomposition entry points run this preflight before writing,
so capacity and numeric errors leave caller-owned dash scratch untouched.

## Architecture at a glance

The rendering pipeline is intentionally explicit:

```text
Path
  -> curve flattening
  -> fill edges or stroke expansion
  -> directed edges
  -> scan conversion and pixel coverage
  -> optional rectangle/path clipping
  -> solid or gradient paint sampling
  -> source-over compositing
  -> caller-owned RGBA8888 buffer
```

The exact-area `f32` implementation is the primary floating-point renderer and
the behavioral reference for fixed differential tests. The sampled `f32`
rasterizer is a separate quality oracle, not the default canvas API. Geometry
containers are generic over coordinates, while raster algorithms stay concrete
where intermediate widths, rounding, overflow, and performance differ.

The normative rendering contract, architecture boundaries, implementation
order, and milestones are maintained in [DESIGN.md](DESIGN.md). External
renderer research and explicit adoption decisions are tracked in
[RESEARCH.md](RESEARCH.md).

## Quick start

The ordinary API owns and reuses scratch storage, planning any growth before a
draw can modify the destination:

```rust
use ugl_rs::{Canvas, common::{color::SRGBA, geometry::PathBuilder}};

const  WIDTH: u32 = 4;
const HEIGHT: u32 = 4;

let mut builder = PathBuilder::new();
builder.move_to((0.5, 0.5)).line_to((3.5, 0.5))
       .line_to((3.5, 3.5)).line_to((0.5, 3.5));
let path = builder.build();
let mut canvas = Canvas::new(WIDTH, HEIGHT).unwrap();
canvas.set_color(SRGBA::new(20, 200, 40, 160)).fill(&path).unwrap();
let pixels = canvas.target().as_bytes();
```

`Canvas::from_buffer` renders into caller-owned storage when integrating with a
window surface, framebuffer, or existing image. `target()` and `target_mut()`
provide dimensions and raw pixel access without exposing raster scratch.
`save()` and `restore()` preserve transform, paint, global alpha, stroke, fill,
and clip state. `set_global_alpha` uses the RGBA8 opacity range `0..=255` and
applies uniformly to solid and custom paints. Consecutive `set_clip_rect`,
`set_clip_mask`, and `set_clip_path` calls intersect with the current clip;
`clear_clip()` explicitly resets it.

`CanvasRef` is the allocation-free facade for callers that provide bounded
scratch explicitly. The lower-level `float::canvas::*` functions expose individual
workspace arrays only for static-memory systems, custom allocators, retained
coverage integration, and renderer development; they are not required for
ordinary drawing.

The core supports `no_std` and currently uses `alloc`. Default builds enable
`f32`, `fixed`, and `std`. The rendering backends are independently selectable:

- `--no-default-features --features fixed` builds the no_std Q24.8 renderer,
  omits the f32 renderer and floating samplers, and has no `libm` dependency;
- `--no-default-features --features f32` builds the complete no_std f32 backend
  and enables optional `libm`;
- adding `std` makes the f32 math dispatcher use platform implementations;
- `native-float` implies `f32` and selects hardware-friendly basic operations
  on explicitly supported no_std hard-float targets.

For the f32 backend, analytic rounding selects the math implementation independently: `std` uses native
platform floor/ceil, Arm hard-float (`eabihf`) targets automatically use an
FPU-friendly no_std implementation, and other no_std FPU targets can enable
`native-float`. Remaining soft-float builds retain `libm` operations. The
dependency's `arch` dispatch is enabled, so `libm` may select a tested target
implementation where one exists and otherwise retains its portable software
implementation. All floating math is routed through one internal backend:
`floor`, `ceil`, `sqrt`, remainder, power, and trigonometric calls no longer
select `libm` independently in rendering modules. MCU FPUs generally do not
implement transcendental functions, so no_std sin/cos/atan2/acos/pow—and sqrt
on targets without a matching `libm` architecture implementation—still use
portable software; a target-specific native backend must demonstrate correct
code generation before replacing them.

The fixed raster APIs can feed any existing `PaintSampler` through streaming,
retained-strip, or retained-tile coverage, with rectangle or borrowed path-mask
clipping. This gives functional parity on desktop, but does not claim a wholly
integer pipeline because those compatibility samplers use `f32`.
`fixed::sampler::PaintSampler` makes the no-FPU contract explicit.
`fixed::sampler::LinearGradient` projects Q24.8 endpoints with widened integer
arithmetic and samples a caller-owned encoded ramp; streaming and retained
strip/tile compositors support the same rectangle and path-mask adapters.
`fixed::sampler::RadialGradient` implements both concentric and general
two-circle/focal geometry with integer root solving and exact integer
spread/ramp mapping. `fixed::sampler::ConicGradient` uses a compact binary-turn
angle and a fixed 16-step integer CORDIC.

Fixed-only context, numeric helpers, sampler contracts, flattening,
rasterization, stroking, tiling, and their focused tests live under
`src/fixed/`. The canonical public API uses `fixed::*` paths directly. Backend
modules use concise names such as `fixed::raster::Workspace`,
`fixed::stroke::Options`, `fixed::Canvas`, and `fixed::CanvasRef`; no legacy crate-root
backend aliases are retained.

Source ownership follows backend boundaries: `src/common/` contains generic
geometry and backend-neutral color, coverage, target, and workspace protocols;
`src/float/` owns f32 math, edges, dash/stroke expansion, rasterization,
sampling, and facades; `src/fixed/` owns the corresponding Q24.8 pipeline.
Feature gates therefore live primarily at backend module boundaries instead of
being repeated around individual f32 functions in shared files.

Both exact-area f32 and Q24.8 fixed paths rasterize arbitrary path clips into
caller-owned `CoverageMaskMut` storage. A borrowed `CoverageMask` can then be
reused by fill, stroke, dash, streaming, retained-strip, or retained-tile
composition. Fixed mask production and native fixed mask consumption require
no FPU.

The fixed execution contract is deliberately per entry point:

| Fixed operation | No-FPU guarantee |
| --- | --- |
| geometry, flattening, stroke, dash, raster, strip/tile encoding | yes |
| `fixed::sampler::*` solid/linear/radial/conic paint | yes |
| path-mask production and native mask composition | yes |
| `fixed::Canvas` with native paint and rectangle/mask/path clip | yes |
| `fixed::CanvasRef` with native paint and rectangle/borrowed-mask clip | yes |
| rectangle clipping | yes; API coordinates and antialiased coverage use Q24.8/integer arithmetic |
| compatibility entry points accepting `float::sampler::PaintSampler` | no |

“Fixed backend” therefore describes geometry and coverage representation; a
complete no-FPU claim additionally requires a native fixed sampler and a clip
route marked `yes` above.

Color boundaries are explicit: solid paints and gradient stops accept straight
encoded `SRGBA<u8>`, while `Pixmap::pixel` returns only validated
`PremulSRGBA8`. `pixel_bytes` exposes the physical RGBA bytes unchanged.
Pixmap construction intentionally validates layout without scanning the image;
source-over callers are responsible for valid premultiplied destination data.

`float::context::CanvasRef` and `fixed::context::CanvasRef` provide parallel bounded
drawing APIs for the exact-area f32 and Q24.8 pipelines. They retain transform,
fill rule, flattening, stroke, solid color, and rectangle/mask clip state while
borrowing the target and bounded scratch storage. `fill_with`, `stroke_with`, and
`stroke_dashed_with` preserve static sampler dispatch; all low-level functions
remain available.

### API layers

Choose the narrowest layer that owns the required state:

- `Canvas` for ordinary f32 drawing with automatically managed scratch and
  retained path clips;
- `fixed::Canvas` for ordinary Q24.8 drawing with automatically managed scratch;
- `float::context::CanvasRef` or `fixed::context::CanvasRef` when scratch must be bounded
  and supplied by the caller;
- `float::canvas::render_*` for direct exact-area f32 rendering;
- `float::canvas::render_*_sampled` only as the supersampled reference;
- `float::linear` for a premultiplied linear-light working framebuffer;
- `fixed::canvas` for explicit Q24.8 streaming, retained strips, and tiles.

`Canvas::new` allocates its destination, while `Canvas::from_buffer` borrows an
existing one. It owns reusable raster scratch and grows it transactionally
before drawing. `CanvasRef` construction takes a
`float::context::Workspace` containing caller-owned slices; dash buffers may be empty
when dashed strokes are not used.

`Pixmap` is the compact encoded-premultiplied RGBA8 compatibility target;
`LinearPixmap` is the higher-quality premultiplied linear-light working target.
Both support owned `new(width, height)` and borrowed `from_buffer` storage, but
they are intentionally separate types: conversion and quantization happen only
through `LinearPixmap::encode_into` or its LUT/dirty variants. No generic pixel
format trait obscures which compositing domain is active.

### Workspace planning

Both backends expose exact, target-independent planners for fill, stroke, and
dash. The f32 entry points are `render_requirements`, `stroke_requirements`,
and `dashed_stroke_requirements`; fixed equivalents live under
`fixed::canvas`. `CanvasRef` methods with matching names apply the current
transform, fill/stroke state, and target dimensions.

Planning is deliberately staged. Exact stroke edges depend on actual curve
flattening and cap/join expansion, while exact row/strip indices depend on the
resulting edges. A planner therefore borrows geometry-only planning scratch
and never touches the destination. If that scratch is insufficient, it returns
the existing capacity error and a required lower bound; once sufficient, the
returned structure contains the complete exact render-time capacities.

`path_clip_requirements` uses the same fill pipeline and reports the workspace
needed by `rasterize_path_clip`. Mask pixel storage remains separately and
explicitly sized as `stride × height`.

### Arbitrary path clipping

`Canvas::set_clip_path` and `fixed::Canvas::set_clip_path` are the ordinary
free-path clipping APIs. They rasterize
and retain the antialiased mask in tightly packed non-zero bounds, intersecting
only that region with the current clip; subsequent fill, stroke, and
dashed-stroke calls apply it without exposing mask storage. Initial construction
derives conservative bounds from the prepared edges and rasterizes directly
into local coverage storage; it does not allocate a temporary canvas-sized mask.
`save`/`restore` scopes nested clips.

The bounded `CanvasRef` and low-level APIs deliberately use a two-stage operation
so image-sized storage and lifetime remain visible:

1. Rasterize any path into caller-owned `CoverageMaskMut` with
   `float::canvas::rasterize_path_clip` or
   `fixed::canvas::rasterize_path_clip`.
2. Borrow it with `as_mask()` and pass it to `float::context::CanvasRef::set_clip_mask`,
   or to a low-level `render_*_masked` function.

During rendering, shape and mask coverage are multiplied before paint
composition. The mask therefore preserves antialiased path boundaries and can
be reused across multiple draws. `set_clip_rect` is the cheaper direct path
for a single rectangle. There is currently no implicit clip stack; bounded
callers that need clip intersections must combine caller-owned masks explicitly.

## Benchmarking

These figures are regression baselines from one Darwin arm64 host, not universal
renderer rankings. Run the Rust benchmarks with:

```text
cargo bench --bench raster --all-features
```

The matched Blend2D harness is:

```text
benches/blend2d/run.sh /absolute/path/to/blend2d
```

See [the harness documentation](benches/blend2d/README.md) for scenes, timing
boundaries, normalization, versions, and CSV output. The baseline below used
nine 5,000-frame samples after 500 warm-up frames on 2026-08-01: ugl-rs
`a2f190c`, Blend2D `6dbc2cef`, AsmJit `0bd5787b`, rustc 1.97.1, and
macOS 15.6 arm64.

| Representative scene | f32 | fixed | Blend2D |
| --- | ---: | ---: | ---: |
| 1 fractional rectangle | 4.02 µs | 4.82 µs | 3.39 µs |
| 64 fractional rectangles | 59.51 µs | 106.70 µs | 33.20 µs |
| large linear gradient | 62.60 µs | 146.72 µs | 31.57 µs |
| large radial gradient | 114.12 µs | 339.56 µs | 41.10 µs |
| large conic gradient, Fast | 181.65 µs | 389.78 µs | 67.22 µs |
| sparse retained path mask | 6.06 µs | 7.74 µs | 29.77 µs¹ |
| build circular path mask | 20.51 µs | 46.78 µs | 9.04 µs |
| cubic fill under rectangle clip | 11.10 µs | 17.83 µs | 3.62 µs |
| cubic butt/miter stroke | 28.22 µs | 64.84 µs | 14.28 µs |
| 32-segment round stroke | 79.98 µs | 188.06 µs | 35.81 µs |

¹ Blend2D has no equivalent free-path Context clip; this row uses a retained
PRGB32 `DST_IN` pass and is not a native path-clip comparison.

The important conclusions are:

- Simple f32 fills are about 1.2–2.0× Blend2D; gradients and strokes are commonly
  2.0–2.8×. Coverage integration and scalar paint/composition remain the main
  desktop gaps.
- Fixed is generally 1.2–3.0× slower than f32 on this Apple CPU. That measures
  widened deterministic integer arithmetic on a desktop, not expected MCU
  throughput.
- f32 and fixed are byte-identical for the rectangle grid, linear gradient, and
  cubic fill. Other fixed scenes differ only near boundaries: the reported
  representative maximum is four code values, while radial/conic and stroke
  maxima are one.
- Sparse retained masks are already competitive because work follows cached
  non-zero bounds; dense paint and long spans benefit more from Blend2D's JIT
  vector pipelines.

Cold single-draw medians, measured in fresh processes, were 47–96 µs for f32,
69–284 µs for fixed, and 366–381 µs for Blend2D across representative fill,
curve, and gradient scenes. Blend2D pays first-use JIT compilation; ugl-rs has
no pipeline warm-up. These latency samples should not be mixed with warmed
throughput.

No numeric micro{gl} claim is made yet. Its cached triangle tessellation may
lead on stable simple meshes, while bounding-box overdraw and approximate AA
make complex or quality-matched results workload-dependent. The comparison
hypothesis, detailed stage profiles, quality tables, memory measurements,
rejected experiments, and optimization history are maintained in
[DESIGN.md](DESIGN.md).

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
