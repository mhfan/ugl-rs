# ugl-rs rendering contract

This document defines the invariants of the rendering core. Changes to these
rules are observable API changes and require tests and release notes.
External techniques and their adopt/adapt/defer/reject decisions are tracked in
[RESEARCH.md](RESEARCH.md). A major rendering stage is not implemented without
first recording the relevant algorithm and fixed-point/memory implications.

## Goal and differentiation

`ugl-rs` is not intended to be another general desktop drawing API or a small
Skia clone. Its target is the intersection of:

- safe, pure Rust;
- CPU-only rendering;
- constrained 32-bit and 64-bit systems;
- `no_std`, fixed or caller-owned memory, and bounded failure;
- operation without an FPU through a fixed-point backend;
- deterministic, high-quality vector rasterization.

Industrial quality means more than feature count: semantics are documented,
invalid input fails explicitly, memory bounds are observable, output is covered
by reference and differential tests, and performance claims include memory and
code-size costs.

## Scope

The core is a deterministic, CPU-only 2D vector rasterizer for constrained
systems. It is `no_std`, may use `alloc`, and must be usable with caller-owned
pixel and scratch buffers. Windowing, image decoding, SVG parsing, text shaping,
and 3D rendering are outside the core.

The first reference backend uses `f32`. Geometry containers are generic over
their coordinate representation so a fixed-point backend can reuse the same
scene representation. Rasterization algorithms are not made generically
numeric until the required fixed-point operations and overflow behavior are
known.

### Core capabilities, in order

1. Paths, affine transforms, curve flattening, filling, and clipping.
2. Solid paint and source-over compositing into premultiplied RGBA8888.
3. Linear, radial, and conic gradients through a bounded sampler interface.
4. Strokes with caps, joins, miter limits, and later dash patterns.
5. Additional pixel formats and blend modes.
6. A fixed-point backend and caller-provided rasterization workspace.

### Non-goals

- Window, display, and GPU integration.
- SVG/PDF parsing, image codecs, and asset loading.
- Font discovery and text shaping.
- GUI widgets or scene layout.
- A 3D shader API in the 2D core.
- Generic abstraction over every possible numeric or pixel type.

These may be separate crates or integrations after the core contract is stable.

## Architecture and numeric strategy

The architecture separates representation from execution:

- `Point<T>`, `Affine<T>`, and `PathSegment<T>` carry coordinates without
  prescribing floating point.
- Owned `Path<T>` uses `alloc`; renderers consume segment slices so static and
  fixed-capacity storage can avoid it.
- The reference geometry and rasterizer algorithms use `f32`.
- The Q24.8 fixed-point backend follows the reference behavior with widened
  intermediates and explicit overflow and rounding rules.

This deliberately avoids a broad `Scalar` trait at the start. Such a trait
would either expose too little for efficient algorithms or encode guessed
requirements. Common operations are extracted only when both backends provide
evidence for them. Fixed-point support must not silently change overflow,
rounding, or degeneracy behavior.

The core layers are:

```text
geometry representation
    -> flattening and edge generation
    -> clipping and scan conversion
    -> coverage runs
    -> paint sampling
    -> compositing
    -> borrowed pixel target
```

Dependencies point downward only. Color and geometry do not depend on canvas
or a renderer. Parsing, codecs, text, and platform integration stay outside.

### Production backend families

The project targets two optimized execution families behind the same Path,
Edge, Paint, CoverageSink, and borrowed Target contracts:

- **Desktop/mobile high performance:** sparse strips or tiles for locality and
  empty/full rejection, analytic cell coverage at active boundaries, and
  optional ahead-of-time SIMD specialization.
- **MCU/fixed memory:** scanline spans or trapezoid decomposition, fixed-point
  analytic boundary area, caller-owned bounded workspace, and streaming
  compositing without a full intermediate mask.

Both may share edge preparation, area formulas, fill semantics, paint sampling,
and compositing. Backend-specific inverse slopes, cell accumulators, strip IDs,
and SIMD layouts do not enter the common `Edge` representation.

## Coordinates and transforms

- User space is Cartesian with increasing `x` to the right and increasing `y`
  downward after the user-to-device transform.
- Device pixel `(x, y)` covers `[x, x + 1) × [y, y + 1)`.
- Pixel centers are at `(x + 0.5, y + 0.5)`.
- Paths may contain negative and off-canvas coordinates.
- Affine transforms use six coefficients and column vectors:
  `x' = a*x + c*y + e`, `y' = b*x + d*y + f`.
- Non-finite `f32` coordinates are invalid input. The renderer will return an
  error rather than panic or silently emit geometry.
- The fixed-point backend must document its Q format, intermediate widths,
  rounding, saturation, and accepted device-coordinate range before release.
- The initial device-coordinate reference format is signed Q24.8
  (`fixed::types::I24F8`): 8 fractional bits align with 8-bit coverage and its
  integer range accommodates large device surfaces. It is a storage and API
  reference, not permission to evaluate transforms, slopes, cross-products, or
  accumulated area in 32 bits. Those operations require at least 64-bit widened
  intermediates and explicit narrowing behavior.
- Fixed raster intersections remain exact rationals while sorting and forming
  topology. At the analytic-area boundary they are rounded to the nearest
  Q24.8 subpixel, with exact half-way cases rounded away from zero. Trapezoid
  area uses a doubled 64-bit integer representation; pixel-clipped area is
  saturated and rounded to the nearest 8-bit coverage value.

## Paths and filling

- A subpath normally starts with `MoveTo`; the first drawing command without a
  current subpath implicitly inserts a `MoveTo` to that command's endpoint.
- A subsequent `MoveTo` starts a new subpath.
- `Close` connects the current point to the subpath start and is idempotent;
  without a current subpath it is a no-op.
- Zero-length edges are accepted but contribute no winding or coverage.
- Both non-zero winding and even-odd fill rules will be supported.
- Open subpaths are implicitly closed for filling, but not for stroking.
- Curve flattening tolerance is measured in device pixels after transformation.

## Pixels and color

- The first target format is premultiplied RGBA8888.
- Public color constructors use logical RGBA channel order regardless of memory
  layout.
- Compositing is source-over unless explicitly selected otherwise.
- Coverage multiplies premultiplied source color and alpha.
- The reference pipeline interprets channel values in linear light. Explicit
  sRGB conversion belongs at input/output boundaries.
- Integer conversion maps channel extrema exactly and uses round-to-nearest.

## Memory and failure

- Rendering never owns the destination; it borrows a caller-provided buffer.
- Invalid dimensions, insufficient destination/scratch storage, non-finite
  geometry, coordinate overflow, and unsupported operations return errors.
- Library code must not panic for data-dependent input.
- The initial owned `Path` uses `alloc::vec::Vec`; rasterization consumes a
  segment slice so fixed-capacity and static paths require no owned `Path`.
- Allocation-free rasterizers accept caller-provided edge and coverage
  workspaces and report the required capacity when they are too small.

## Determinism and quality

- Given the same backend, target, and input, output bytes are identical.
- The `f32` backend is the behavioral reference for fixed-point differential
  tests, not a promise of bit-identical cross-platform floating-point output.
- Golden images cover boundary placement, fill rules, degenerate geometry,
  clipping, transforms, curves, alpha, and color conversion.
- Performance work must report time, peak scratch memory, allocation count, and
  code size where relevant. SIMD is introduced only behind equivalent-output
  tests.

## Verification strategy

Every rendering stage is tested at three levels:

1. **Properties and unit tests:** extrema, winding invariants, empty and
   degenerate geometry, transform behavior, bounded coverage, and alpha laws.
2. **Golden images:** reviewed scenes with exact output or an explicitly
   documented error metric.
3. **Differential and fuzz tests:** randomized paths compared with the `f32`
   reference and, where semantics match, established renderers.

Feature combinations are compiled independently so dev-dependency feature
unification cannot hide broken `no_std`, `serde`, fixed-point, or allocation
configurations. The declared MSRV is Rust 1.93; CI also checks stable Rust,
32-bit Linux, and a Cortex-M target without an FPU.

## Implementation rules

- Correctness and documented semantics precede performance.
- Study micro{gl}, Blend2D, mature CPU renderers, and relevant current research
  before committing a major stage; record the resulting decision in
  `RESEARCH.md`.
- Borrow constraints and proven techniques, not source structure. Verify
  license compatibility before using implementation code.
- Complete one narrow vertical slice before broadening the API.
- Avoid `unsafe` until profiling demonstrates a material need; each use requires
  a local safety contract and targeted validation.
- Do not add `#[inline]` by default. The compiler decides ordinary inlining;
  annotations require benchmark or code-size evidence.
- Do not introduce SIMD before scalar equivalence tests exist.
- Public types must not expose large third-party math or rendering dependencies.
- Data-dependent input returns `Result`; it must not panic or abort.
- Every optimization states its time, memory, allocation, and code-size tradeoff.

## Rendering pipeline delivery order

The first vertical slice is implemented and validated in this order:

1. Define `Point`, `Affine`, `PathSegment`, `Path`, and `PathBuilder`.
2. Validate path state and reject non-finite reference coordinates.
3. Transform and adaptively flatten quadratic and cubic Bézier curves.
4. Produce directed edges while removing zero-contribution degeneracies.
5. Clip edges to the target bounds without changing winding semantics.
6. Convert edges to anti-aliased pixel coverage using non-zero winding first.
7. Sample a solid premultiplied color.
8. Composite source-over into a borrowed RGBA8888 target.
9. Add even-odd filling and golden/differential scenes.
10. Establish benchmark baselines before optimizing allocation or SIMD.

Stroke expansion, gradients, other formats, and fixed-point execution begin
only after this path is complete.

## Milestones

### M0 — Contract and test foundation

- This design contract, crate scope, MSRV, and supported target matrix.
- `no_std` core build and independent feature checks.
- Geometry and color property tests.
- Golden-image and benchmark harness skeletons.

### M1 — Floating-point solid path fill

- Validated paths and affine transforms.
- Adaptive quadratic and cubic flattening.
- Non-zero and even-odd filling with anti-aliased coverage.
- Solid source-over output into borrowed RGBA8888 storage.
- Degenerate, boundary, clipping, and randomized tests.

### M2 — Paint and clipping

- Rectangular and path clips.
- Linear, radial, and conic gradients.
- A bounded sampler contract and explicit color-space boundaries.

### M3 — Stroke

- Width, cap, join, and miter behavior.
- Degenerate subpaths and self-intersections.
- Dash patterns only after the base stroke contract is stable.

### M4 — Fixed-point backend

- A documented Q format and device-coordinate range.
- Proven intermediate widths and explicit rounding/overflow policy.
- Differential tests against the `f32` reference.
- Representative targets that build without hardware floating point.

### M5 — Fixed-memory rendering

- Caller-provided edge, coverage, and temporary workspaces.
- No allocation on the render path.
- Capacity errors report required resources where feasible.

### M6 — Performance engineering

- Reproducible comparisons with relevant CPU rasterizers.
- Time, peak scratch memory, allocation count, binary size, and image-quality
  results.
- Specialized/SIMD paths only where measurements justify them.

### M7 — Production release

- Stable public contract and SemVer policy.
- Documented MSRV and supported targets.
- Fuzzing, unsafe review, examples, integration guidance, and real-device
  validation.

## Current status

- M0 is substantially complete; comprehensive golden scenes and benchmark
  baselines remain.
- M1 has an allocation-free vertical slice from path segments through sampled
  or analytic `f32` coverage to premultiplied source-over output; the analytic
  backend persists active edges and skips empty vertical ranges.
- M4/M5 prototypes include generic fixed-point geometry, widened Q24.8
  arithmetic, rational crossing events, span/trapezoid area evaluation,
  caller-owned workspaces, and differential/error-path tests.
- Production fixed edge binning, broader golden scenes and benchmarks,
  fuzzing, gradients, clipping, and stroking remain future work.
