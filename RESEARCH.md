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
