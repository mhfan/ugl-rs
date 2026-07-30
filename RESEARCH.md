# Rendering research and adoption log

This document records the external designs studied by `ugl-rs` and the
engineering decisions derived from them. It is not a list of inspirations:
each relevant technique must be classified as **adopt**, **adapt**, **defer**,
or **reject**, with the constraints and validation required for its use.

Implementations must be independently written from documented algorithms and
the `ugl-rs` rendering contract. Source code is not copied unless its license
has been verified as compatible and attribution requirements are satisfied.

## Evaluation priorities

Techniques are evaluated in this order:

1. Correct fill, coverage, clipping, alpha, and degeneracy semantics.
2. Bounded memory and explicit resource failure.
3. Suitability for `no_std`, 32-bit targets, and fixed-point arithmetic.
4. Deterministic behavior and testability.
5. End-to-end time, locality, allocation count, and code size.
6. API elegance and extensibility.

A desktop throughput improvement is not automatically useful if it requires an
unbounded allocator, wide floating-point intermediates, large lookup tables, or
unacceptable binary size on the target class.

## micro{gl}: constrained-system design

Primary sources:

- <https://github.com/micro-gl/micro-gl>
- <https://github.com/micro-gl/micro-gl/tree/master/include/microgl>
- <https://github.com/micro-gl/micro-gl/blob/master/include/microgl/canvas.h>
- <https://github.com/micro-gl/micro-gl/blob/master/include/microgl/canvas.tpp>

Characteristics worth preserving:

- No standard-library, FPU, or GPU requirement.
- Explicit pixel coders, bitmap storage, samplers, compositing, and geometry
  layers instead of one monolithic canvas abstraction.
- Compile-time specialization without runtime virtual dispatch.
- Pluggable allocation and fixed/static memory strategies.
- Generic coordinate representation, including Q fixed-point numbers.
- Shape fast paths alongside a general path/tessellation pipeline.
- Trapezoid-oriented decomposition and exact fractional coverage as an
  alternative to brute-force supersampling.

Adoption decisions:

| Technique | Decision | ugl-rs interpretation |
|---|---|---|
| Caller-selectable storage and pixel coding | Adopt | Borrowed targets and explicit format traits; RGBA8888 first |
| Sampler/paint separated from coverage | Adopt | Coverage generation never knows gradient or texture details |
| Fixed-point-capable geometry | Adopt | Generic containers; evidenced fixed backend after `f32` reference |
| Pluggable/static allocation | Adopt in stages | Slice-consuming stages first, caller workspace in M5 |
| Compile-time specialization | Adapt | Rust generics/enums only at real variation points; watch code size |
| Shape fast paths | Defer | Add only after equivalence tests and profiling |
| Trapezoid decomposition | Research candidate | Compare quality, working set, and fixed-point range with strip/scanline approaches |
| 2D and 3D in one core | Reject | Keep the 2D raster core cohesive |

We will learn from micro{gl}'s constraints and decomposition, not reproduce its
C++ template surface. Rust ownership should express buffer lifetime and
workspace exclusivity, while resource exhaustion uses `Result`.

## Mature CPU reference: tiny-skia and Skia

Primary sources:

- <https://github.com/linebender/tiny-skia>
- <https://docs.rs/crate/tiny-skia/latest/source/>
- <https://skia.org/docs/>
- <https://skia.org/docs/user/api/skcanvas_overview/>

Experience to carry forward:

- Path boundary behavior, winding rules, clipping, premultiplied alpha, and
  degeneracies need compatibility-style test matrices.
- Separate path representation, paint state, pixmap storage, and raster stages.
- SIMD is optional and must remain equivalent to a scalar implementation.
- A small renderer still needs production-quality hairlines, transforms,
  gradients, clipping, and image-quality regression tests.

Decision: use these implementations as semantic and differential references
where contracts match. Do not inherit their desktop-oriented allocation or API
choices automatically.

## Current CPU research: Vello CPU and sparse strips

Primary sources:

- <https://github.com/linebender/vello/tree/main/sparse_strips>
- <https://github.com/linebender/vello/tree/main/sparse_strips/vello_common>
- <https://github.com/linebender/vello/tree/main/sparse_strips/vello_cpu>
- <https://docs.rs/vello_cpu/latest/vello_cpu/>

Relevant ideas:

- Share geometry, tiling, and strip representations between execution
  backends, but isolate renderer-specific policy.
- Represent only active coverage regions instead of materializing a full
  intermediate mask.
- Tile/strip decomposition improves locality and enables skipping empty or
  constant regions.
- Reuse render contexts and working memory across frames.
- Keep scalar/reference and vectorized execution behaviorally equivalent.
- Offer `std` and `libm` math policies explicitly; `ugl-rs` currently chooses
  `libm` for its `no_std` core.

Decision: design the edge-to-coverage boundary so it can evolve from a simple
reference scan converter to sparse strips or tiles without changing Path,
Paint, or Target APIs. Do not commit to strip dimensions or SIMD layouts before
representative embedded and desktop measurements.

## High-performance CPU reference: Blend2D

Primary sources:

- <https://blend2d.com/>
- <https://blend2d.com/about.html>
- <https://blend2d.com/performance.html>
- <https://blend2d.com/doc/multithreaded-rendering.html>
- <https://github.com/blend2d/blend2d>
- <https://blend2d.com/research/simplify_and_offset_bezier_curves.pdf>

Blend2D is a required performance and architecture baseline. Its published
benchmarks place it among the fastest general CPU vector renderers across small
primitives and complex paths. Those are project-maintained results rather than
an unconditional proof that it is fastest for every machine and workload, so
`ugl-rs` will reproduce relevant subsets with controlled output comparison.

Relevant design experience:

- Optimize the complete stack: geometry-to-edge conversion, analytic
  rasterization, dispatch overhead, pixel pipelines, and threading.
- Preserve low overhead for tiny draws; throughput on a large scene alone can
  hide costs that dominate embedded UIs.
- Use an analytic 8-bit coverage rasterizer derived from the same broad family
  as FreeType and AGG, rather than brute-force supersampling.
- Keep a portable scalar pipeline as a semantic fallback.
- Specialize hot pixel composition pipelines for the exact format, paint,
  opacity, and compositing combination instead of branching per pixel.
- Cache generated/specialized pipelines across contexts.
- Serialize asynchronous drawing into batches before worker execution, and
  avoid threading for targets too small to amortize coordination.
- Treat curve simplification and curve offsetting as first-class geometry
  algorithms; Blend2D's stroker offsets curves rather than flattening them
  prematurely.
- Preserve IEEE exceptional-value semantics; Blend2D explicitly rejects unsafe
  fast-math transformations because NaN/Infinity and expression ordering matter.

Adoption decisions:

| Technique | Decision | ugl-rs interpretation |
|---|---|---|
| Whole-pipeline measurement | Adopt | Benchmark geometry, coverage, paint, composite, and dispatch separately and end-to-end |
| Analytic 8-bit coverage | Adopt as leading candidate | Compare against micro{gl} trapezoids and sparse strips before fixing representation |
| Low-overhead tiny draws | Adopt | Include 8×8 through 256×256 cases, not only full-frame scenes |
| Portable semantic fallback | Adopt | `f32` scalar reference remains executable and testable |
| Specialized pixel pipelines | Adapt | Static Rust specialization/enums first; avoid combinatorial code-size growth |
| Pipeline caching | Defer | Relevant after formats, paints, and compositors stabilize |
| JIT code generation | Reject for core | Conflicts with MCU, `no_std`, W^X, binary-size, and determinism goals |
| Batched multithreading | Defer to integration layer | Valuable on desktop/mobile, inappropriate as a core assumption |
| Curve-offset stroking | Research for M3 | Study before choosing flatten-then-offset stroke expansion |

Blend2D serves two different roles:

1. **Quality/semantic reference** for analytic coverage, curves, strokes,
   gradients, formats, and compositing.
2. **Desktop performance ceiling** showing what aggressive specialization,
   vectorization, batching, and JIT can achieve.

The second role must not distort the constrained-device core. An optimization
is adopted only if its scalar or ahead-of-time form has acceptable memory and
code-size cost. Desktop-only accelerators may later live behind optional
features or in a separate crate.

The benchmark harness should mirror the useful parts of `bl_bench`: aligned,
fractional, and rotated rectangles; round rectangles; self-intersecting
polygons; curve-heavy paths; complex world geometry; non-zero/even-odd fills;
solid and gradient paint; and sizes from tiny icons to full frames. It must add
peak workspace, allocation count, binary size, and pixel-difference reporting.

## GPU research: Vello

Primary source:

- <https://github.com/linebender/vello>

Vello demonstrates the value of staged encodings, prefix-sum-friendly work,
compact scene representations, and separating coarse from fine rasterization.
Its GPU execution and synchronization strategy is outside `ugl-rs` scope, but
its data-oriented stage boundaries are relevant.

Decision: adapt data-flow lessons only. Avoid GPU-driven complexity, global
scene encodings, or parallel-prefix machinery until a CPU workload proves the
need.

## Classic analytic rasterization

Research set:

- Anti-Grain Geometry: <https://agg.sourceforge.net/antigrain.com/>
- FreeType rasterization: <https://freetype.org/freetype2/docs/>
- Pathfinder: <https://github.com/servo/pathfinder>

Topics to evaluate before choosing the production coverage algorithm:

- exact/analytic area coverage versus multisampling;
- active-edge scan conversion versus trapezoids;
- coarse tiles plus fine per-pixel rasterization;
- winding accumulation and shared-edge rules;
- clipping before or during edge binning;
- curve flattening error after transformation;
- numerical behavior for nearly horizontal edges and large coordinates.

The first `f32` implementation should be simple enough to serve as an oracle,
not optimized into an opaque production algorithm. Candidate production
algorithms will be compared against it using golden images and randomized
paths.

## Required decision records

Before implementing each major stage, add a short entry containing:

- problem and observable contract;
- algorithms and implementations studied;
- adopted design and why it fits constrained targets;
- rejected alternatives and their tradeoffs;
- fixed-point implications and required intermediate range;
- memory upper bound and allocation behavior;
- tests and benchmarks that can falsify the decision.

The first required records are:

1. Curve flattening and device-space tolerance.
2. Directed edge representation and horizontal-edge policy.
3. Coverage algorithm: active edges, trapezoids, or sparse strips.
4. Clip semantics and winding preservation.
5. Coverage run representation and paint/compositor boundary.

## Decision record 1: reference curve flattening

### Problem and contract

Quadratic and cubic Bézier segments must become directed device-space lines.
The maximum accepted geometric deviation is controlled by a positive finite
tolerance measured in device pixels. Path transforms therefore happen before
flatness evaluation. The stage must not require allocation and must have a
bounded failure mode for pathological curves.

### Designs studied

- micro{gl}'s configurable curve division and fixed-number approach.
- Blend2D recursive subdivision, approximation options, and research on
  simplifying/offsetting Bézier curves within error limits.
- Analytic quadratic flattening and curve work by Raph Levien/kurbo.
- Uniform parameter stepping and forward differencing.
- Segment-count estimation followed by direct evaluation.

### Decision

The first `f32` reference uses iterative de Casteljau subdivision with a fixed
explicit stack:

- control points are transformed to device space first;
- flatness compares squared control-point distance from the endpoint chord with
  squared tolerance, avoiding a square root;
- degenerate chords compare controls with the endpoint directly;
- subdivision occurs at `t = 0.5`;
- right halves are pushed before left halves to preserve path order;
- recursion depth is caller-configurable up to a compile-time bound;
- directed lines are emitted through a fallible sink, so a caller can use a
  `Vec`, fixed-capacity buffer, streaming edge builder, or direct raster stage.

### Why this is the reference

It is simple, deterministic, auditable, invariant under affine transformation
when evaluated after that transformation, and has an obvious fixed-point
migration path based on midpoint averages and widened cross products. The
explicit stack bounds memory and avoids recursion on small embedded stacks.

### Deferred alternatives

- Analytic quadratic flattening may reduce segment count and branches.
- Wang-style or other segment-count estimates may vectorize better.
- Blend2D-style specialized flatteners may reduce dispatch and improve curve
  throughput.
- Stroke construction may offset curves directly rather than flattening first.

These remain production candidates, but replacing the reference requires
equivalent-output tests and measurements for line count, time, workspace, and
fixed-point intermediate range.

### Fixed-point implications

Midpoint subdivision needs one extra fractional bit or an explicit rounding
rule. Squared cross products can need roughly twice the coordinate width, and
the comparison against squared chord length and tolerance may require wider
intermediates again. The fixed backend must establish its device-coordinate
range and use widened integer products before sharing this implementation.

### Falsification

Tests cover straight and degenerate curves, transformed tolerance, output
order, exact endpoints, invalid tolerances, non-finite transformed values,
depth exhaustion, and sink capacity errors. Random curves will later compare
maximum sampled deviation and output with the reference renderer.

## Decision record 2: directed fill edges

### Problem and contract

Flattened subpaths must become edges that preserve winding and can feed several
candidate coverage algorithms. Fill semantics implicitly close open subpaths.
Horizontal lines contribute no scan crossing, while shared vertices must later
use a half-open vertical interval to avoid double counting.

### Designs studied

- micro{gl} tessellation/trapezoid separation and allocator-aware geometry.
- Blend2D's geometry-to-edge focus and analytic rasterizer.
- AGG/FreeType-style directed cell/edge accumulation.
- Vello CPU shared geometry followed by tile/strip-specific processing.

### Decision

The common `Edge<T>` stores only:

- endpoints normalized to increasing device-space `y`;
- a signed winding value preserving original direction.

It deliberately does not store inverse slope, scanline bounds, tile IDs, or
coverage coefficients. Those belong to the selected raster backend and have
different numeric and locality tradeoffs.

The flattener exposes subpath begin/end events through default sink methods.
A fill-edge adapter uses them to close every subpath implicitly, including when
another `MoveTo` begins a new one. Exact horizontal and zero-length lines emit
no edge. With device `y` increasing downward, downward source edges have
winding `+1` and upward edges `-1`; changing both signs would not affect
non-zero filling, but the convention remains stable for tests.

### Fixed-point and memory implications

The shared edge performs no division and needs no wider intermediate than its
coordinate representation. Fixed-point slope or intersection calculations are
deferred to raster-backend preparation, where their required width can be
proved. Edges stream through a fallible sink, so callers can use owned,
fixed-capacity, or immediate binning storage.

### Deferred choices

- Active-edge sorting and half-open intersection rules.
- Direct trapezoid decomposition.
- Strip/tile binning and compact edge encodings.
- Clipping before edge creation versus during backend preparation.

### Falsification

Tests cover clockwise/counter-clockwise winding, implicit and explicit closure,
multiple subpaths, horizontal removal, transformed curves, and capacity failure.

## Decision record 3: bootstrap reference coverage

### Problem and contract

The project needs an executable edge-to-coverage reference before selecting and
optimizing the production rasterizer. It must support non-zero and even-odd
fills, deterministic 8-bit coverage, half-open vertical edge intervals, exact
horizontal span overlap, caller-owned memory, and explicit capacity failure.

### Designs studied

- Blend2D, AGG, and FreeType analytic cell accumulation.
- micro{gl} trapezoid decomposition and fractional coverage.
- Vello CPU sparse strips and coarse/fine separation.
- Active-edge scanline conversion with supersampled or analytic coverage.

### Decision

The bootstrap `f32` reference uses deterministic stratified vertical sampling.
For each pixel row and sample:

1. intersect all edges using `[upper.y, lower.y)` half-open intervals;
2. sort intersections by `x`;
3. accumulate non-zero winding or even-odd parity;
4. add exact horizontal overlap of every inside span to pixel coverage;
5. average samples and round once to 8-bit coverage.

The default is 256 vertical samples. This is not claimed to be an analytic or
production-performance rasterizer. It establishes fill semantics, workspace
and sink APIs, end-to-end tests, and a high-quality deterministic baseline
without hiding complex cell arithmetic in the first implementation.

### Memory and complexity

The caller supplies:

- one intersection slot per input edge;
- one floating coverage accumulator per target-row pixel.

No render-stage allocation occurs. Runtime is
`O(height × samples × (edges + intersections log intersections + covered width))`;
this is intentionally expensive and suitable only as a reference.

### Production candidates

The production backend must compare:

- AGG/FreeType/Blend2D-style analytic cell accumulation;
- micro{gl}-style trapezoid decomposition;
- active edges with analytic area;
- sparse strip/tile binning with coarse empty/full rejection.

The common `Edge`, `CoverageSink`, fill rules, and target contract must survive
that replacement.

### Limitations and falsification

Vertical sampling is approximate and can miss geometry thinner than the sample
spacing or produce error around high-curvature/intersection events. Tests cover
aligned and fractional rectangles, shared endpoints, nested winding/parity,
clipping to the target, workspace errors, and deterministic quantization.
Analytic implementations are compared both against this reference and exact
area fixtures; disagreement is not automatically an analytic-backend bug when
the reference sampling error explains it.

## Architecture decision: two production backend families

The production roadmap is not a single compromise rasterizer:

- Desktop/mobile targets pursue sparse strips or tiles combined with analytic
  cells and optional SIMD, using Blend2D and Vello CPU as major references.
- MCU and fixed-memory targets pursue trapezoids or scanline spans combined
  with fixed-point analytic boundary area, using micro{gl}, AGG, and FreeType
  techniques as references.

Shared contracts end at directed edges on input and coverage runs/pixels on
output. Paint sampling and compositing do not depend on how coverage was
generated. This permits differential testing across all backends and prevents
desktop scheduling/layout decisions from increasing MCU memory requirements.

## Decision record 4: retained coverage strips and tiles

### Problem and studied designs

Desktop batching needs a compact intermediate that skips inactive regions,
but MCU rendering must retain bounded streaming operation. The design follows
micro{gl}'s explicit-memory constraint, Vello CPU's sparse strip/tile and
coarse/fine separation, and Blend2D's requirement to measure the complete
pipeline rather than assuming tiling is automatically faster.

### Decision

The fixed rasterizer keeps three optional output levels:

1. direct `CoverageSink` streaming for minimum memory and latency;
2. retained 16-row sparse strips containing uniform non-zero coverage runs;
3. 16 × 16 sparse tiles derived from retained strips.

Empty tiles have no record. Full tiles have one descriptor and no fine runs.
Boundary tiles store four-byte tile-local runs. Conversion uses caller-owned
eight-byte sortable pieces, and output capacity failure does not expose a
partially successful tile stream. The fixed tile grid is an internal backend
contract and does not enter `Path`, `Edge`, paint, or target APIs.

### Fixed-point, memory, and performance implications

Tiling changes no Q24.8 arithmetic or 8-bit coverage values. A tile descriptor
is 16 bytes, a retained boundary run is 4 bytes, and conversion scratch is
8 bytes per run/tile overlap. All capacities are explicit and allocation-free
inside the stage.

Measured strip retention adds roughly 1–3% in the current scenes. The first
row-major-strip to tile-major converter adds roughly 14% for a sparse scene
and 41–61% for denser scenes, primarily due to piece sorting. It is therefore
an optional batching/cache prototype, not the immediate renderer default.

The direct successor links pieces by tile column during each active raster
strip and compacts only touched columns. Scratch becomes one strip of 8-byte
linked pieces plus three `u32` arrays per tile column. Against the earlier
converter it reduced measured encode time from about 317 to 241 µs for 64
rectangles, 48 to 43 µs for the sparse scene, and 244 to 188 µs for 256 short
edges. Streaming remains cheaper, but direct tile output is now close enough
to evaluate with a tile-aware compositor and repeated/batched use.

A conservative requirements API now bounds tile descriptors, fine runs,
one-strip pieces, and all three column arrays from target dimensions. It
rejects dimensions outside the fixed backend's documented coordinate range
and allows fixed-memory callers to avoid retry-based capacity discovery.
Because direct emission releases each strip as it advances, an error may leave
earlier formal output records overwritten; callers discard outputs on error.
Guaranteeing rollback would require an additional retained buffer or a second
raster pass and is not part of the low-memory contract.

### Rejected and deferred alternatives

- A full-frame tile table is rejected because memory scales with target area
  even when coverage is sparse.
- Making retained tiles the MCU default is rejected because it adds scratch,
  latency, and a second representation.
- Expanding full tiles back into sixteen row spans is retained only for
  `CoverageSink` compatibility. Immediate tile-aware rendering remains slower,
  but retained coverage reuse is 8.3–14.9× faster than rasterizing again in
  the current four benchmark scenes.
- SIMD and tile-parallel scheduling remain deferred until scalar direct
  emission has equivalent-output tests and favorable end-to-end measurements.

### Falsification

Tests verify empty omission, full/boundary classification, clipped edge tiles,
compact layouts, capacity errors, and exact replay against streaming raster
output. Deterministic randomized cases compare streaming, retained strips,
and direct tiles for both fill rules and require identical raster errors.
Benchmarks separate streaming, strip encoding/replay, tile encoding/replay,
immediate composition, and retained composition across dense, sparse,
short-edge, and full-tile scenes.

## Decision record 5: stroke expansion

### Primary sources and observations

- micro{gl}/micro-tess path stroke exposes width, cap, join, miter limit, dash
  array, and dash offset; it supports allocator-aware/static containers and
  fixed-point number types. Its tessellation and buffer caching are useful
  constrained-memory references:
  <https://micro-gl.github.io/docs/micro-tess/algorithms/path-stroke>.
- Blend2D exposes stroked paths separately from rasterization and its offset
  sink consumes both sides of each figure, adding caps for open contours. It
  also treats curve simplification/offsetting as a dedicated approximation
  problem rather than flattening everything first:
  <https://blend2d.com/doc/group__bl__geometry.html>,
  <https://blend2d.com/research/precise_offset_curves.pdf>.
- tiny-skia's Skia-derived stroker maintains separate inner and outer
  builders, reverses the inner side when closing an outline, recursively
  approximates offset curves, detects curve reductions/cusps, and gives
  zero-length segments explicit cap semantics:
  <https://github.com/RazrFalcon/tiny-skia/blob/master/path/src/stroker.rs>.
- GPU-oriented polar and minimal-arc expansion are valuable later references,
  but they do not by themselves satisfy the current caller-owned CPU/fixed
  workspace contract:
  <https://arxiv.org/abs/2007.00308>,
  <https://arxiv.org/abs/2405.00127>.

### Decision

M3 uses two implementation levels behind the same stroke semantics:

1. The scalar behavioral reference transforms and flattens the centerline in
   device space, then expands line segments with explicit caps and joins.
   Width and miter limit are therefore measured in device pixels. This reuses
   the existing post-transform tolerance contract and is straightforward to
   differential-test.
2. A later desktop/mobile production stroker may offset quadratic and cubic
   curves directly, subdividing around inflections, cusps, and approximation
   failures. It must match the scalar reference within a documented pixel
   tolerance.

The MCU/fixed backend may retain bounded flatten-first expansion when direct
curve offsetting would increase code size or scratch memory without a measured
benefit. Direct curve offsetting is not forced into a generic numeric trait.

The first stroke slice supports positive finite width, butt/round/square caps,
bevel/round/miter joins, and a finite miter-limit ratio. Miter joins fall back
to bevel when the intersection exceeds `miter_limit × half_width`. Dash
patterns and zero-width hairlines remain deferred.

Open contours receive caps and are never implicitly closed. Closed contours
join their last and first non-degenerate segments and receive no caps.
Repeated zero-length segments do not create joins. A contour containing only
one point is empty with butt caps; round and square caps produce a centered
shape, following the established Skia behavior.

### Memory and output strategy

The reference expander writes fillable stroke geometry through a bounded sink;
it does not require an owned output `Path`. Segment bodies, joins, and caps may
be emitted as consistently wound closed contours, so a streaming MCU path does
not need to retain and reverse an entire inner outline. A retained-outline
adapter can still build an owned `Path` for caching, debugging, and desktop
curve-offset work.

Capacity failure reports required output progress and invalid or non-finite
style/geometry fails before successful rendering. Fixed-point implementation
must widen normalization, cross products, line intersections, and miter tests;
Q24.8 storage alone is insufficient for these intermediates.

### Deferred alternatives and falsification

- Direct offset curves are deferred from the first slice, not rejected.
- Hairlines are deferred because device coverage semantics differ from a
  geometric stroke of width zero.
- Dashes follow only after contour/cap/join behavior is stable.
- A triangle mesh is not the common core output because the CPU fill
  rasterizers already consume paths/edges and a mesh would impose indexing and
  extra storage on MCU callers.

Tests must cover every cap/join, miter fallback, open versus closed contours,
repeated and point-only contours, reversals, acute/obtuse corners,
self-intersections, transformed curves, capacity failure, and randomized
comparison against the filled expanded outline. Benchmarks separate expansion,
rasterization, and end-to-end stroke rendering.
