
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
scratch storage, rectangular and path coverage clips, and allocation-free
solid, linear, two-circle radial, and conic paint samplers. Paint transforms
are independent from path transforms and invert once at construction.
Allocation-free analytic stroking now covers transformed paths, flattened
curves, open and explicitly closed contours, butt/round/square caps, and
miter/round/bevel joins. Its flattened points, compact contour descriptors,
expanded edges, intersections, and row coverage all use caller-owned bounded
storage. The project also has an early Q24.8 fixed-point backend. Production
fixed edge binning and persistent active edges now operate on caller-owned
sparse strip storage. The fixed backend can optionally retain compact sparse
coverage strips for batching or caching while keeping the lower-memory
streaming sink as its default. Q24.8 paths can now be transformed with checked
widened arithmetic, adaptively flattened, and filled without an FPU. An
optional 16 × 16 tile prototype now
classifies empty, full, and boundary regions, supports direct tile-major
output, and can composite a retained tile mask without rasterizing it again.
External fuzzing and broader golden/benchmark scenes are still under
development, so it is not yet suitable as a production renderer.

The current MSRV is Rust 1.93. CI checks MSRV and stable builds, independent
feature combinations, 32-bit Linux, and a Cortex-M target without an FPU.

## Current status

| Area | Status |
| --- | --- |
| `f32` fill and clipping | Reference path implemented and allocation-free |
| Paint and color | Solid and gradient samplers; encoded compatibility and linear-light paths |
| Stroke | Allocation-free f32/fixed dashes, caps, joins, and path stroke pipelines implemented |
| Fixed point | Q24.8 transformed path fill/stroke, sparse strips/tiles, clipping, native fixed gradients, and all fixed caps/joins implemented |
| Production readiness | Pre-release: broader fuzzing, golden scenes, and real-device validation remain |

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
`dash_requirements` and `fixed_dash_requirements` return exact point/contour
capacities. Both decomposition entry points run this preflight before writing,
so capacity and numeric errors leave caller-owned dash scratch untouched.

## Architecture at a glance

The first complete rendering path is intentionally small:

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

## Quick start

The rendering APIs borrow both destination and scratch storage. Capacities are
chosen by the caller and insufficient workspace is returned as an error:

```rust
use ugl_rs::{analytic::AnalyticIntersection,
    color::RGBA, edge::Edge, geometry::{Affine, PathBuilder},
    canvas::{AnalyticRenderOptions, AnalyticRenderWorkspace, PixmapMut,
        render_solid_analytic},
};

const  WIDTH: u32 = 4;
const HEIGHT: u32 = 4;

let mut builder = PathBuilder::new();
builder.move_to((0.5, 0.5)).line_to((3.5, 0.5))
       .line_to((3.5, 3.5)).line_to((0.5, 3.5));
let path = builder.build();
let mut pixels = [0; WIDTH as usize * HEIGHT as usize * 4];
let mut intersections = [AnalyticIntersection::default(); 8];
let mut target = PixmapMut::new(&mut pixels, WIDTH, HEIGHT, WIDTH * 4).unwrap();
let (mut edges, mut coverage) = ([Edge::default(); 8], [0.0; WIDTH as usize]);
let (mut row_offsets, mut edge_indices) = ([0; HEIGHT as usize + 1], [0; 8]);

render_solid_analytic(&path, Affine::identity(), RGBA::new(20, 200, 40, 160),
    AnalyticRenderOptions::default(), &mut target,
    &mut AnalyticRenderWorkspace { edges: &mut edges,
        intersections: &mut intersections, row_coverage: &mut coverage,
        row_offsets: &mut row_offsets, edge_indices: &mut edge_indices,
    },
).unwrap();
```

The crate is `no_std` and currently uses `alloc`. The default feature enables
the Q24.8 fixed backend; use `--no-default-features` for the floating-point
core alone. Optional `serde` support is independent.

The fixed raster APIs can feed any existing `PaintSampler` through streaming,
retained-strip, or retained-tile coverage, with rectangle or borrowed path-mask
clipping. This gives functional parity on desktop, but does not claim a wholly
integer pipeline because those compatibility samplers use `f32`.
`FixedPaintSampler` makes the no-FPU contract explicit.
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

Both f32 analytic and Q24.8 fixed paths can rasterize arbitrary path clips into
caller-owned `CoverageMaskMut` storage. Fixed compositors can therefore produce
and consume arbitrary path masks end to end without an FPU.

Color boundaries are explicit: solid paints and gradient stops accept straight
encoded `SRGBA<u8>`, while `PixmapMut::pixel` returns only validated
`PremulSRGBA8`. `pixel_bytes` exposes the physical RGBA bytes unchanged.
Pixmap construction intentionally validates layout without scanning the image;
source-over callers are responsible for valid premultiplied destination data.

`Context` and `fixed::context::Context` provide parallel stateful drawing APIs for the
analytic f32 and Q24.8 pipelines. They retain transform, fill rule, flattening,
stroke, solid color, and rectangle/mask clip state while borrowing the target
and bounded scratch storage. `fill_with` and `stroke_with` preserve static
sampler dispatch; all original low-level functions remain available.

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
| analytic `f32` | 128 `Edge` | 257 `u32` row offsets + 128 `u32` edge indices + 128 `AnalyticIntersection` | 256 `f32` |
| Q24.8 fixed | 128 `fixed::raster::Segment` + 64 `fixed::raster::Trapezoid` | one `u32` offset per strip plus one `u32` per line/strip overlap | 256 `u64` |

The compact target uses 4 bytes per pixel. `LinearPixmapMut` deliberately uses
16 bytes per pixel (`LinearPremulRGBA<f32>`) and its fast presentation path
borrows an additional 4096-byte sRGB encoding LUT. This desktop-quality working
buffer is not the intended MCU storage path. Optional dirty tracking costs one
bit per 16 × 16 tile: 32 bytes for a 256 × 256 target.

Renderer allocation count inside the measured path is zero by API
construction: every mutable geometry, crossing, area, and destination buffer
is borrowed from the benchmark. Criterion's own allocations are outside that
contract.

### Stroke and active-edge scalability

The analytic stroke baseline can be reproduced with:

```text
cargo bench --bench raster --all-features -- stroke
```

Commit `81b198c` adds separate end-to-end and geometry-expansion groups.
`stroke_rgba8888` clears a 256 × 256 destination and measures path flattening,
stroke expansion, analytic rasterization, and solid source-over composition.
`stroke_expand` measures flattening and edge emission without rasterization or
destination writes. Path construction and all scratch allocation remain
outside the measured loop. A short initial run on the same Darwin arm64
machine used a 1-second warm-up, 1-second measurement, and 10 samples:

| Scene | Points | Contours | Edges/intersections | Before bins | Binned end-to-end | Expansion only |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 32-segment Butt/Miter polyline | 33 | 1 | 191 | 557.59 µs | 430.67 µs | 972.14 ns |
| 32-segment Round cap/join polyline | 33 | 1 | 326 | 722.35 µs | 494.36 µs | 3.7321 µs |
| 8-cubic Butt/Miter path | 145 | 1 | 1102 | 4.3974 ms | 1.6534 ms | 7.9931 µs |

Every scene also borrows one 256-element `f32` row-coverage buffer, 257 `u32`
row offsets, and one `u32` edge index per visible edge. The table reports exact
minimum geometry capacities for these inputs at the default flattening
tolerance, and benchmark identifiers encode the same
`points/contours/edges` counts. Round joins increase emitted edges by 70.7%
for the polyline, while curve flattening expands eight cubics into 145 points
and 1102 fill edges. Commit `5543d2b` switches canvas analytic rendering to
caller-owned sparse row bins, eliminating repeated all-edge scans at every
vertical slab. Relative to the pre-binning values, end-to-end time
improved by 22.7% for Butt/Miter, 32.0% for Round, and 62.3% for the dense
curve scene; expansion-only time remained effectively unchanged. Coverage
integration and active-edge ordering now dominate the remaining curve cost.
These short measurements are an initial regression baseline, not a
cross-renderer performance comparison.

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
and pixel compositing. A 1-second/10-sample diagnostic run on 2026-07-31
measured 493.56 µs for 128 stable full-height edges and 44.44 µs for 512
short-lived edges, confirming that persistent active-set size dominates edge
activation churn. Thirty-two edges meeting at one coincident crossing exposed
`f32` event fragmentation: near-identical crossing heights originally formed
many negligible slabs and took 3.721 s. Coalescing events within four ulps of
the current y reduced that case to 2.407 ms while retaining sampled-reference
coverage tests for both fill rules.

The same group now tracks stable active sets of 16, 32, 64, 128, and 256
edges. A focused scaling run measured approximately 116, 170, 290, 534, and
896 µs respectively. Reversing all 256 initial edges cost only another 3.7%
because later rows remain ordered; reversing every 32-edge activation batch
raised the short-edge scene from 44.50 to 51.22 µs. A hybrid insertion/introsort
experiment did not improve the repeated-disorder case and regressed stable,
ordinary-churn, and crossing scenes by roughly 3.6–4.0%, so the hot path keeps
the specialized adaptive insertion sort. Future unordered-activation work
should sort row bins by x or merge each ordered activation batch instead of
adding a comparator and movement budget to every slab.

Vertical active sets now retain their required initial x ordering but skip
integer-x event searches, adjacent-crossing scans, and midpoint reordering:
their x coordinates cannot change. On the same host, `stable_256` improved
from about 898 µs to 783 µs and `churn_512` from about 46.5 µs to 41.8 µs.
The standard 64-rectangle linear render measured about 105.8 µs versus
110.2 µs before this specialization; the crossing scene remained near
2.42 ms.

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
