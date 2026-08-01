
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

- unqualified `canvas::render_*` functions use exact-area analytic `f32`
  coverage with sparse row bins;
- `canvas::render_*_sampled` is the slower supersampled reference used for
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
| Facade | Parallel f32/fixed `Context` APIs for fill, stroke, dash, rectangle clip, and borrowed path masks |
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
use ugl_rs::{Canvas, color::SRGBA, geometry::PathBuilder};

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

`Context` is the allocation-free facade for callers that provide bounded
scratch explicitly. The lower-level `canvas::*` functions expose individual
workspace arrays only for static-memory systems, custom allocators, retained
coverage integration, and renderer development; they are not required for
ordinary drawing.

The core supports `no_std` and currently uses `alloc`. Default desktop builds
enable `std` plus the Q24.8 fixed backend. Use `--no-default-features` for the
smallest floating-point core, or add `fixed` explicitly for a no_std fixed
build. Analytic rounding selects the backend independently: `std` uses native
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
`fixed::stroke::Options`, and `fixed::context::Context`; no legacy crate-root
backend aliases are retained.

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
| `fixed::context::Context` with a native fixed sampler and no clip/mask clip | yes |
| rectangle clipping | no; the shared antialiased rectangle adapter uses `f32` |
| compatibility entry points accepting `sampler::PaintSampler` | no |

“Fixed backend” therefore describes geometry and coverage representation; a
complete no-FPU claim additionally requires a native fixed sampler and a clip
route marked `yes` above.

Color boundaries are explicit: solid paints and gradient stops accept straight
encoded `SRGBA<u8>`, while `Pixmap::pixel` returns only validated
`PremulSRGBA8`. `pixel_bytes` exposes the physical RGBA bytes unchanged.
Pixmap construction intentionally validates layout without scanning the image;
source-over callers are responsible for valid premultiplied destination data.

`context::Context` and `fixed::context::Context` provide parallel bounded
drawing APIs for the exact-area f32 and Q24.8 pipelines. They retain transform,
fill rule, flattening, stroke, solid color, and rectangle/mask clip state while
borrowing the target and bounded scratch storage. `fill_with`, `stroke_with`, and
`stroke_dashed_with` preserve static sampler dispatch; all low-level functions
remain available.

### API layers

Choose the narrowest layer that owns the required state:

- `Canvas` for ordinary f32 drawing with automatically managed scratch and
  retained path clips;
- `context::Context` or `fixed::context::Context` when scratch must be bounded
  and supplied by the caller;
- `canvas::render_*` for direct exact-area f32 rendering;
- `canvas::render_*_sampled` only as the supersampled reference;
- `canvas_linear` for a premultiplied linear-light working framebuffer;
- `fixed::canvas` for explicit Q24.8 streaming, retained strips, and tiles.

`Canvas::new` allocates its destination, while `Canvas::from_buffer` borrows an
existing one. It owns reusable raster scratch and grows it transactionally
before drawing. Context construction takes a
`context::Workspace` containing caller-owned slices; dash buffers may be empty
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
`fixed::canvas`. Context methods with matching names apply the current
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

`Canvas::set_clip_path` is the ordinary free-path clipping API. It rasterizes
and retains the antialiased mask in internal storage, intersecting it with the
current clip; subsequent fill, stroke, and dashed-stroke calls apply it without
exposing mask storage. `save`/`restore` scopes nested clips.

The bounded Context and low-level APIs deliberately use a two-stage operation
so image-sized storage and lifetime remain visible:

1. Rasterize any path into caller-owned `CoverageMaskMut` with
   `canvas::rasterize_path_clip` or
   `fixed::canvas::rasterize_path_clip`.
2. Borrow it with `as_mask()` and pass it to `context::Context::set_clip_mask`,
   or to a low-level `render_*_masked` function.

During rendering, shape and mask coverage are multiplied before paint
composition. The mask therefore preserves antialiased path boundaries and can
be reused across multiple draws. `set_clip_rect` is the cheaper direct path
for a single rectangle. There is currently no implicit clip stack; bounded
callers that need clip intersections must combine caller-owned masks explicitly.

## Benchmarking

The figures below are development regression baselines from one Darwin arm64
host, not cross-renderer rankings. Each subsection states what is included in
its timed loop; compare only measurements with compatible scenes and settings.

### Running the benchmarks

Run the scalar rasterizer comparison with:

```text
cargo bench --bench raster --all-features
```

Set `UGL_SPAN_STATS=1` to print the canonical scene's non-timed analytic span
distribution. A filter which selects no benchmark avoids running Criterion:

```text
UGL_SPAN_STATS=1 cargo bench --bench raster --all-features -- '^$'
```

Run only the paint-sampler comparison with:

```text
cargo bench --bench raster --all-features -- paint_sample_rgba8888
```

### Blend2D comparison

The reproducible third-party harness compares only Blend2D and ugl-rs; no
results from unrelated renderers are mixed into this baseline. Build and run it
with:

```text
benches/blend2d/run.sh /absolute/path/to/blend2d
```

See [`benches/blend2d/README.md`](benches/blend2d/README.md) for the exact
scene, timing boundary, sampling protocol, image normalization, and required
version metadata. The current three-backend baseline was measured on 2026-08-01
after ugl-rs `5a1ddb3`, using Blend2D
`6dbc2cefbc996379e07104e34519a440b49b15d7`, and AsmJit
`0bd5787b54b575ed94bf32ac452153b34385c514`, built with Apple Clang 17 and
rustc 1.97.1 on macOS 15.6 arm64. Nine 5,000-frame samples after 500 warm-up
frames produced:

| Scene | f32 median | fixed median | Blend2D median | Blend2D vs f32 | fixed vs f32 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 fractional rectangle, fill | 4.03 µs | 4.38 µs | 3.97 µs | 1.01× faster | 1.09× slower |
| 16 fractional rectangles, fill | 17.31 µs | 28.42 µs | 11.53 µs | 1.50× faster | 1.64× slower |
| 64 fractional rectangles, fill | 59.31 µs | 108.87 µs | 34.13 µs | 1.74× faster | 1.84× slower |
| large fractional rectangle, fill | 22.22 µs | 29.26 µs | 14.38 µs | 1.55× faster | 1.32× slower |
| large rectangle, linear gradient | 63.81 µs | 149.07 µs | 31.85 µs | 2.00× faster | 2.34× slower |
| large rectangle, radial gradient | 115.82 µs | 345.51 µs | 41.44 µs | 2.80× faster | 2.98× slower |
| large rectangle, conic gradient (Fast) | 184.31 µs | 356.96 µs | 68.38 µs | 2.70× faster | 1.94× slower |
| large rectangle, sparse retained path mask | 5.80 µs | 7.46 µs | 31.12 µs¹ | 5.37× slower | 1.29× slower |
| large rectangle, dense retained path mask | 23.39 µs | 31.81 µs | 30.31 µs¹ | 1.30× slower | 1.36× slower |
| build circular path mask | 21.09 µs | 46.41 µs | 9.55 µs | 2.21× faster | 2.20× slower |
| 64 triangles, fill | 65.95 µs | 130.68 µs | 33.62 µs | 1.96× faster | 1.98× slower |
| 8 gentle cubic arches, fill | 13.73 µs | 20.68 µs | 8.27 µs | 1.66× faster | 1.51× slower |
| cubic fill under rectangle clip | 10.82 µs | 17.54 µs | 3.54 µs | 3.06× faster | 1.62× slower |
| cubic arches, width-6 butt/miter stroke | 29.44 µs | 64.71 µs | 14.35 µs | 2.05× faster | 2.20× slower |
| 32-segment polyline, butt/miter stroke | 62.12 µs | 133.42 µs | 25.71 µs | 2.42× faster | 2.15× slower |
| 32-segment polyline, round stroke | 78.27 µs | 181.42 µs | 34.51 µs | 2.27× faster | 2.32× slower |

| Scene | f32 pixels changed from Blend2D | fixed pixels changed from f32 | fixed mean/max error from f32 |
| --- | ---: | ---: | ---: |
| large rectangle | 0.343% | 0% | 0 / 0 |
| linear gradient rectangle | 10.800% | 0% | 0 / 0 |
| radial gradient rectangle | 11.160% | 0.098% | 0.00024 / 1 |
| conic gradient rectangle (Fast) | 13.873% | 0.003% | 0.000008 / 1 |
| sparse retained path mask | 0.093% | 0.130% | 0.00075 / 2 |
| dense retained path mask | 0.505% | 0.764% | 0.00539 / 3 |
| built path mask | 0.529% | 0.801% | 0.01152 / 4 |
| rectangle grid | 2.246% | 0% | 0 / 0 |
| triangles | 2.637% | 0.195% | 0.00147 / 1 |
| cubic fill | 0.452% | 0% | 0 / 0 |
| clipped cubic fill | 0.301% | 0% | 0 / 0 |
| cubic stroke | 1.321% | 0.311% | 0.00171 / 1 |
| polyline stroke | 3.024% | 0.865% | 0.00442 / 1 |
| round polyline stroke | 3.267% | 0.752% | 0.00398 / 1 |

Cold first-frame latency uses nine independent processes per scene with zero
warm-up and one timed draw. It retains first-use pipeline JIT in Blend2D while
path/image/context construction and ugl-rs scratch allocation remain outside
the timer:

| Scene | f32 median | fixed median | Blend2D median |
| --- | ---: | ---: | ---: |
| large fractional rectangle | 47.38 µs | 144.25 µs | 365.88 µs |
| 8 gentle cubic arches | 49.21 µs | 68.92 µs | 371.54 µs |
| linear-gradient rectangle | 96.25 µs | 284.29 µs | 381.04 µs |

These process-level samples describe latency, not steady-state throughput.
Blend2D's first pipeline compilation makes its median roughly 4–8× the f32
draw, while ugl-rs has no JIT warm-up. OS scheduling and code-page state make
the cold range noisier; warmed 9×5,000 medians remain the throughput baseline.

For the warmed 64-rectangle process, macOS `time -l` reported 2.89 MiB peak
RSS for Blend2D, 1.92 MiB for the f32 runner, and 2.55 MiB for the fixed runner.
The static Blend2D harness executable was 1.87 MiB; the Rust comparison binary
was 0.58 MiB but contains both f32 and fixed code. These are process/harness
diagnostics—including runtime, allocator, and JIT state—not renderer-owned
memory or minimum deployment sizes. Bounded deployments should use the exact
workspace planners; a future pure-fixed feature gate is required before a
fixed-only code-size claim is meaningful.

The fixed results are reported separately: f32 versus Blend2D measures desktop
competitiveness, while fixed versus f32 measures the Q24.8 cost and output
delta. Same-host fixed versus Blend2D numbers are not evidence about an MCU or
a no-FPU target. Rectangle output is byte-identical between the ugl-rs
backends; the gentle curves differ only by one code value at a small number of
boundary pixels.

The harness explicitly aligns butt caps, miter-bevel joins, and miter limit 4,
since Blend2D's default miter-clip join does not match ugl-rs. More strongly
inflected cubic strokes currently make the fixed backend return
`CrossingEdges`; they remain a production-reliability task rather than being
timed as if all backends supported the same input. Stroke still includes curve
flattening and outline construction on every draw, while Blend2D uses its
production stroker and JIT raster pipeline. Compact fixed outline emission
reduced the cubic-stroke median from 284.75 to 76.89 µs without increasing its
maximum f32 delta beyond one code value. Long round strokes remain expensive
because fixed arc construction and the resulting edge count are still scalar.

The large solid span and rectangle-clip scenes expose the next structural
gaps. Pairwise packed scalar source-over improved every f32 scene by roughly
5–10%, but long encoded RGBA8 spans remain far behind Blend2D's JIT vector
compositor. Integer rectangle clips cache their classification and bounds once,
bypass per-pixel coverage multiplication, and constrain analytic/fixed row and
cell processing to the conservative clipped domain. Integer clips then pass the
compositor directly to the bounded rasterizer, removing the adapter branch from
every emitted span. This reduced the matched clipped cubic from 19.26 to
10.82 µs for f32 and from 31.08 to 17.54 µs for fixed without changing either
checksum.
Memory and cold-start/JIT cost still require separate matched scenes.

The nested-prefix 1/16/64 rectangle series separates fixed frame overhead from
per-shape scaling. Direct vertical-run emission reduced the original f32
4.26/23.23/83.67 µs baseline; the current synchronized run is
4.03/17.31/59.31 µs and its 64-shape gap is 1.74×.
Fixed vertical-trapezoid boundary area reduced its raster-only stage from
203.61 to 144.04 µs. Direct disjoint-trapezoid emission subsequently brings
the complete fixed 1/16/64 draws to 4.38/28.42/108.87 µs. The f32 and fixed
outputs remain byte-identical. Event-free f32 rows
with disjoint sloped spans now integrate their boundary cells directly and
omit the empty gaps from cell clearing and prefix scanning; touching,
crossing, or partial-height rows retain the general analytic-cell path. Fixed
full rows use the analogous Q24.8 piecewise integral when trapezoid pixel
envelopes are disjoint; overlapping, crossing, and partial-height slabs retain
the exact rational/polygon accumulator.

The matched horizontal linear gradient uses a 256-entry encoded ramp and black
stops whose alpha changes from 32 to 224, avoiding ambiguity from different RGB
interpolation spaces. Batched affine span stepping plus direct full-coverage
composition reduced f32 from 381.33 to 192.50 µs. Direct Pad-ramp traversal
then reduced it to 130.14 µs; direct vertical coverage now brings the complete
f32 draw to 63.81 µs. Direct fixed trapezoid emission and a checked i64 span
projection reduce the fixed result to 149.07 µs. Both
ugl-rs backends are byte-identical; their one-code-value delta from Blend2D is
its gradient quantization rule. The remaining 2.02× desktop gap is dominated
by scalar ramp lookup and per-pixel writes rather than coverage.

The retained path-mask scenes exclude mask construction. Equal mask runs are
scanned eight bytes at a time and vertical coverage is emitted directly. f32
now measures 5.44 µs for a radius-24 sparse mask and 22.14 µs for the existing
radius-100 mask; fixed measures 7.46 and 31.81 µs. `CoverageMask` derives and
caches its non-zero bounds during retained-resource setup, so both rasterizers
visit only that domain; the f32 sink still uses word-wise zero-run filtering
inside it. ¹ Blend2D's roughly
30 µs result is an explicitly labeled equivalent implemented by drawing the
shape and applying a retained
PRGB32 mask with `DST_IN`; Blend2D exposes no free-path Context clip, so this
includes an extra image pass and is not evidence for a native path-mask API.
Building the same mask costs 21.09 µs for f32, 46.41 µs for fixed, and
9.55 µs for Blend2D; normalization to RGBA is outside the timed region. Direct
disjoint-row emission substantially closes the former rasterization gap; the
remaining cost is still separate from retained-mask composition.

#### f32 stroke stage profile

The matched gentle cubic stroke expands to 65 centerline points, one contour,
and 130 directed outline edges. Run its internal stage profile with:

```text
cargo bench --bench raster --all-features -- stroke_stages_f32
```

A focused release diagnostic on the same host produced these central estimates:

| Stage | Time |
| --- | ---: |
| centerline curve flatten | 1.83 µs |
| stroke outline expansion | 0.96 µs |
| sparse row bin construction | 1.16 µs |
| analytic coverage integration and run emission | 22.52 µs sparse cells |
| analytic coverage plus encoded blending | 29.91 µs |
| complete clear + flatten + stroke + encoded composite | 34.66 µs |

The independently measured stages are not strictly additive, but they locate
the dominant cost: flatten, outline, and binning total about 4 µs, while
coverage plus encoded blending remains about 30 µs. A prepared-stroke
API can still remove repeated geometry work for retained content, but it cannot
close the measured Blend2D gap by itself. Active-edge processing, slab event
handling, area integration, and emitted-run cost therefore take priority;
compositing and clear account for most of the remaining residual.

#### Analytic pipeline status

The production path bins edge starts by row, retains ordered active edges, and
splits slabs only at edge endpoints or real crossings. Boundary cells use a
closed-form integral of `clamp(edge_x - cell_x, 0, 1)`; guaranteed-full spans
use two range-delta writes. Clearing and run emission are restricted to the
touched x range, so row scratch remains one 8-byte `Cell` per target column.

Ordering is updated only for newly activated edges and actual crossings. A
midpoint check preserves the numeric contract for crossings within the event
tolerance, while a cold split-integral path handles pairs that still reverse
inside one slab. The sparse-cell implementation is checked against both the
dense analytic reference and deterministic high-sample randomized paths for
NonZero, EvenOdd, coincident, and self-intersecting geometry.

Open non-degenerate strokes emit one boundary contour rather than overlapping
segment and join polygons. This reduces the matched cubic stroke from 480 to
130 edges and is responsible for most of its current end-to-end gain. Rows with
an unchanged all-vertical active set reuse the preceding sparse-cell coverage,
but still replay runs for the new y so clipping and compositing semantics remain
unchanged.

The latest Time Profiler trace attributes about 79% of the cubic stroke samples
to analytic rasterization, 6% to solid blending, 3.5% to curve flattening,
2.5% to outline construction, and roughly 1% to row-bin sorting. Further work
should target coverage batching and long-span composition rather than more
row-bin sorting special cases. Rejected experiments and historical measurements
are summarized in [`DESIGN.md`](DESIGN.md), not duplicated here.

The stripped example executables were 448,176 bytes for ugl-rs and 1,965,280
bytes for statically linked Blend2D on this build. Those numbers describe the
complete harness binaries, not the incremental library contribution, and must
not be presented as a like-for-like library code-size result.

### Paint sampling and gradient kernels

The paint benchmark directly samples 65,536 device-space pixel centers and
accumulates the resulting premultiplied RGBA channels. It excludes path
processing, rasterization, clipping, destination writes, and compositing.
The original encoded-domain baseline at commit `ad3906f`, measured on 2026-07-30 with
`rustc 1.97.1`/LLVM 22.1.6 on Darwin arm64 using Criterion's default 3-second
warm-up, 5-second measurement, and 100 samples, is:

| Paint | Time estimate | Reported interval | Throughput |
| --- | ---: | ---: | ---: |
| solid | 287.33 µs | 277.61–298.01 µs | 228.09 Mpixel/s |
| linear | 500.50 µs | 487.76–517.64 µs | 130.94 Mpixel/s |
| two-circle radial | 1.3429 ms | 1.3189–1.3844 ms | 48.80 Mpixel/s |
| conic | 1.1343 ms | 1.1126–1.1625 ms | 57.78 Mpixel/s |

After moving gradient interpolation to linear light, the high-throughput path
uses a caller-owned 1024-entry encoded premultiplied ramp. The same machine and
Criterion settings measured the 2026-07-31 working tree as:

| Paint | Time estimate | Reported interval | Throughput |
| --- | ---: | ---: | ---: |
| solid | 259.12 µs | 248.92–280.20 µs | 252.92 Mpixel/s |
| linear | 269.32 µs | 263.44–278.85 µs | 243.33 Mpixel/s |
| two-circle radial | 894.45 µs | 876.32–917.27 µs | 73.27 Mpixel/s |
| conic | 582.08 µs | 567.54–602.77 µs | 112.59 Mpixel/s |

`GradientStops::new` remains the exact linear-light reference path;
`GradientStops::with_ramp` is the measured path. The ramp is prepared outside
the timed loop. Its nearest-entry sampling keeps tested smooth-gradient output
within one RGBA8 code value per channel of the exact path.

Linear framebuffers have a separate `GradientStops::with_linear_ramp` path.
Its caller-owned entries remain premultiplied linear `f32`, so it removes
per-sample stop lookup without an encoded round trip. A 1024-entry ramp costs
16 KiB, versus 4 KiB for the encoded RGBA8 ramp; `GradientStops::new` remains
the smaller exact path for MCU/reference use. A short Criterion diagnostic on
2026-07-31 measured the linear-gradient sampler at about 212.6 µs with the
linear ramp versus 627.4 µs with exact stop lookup over 65,536 samples
(approximately 3.0× faster).

The linear sampler also exposes allocation-free affine span sampling.
`LinearGradient` computes one start parameter and one per-pixel step;
`TransformedPaint` maps both into paint space once per span. A short diagnostic
measured 65,536 ramp samples at about 200.3 µs through the span path versus
211.6 µs point-by-point. In the 64-rectangle analytic render benchmark, span
stepping reduced linear-gradient rendering from about 443.1 µs to 409.2 µs
(approximately 7.7%).

Concentric radial gradients use a dedicated distance-squared recurrence and
one square root per sample, bypassing the general two-circle quadratic solver.
Four independent recurrence values are scheduled together so LLVM can overlap
the square roots without changing their scalar recurrence order. In the matched
large-gradient draw this reduced the f32 median from 123.98 to 115.82 µs with
the same output checksum; the remaining 2.80× Blend2D gap requires wider SIMD
sampling/composition rather than more coordinate algebra.
A short diagnostic measured 65,536 samples at about 467.4 µs through the span
path versus 690.4 µs point-by-point (approximately 32%). The 64-rectangle
analytic render measured about 903.1 µs versus 1.231 ms (approximately 27%).
The specialized path is checked against point sampling across 512-sample spans,
transforms, center crossings, and all spread modes with a maximum linear-channel
tolerance of `1e-4`. Non-concentric radial and conic paints retain their general
point-sampling fallback.

The native fixed sampler benchmark uses the same 65,536 pixel centers and a
caller-owned 1024-entry encoded ramp. A short 10-sample diagnostic on
2026-07-31 measured `fixed::sampler::LinearGradient` at about 423.3 µs
(154.8 Mpixel/s) and concentric `fixed::sampler::RadialGradient` at about
689.4 µs (95.1 Mpixel/s).
The radial implementation selects a `u64` integer-square-root and `i64` ramp
mapping fast path for ordinary device coordinates, with widened arithmetic
retained for the full public coordinate range. Before that specialization the
same radial diagnostic measured about 1.78 ms.

The general fixed two-circle path uses the same largest-valid-root policy as the
`f32` reference and retains up to 16 adaptive fractional square-root bits. A
short 10-sample diagnostic measured about 2.23 ms (29.4 Mpixel/s), improved
from about 7.03 ms after keeping ordinary discriminants and ramp division on
proved `u64`/`i64` paths. This remains a scalar reference baseline rather than
a final MCU performance target.

The native fixed conic diagnostic measured about 2.42 ms (27.1 Mpixel/s).
Its 16-step shift/add CORDIC stays below `6e-6` turn of angular error on the
tested integer grid, and the encoded-ramp differential test permits at most one
adjacent entry of error versus exact `atan2f`. This is also a scalar no-FPU
baseline; an octant LUT or platform-specific implementation requires benchmark
and code-size evidence before replacing it.

Conic gradients keep exact `atan2f` as the default and expose
`ConicAngleMode::Fast` as an explicit quality/performance choice. Fast mode
uses the same Sollya-generated seventh-degree unit-angle polynomial as
[Skia's CPU raster pipeline](https://skia.googlesource.com/skia/+/084fa9d8601a7f7895fc64efad3035098107d319/src/opts/SkRasterPipeline_opts.h#3152).
An exhaustive 65,536-angle test measured at most `2.66e-5` turns of circular
error. That bound can move a discontinuous seam by the same amount, so fast
mode is never selected implicitly. A short diagnostic measured 65,536 linear
conic samples at about 486.6 µs versus 603.7 µs for exact evaluation
(approximately 19% faster); the 64-rectangle analytic render measured about
691.5 µs versus 799.8 µs (approximately 14% faster).

The linear sampler contract also propagates conservative opacity metadata.
When coverage is full and every possible sample has alpha exactly one, the
compositor writes sampled pixels directly instead of reading the destination
and evaluating source-over. Fractional antialiasing coverage always retains
the general compositor. In the opaque linear-gradient 64-rectangle diagnostic,
this reduced rendering from about 397.3 µs to 215.9 µs (approximately 46%).
This store-only span is also the first SIMD-ready kernel boundary: future
platform backends can batch sampling and stores without coupling paint
evaluation to destination loads.

Linear-premultiplied arithmetic now uses its closed-domain invariant instead of
revalidating and clamping all four channels after every scale, interpolation,
and source-over operation. In short diagnostics this reduced the translucent
solid 64-rectangle render from about 156.7 µs to 112.6 µs (approximately 28%)
and its translucent linear-gradient counterpart from about 409.2 µs to
198.5 µs (approximately 51%). Two arm64 NEON experiments were slower than this
scalar kernel (about 116.6 µs for channel-vector packing and 124.5 µs for four
interleaved pixels), so neither was retained. SIMD is deferred until spans are
long enough to amortize layout conversion or a tile-local structure-of-arrays
working buffer exists.

The canonical scene currently emits 4,416 runs covering 33,856 pixels. Mean
run length is 7.67 pixels: 2,944 runs are one-pixel antialiasing boundaries and
1,472 are 16–31 pixels long (maximum 21). Although only 30.4% of runs have full
coverage, they contain 83.4% of covered pixels. This supports specialized
full-coverage kernels, but not converting every short boundary run to SoA.

These are scalar reference costs, not optimized paint targets. In particular,
the general radial sampler performs stable two-circle root solving per pixel;
future specialized concentric/span-stepping paths must retain byte-equivalent
tests and report their code-size and memory costs.

### Raster and compositing baselines

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

The linear-light compositor baseline was added on 2026-07-31 and measured on
the same Darwin arm64 host with Criterion's default 3-second warm-up, 5-second
measurement, and 100 samples:

| Analytic color path | Time estimate | Reported interval | Throughput |
| --- | ---: | ---: | ---: |
| encoded RGBA8888 compatibility | 123.87 µs | 123.66–124.06 µs | 529.09 Mpixel/s |
| linear `f32` working buffer | 155.88 µs | 155.16–156.71 µs | 420.44 Mpixel/s |
| linear + 4096-entry LUT presentation | 357.79 µs | 357.08–358.63 µs | 183.17 Mpixel/s |
| linear + adaptive dirty tracking, dense scene | 381.27 µs | 379.44–384.42 µs | 171.89 Mpixel/s |
| linear + exact `powf` presentation | 4.2181 ms | 4.2036–4.2368 ms | 15.54 Mpixel/s |

The working-buffer row includes clearing and compositing but no presentation.
Both presentation rows encode the complete 256 × 256 target after rendering;
the LUT is prepared before the timed loop and stays within one RGBA8 code value
per channel of the exact transfer path in the boundary tests.

For an incremental scene containing one 22.5 × 21.75 rectangle, full-frame LUT
presentation measured 69.52 µs while adaptive 16 × 16 dirty-tile presentation
measured 12.52 µs, a 5.55× reduction. On the dense 64-rectangle scene, tracking
adds about 6.6%; the adaptive presenter switches to a contiguous full-frame
encode when at least half the tile area is dirty, but span-marking still has a
cost. Callers which know every frame is dense should use the non-tracking
constructor and full presentation APIs.

The additional fixed distribution scenes measure 45.76 µs for 16 sparse
rectangles and 185.37 µs for 256 short-edge rectangles. Before strip binning
and persistent active edges, the same scenes measured 65.34 µs and 621.64 µs,
respectively. These numbers are a regression reference for this machine, not a
cross-platform ranking.

### Scratch memory and allocation

The initial caller-owned scratch budgets are:

| Backend | Edge/segment storage | Strip/crossing storage | Row storage |
| --- | ---: | ---: | ---: |
| sampled `f32` | 128 `Edge` | 128 `Intersection` | 256 `f32` |
| analytic `f32` | 128 `Edge` | 257 `u32` row offsets + 128 `u32` edge indices + 128 `Intersection` | 256 8-byte `Cell` values |
| Q24.8 fixed | 128 `fixed::raster::Segment` + 64 `fixed::raster::Trapezoid` | one `u32` offset per strip plus one `u32` per line/strip overlap | 256 `u64` |

The compact target uses 4 bytes per pixel. `LinearPixmap` deliberately uses
16 bytes per pixel (`LinearPremulRGBA<f32>`) and its fast presentation path
borrows an additional 4096-byte sRGB encoding LUT. This desktop-quality working
buffer is not the intended MCU storage path. Optional dirty tracking costs one
bit per 16 × 16 tile: 32 bytes for a 256 × 256 target.

Renderer allocation count inside the measured path is zero by API
construction: every mutable geometry, crossing, area, and destination buffer
is borrowed from the benchmark. Criterion's own allocations are outside that
contract.

### Stroke and active-edge scalability

The end-to-end stroke groups can be reproduced with:

```text
cargo bench --bench raster --all-features -- stroke_rgba8888
cargo bench --bench raster --all-features -- stroke_stages_f32
```

`stroke_rgba8888` clears a 256 × 256 destination and measures path flattening,
stroke expansion, analytic rasterization, and solid source-over composition.
`stroke_stages_f32` separates flattening, compact outline construction, row
binning, coverage, and coverage-plus-blending. Path construction and scratch
allocation remain outside every timed loop. Benchmark identifiers include the
exact `points/contours/edges` capacity, preventing results for an obsolete
outline representation from being mistaken for the current 130-edge cubic
scene. The synchronized Blend2D table above is the authoritative whole-pipeline
baseline.

The no-FPU `stroke_expand_fixed` group measures a 64-point Q24.8 zig-zag with
square caps and miter joins, excluding rasterization and destination writes.
A short 10-sample diagnostic on 2026-07-31 measured Square/Miter at about
3.23 µs (19.8 million input points/s) and Round at about 18.26 µs
(3.51 million input points/s). Round geometry defaults to eight segments per
half circle and uses the shared integer CORDIC; callers can trade edge capacity
and the explicit chord-error bound with `with_round_segments`. The end-to-end
canvas entry borrows both edge and prepared-line storage and feeds the native
fixed paint pipeline.

The `analytic_active` group isolates binned scan conversion from path expansion
and pixel compositing. It covers stable active-set scaling, short-edge churn,
unordered activation batches, coincident crossings, and a 32-edge crossing
stress scene. These diagnostics guide algorithm selection; they are not
cross-renderer rankings. Current design conclusions and rejected sorting
experiments are recorded under “Performance decisions” in `DESIGN.md`.

### Fixed retained coverage and tiles

The optional retained fixed output groups only non-empty 16-row strips. Each
strip descriptor is 12 bytes and each uniform non-zero coverage run is 12
bytes (`u32` x/length plus `u8` row/coverage). It therefore does not impose a
full-frame mask, and callers choose an explicit bounded run capacity.

Commit `c2de47a` adds a separate retained-tile composite entry point, so a
stable coverage mask can be reused with another color or destination without
rasterizing its geometry again. A focused run on the same machine used
Criterion's default 3-second warm-up, 5-second measurement, and 100 samples.
Both paths clear and composite the RGBA8888 destination; `cached` excludes the
one-time rasterization and tile encoding cost:

| Scene | rasterize + tiled composite | cached tiled composite | Speedup |
| --- | ---: | ---: | ---: |
| 64 fractional rectangles | 343.00 µs | 41.220 µs | 8.3× |
| 16 sparse rectangles | 55.516 µs | 3.7166 µs | 14.9× |
| 256 short-edge rectangles | 238.48 µs | 22.116 µs | 10.8× |
| 16 full-tile rectangles | 100.08 µs | 10.607 µs | 9.4× |

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
