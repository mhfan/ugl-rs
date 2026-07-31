# ugl-rs design and rendering contract

This document defines the invariants of the rendering core. Changes to these
rules are observable API changes and require tests and release notes.
External techniques and their adopt/adapt/defer/reject decisions are tracked in
[RESEARCH.md](RESEARCH.md). A major rendering stage is not implemented without
first recording the relevant algorithm and fixed-point/memory implications.
User-facing project status, commands, and measured baselines belong in
[README.md](README.md); this file records normative behavior and engineering
decisions.

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

### Core capabilities

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
- Both non-zero winding and even-odd fill rules are supported.
- Open subpaths are implicitly closed for filling, but not for stroking.
- Curve flattening tolerance is measured in device pixels after transformation.

## Pixels and color

- The first target format is encoded-sRGB premultiplied RGBA8888.
- Public color constructors use logical RGBA channel order regardless of memory
  layout.
- Compositing is source-over unless explicitly selected otherwise.
- Coverage multiplies premultiplied source color and alpha.
- `SRGBA`, `LinearRGBA`, and their premultiplied counterparts make transfer
  state explicit. `canvas_linear::LinearPixmapMut` retains premultiplied
  linear-light `f32` through source-over and encodes only when presenting into
  RGBA8888. `canvas::PixmapMut` remains the compact encoded-domain compatibility
  and performance path.
- Linear presentation has two explicit modes: `encode_into` is the exact
  transfer-function reference, while `encode_into_with` uses a caller-owned
  4096-entry `Srgb8Encoder` table and is constrained to one RGBA8 code value per
  channel of the reference by tests.
- `LinearPixmapMut::with_dirty_tiles` optionally borrows one bit per 16×16 tile.
  Coverage spans mark tiles during composition; incremental presentation
  consumes those bits and preserves untouched destination tiles. At 50% dirty
  tile area it switches to contiguous full-frame encoding. Known-dense callers
  should omit tracking to avoid its span-marking cost.
- Integer conversion maps channel extrema exactly and uses round-to-nearest.

## Paint and gradients

- `PaintSampler` returns `EncodedPremulSRGBA8` at device-space pixel centers
  and is statically dispatched without allocation.
- `LinearPaintSampler` is a separate explicit contract returning
  `LinearPremulRGBA<f32>` without an encoded round trip. Built-in solid and
  gradient paints implement both contracts; custom encoded samplers do not
  silently opt into linear compositing.
- Fixed streaming, retained-strip, and retained-tile coverage share the encoded
  `PaintSampler` compositor and rectangle/path-mask adapters. This establishes
  functional backend parity without claiming FPU-free paint evaluation:
  existing gradient samplers remain `f32`.
- `FixedPaintSampler` is the explicit no-FPU contract. `FixedLinearGradient`
  accepts Q24.8 endpoints and a caller-owned encoded ramp. It uses `i64`
  coordinate deltas and exact `i128` projection, spread mapping, and nearest
  ramp selection: the full Q24.8 endpoint difference squared reaches the edge
  of `i64`, so `i128` is required before summing two axes.
  `FixedRadialGradient` supports increasing or decreasing concentric radii and
  general two-circle/focal geometry. Its reduced discriminant is proven within
  `i128` over the fixed device domain; adaptive integer square roots retain up
  to 16 fractional bits, and the same largest-valid-root policy as the `f32`
  reference handles focal cones. Ordinary values take `u64`/`i64` fast paths.
  Static ramps need no allocation or runtime color conversion on an MCU.
- `FixedAngle` stores one binary turn in `u32`, avoiding unit ambiguity and
  floating-point conversion at the fixed conic API boundary.
  `FixedConicGradient` uses 16 integer CORDIC vectoring steps and direct
  wrapping ramp selection. Tests bound angular error below `6e-6` turn on the
  integer grid and encoded output to one adjacent ramp entry versus `atan2f`.
- Solid paint reports its constant color so span and tile compositors retain
  their bulk fast paths.
- `TransformedPaint` maps device samples into paint-local coordinates through
  an inverse affine computed once at construction. A singular or non-finite
  transform is rejected.
- Gradient stops borrow caller-owned storage, are ordered in `[0, 1]`, decode
  supplied sRGB to linear light once, and interpolate premultiplied `f32`
  channels. Samples are encoded to sRGB at the framebuffer boundary. Equal
  offsets form a hard transition; the last stop at an exact repeated offset
  wins.
- Linear and radial gradients support pad, repeat, and reflect extension.
- Radial paint uses the general two-circle model. Samples outside its valid
  cone are transparent, and negative, non-finite, or identical-circle inputs
  are rejected.
- Conic paint covers one complete turn, repeats at the seam, and takes its
  start angle in radians.
- `GradientStop::new` treats `RGBA<u8>` as straight encoded sRGB for migration;
  `GradientStop::from_srgba` is the explicit API. Linear interpolation and
  encoded-domain framebuffer compositing are intentionally separate stages.
- `GradientStops::new` is the exact reference path. `GradientStops::with_ramp`
  precomputes into caller-owned storage for allocation-free high-throughput
  sampling. Its encoded premultiplied entries approximate the linear-light
  reference: smooth-gradient error decreases with ramp size, while hard
  transitions are quantized to one ramp interval. A 1024-entry ramp is the
  current performance baseline.
- The encoded ramp is intentionally bypassed by `LinearPaintSampler`.
  `GradientStops::with_linear_ramp` instead builds a caller-owned
  `LinearPremulRGBA<f32>` ramp, avoiding transfer conversion and quantization.
  A 1024-entry linear ramp costs 16 KiB; `GradientStops::new` retains exact
  direct interpolation with no ramp storage for reference and MCU use.
- `LinearPaintSampler::sample_linear_span` is the zero-allocation stepping
  boundary. Its default preserves point sampling; linear gradients specialize
  affine parameter stepping, and transformed paints map the span origin and
  direction once. Concentric radial gradients use a second-difference recurrence
  for squared distance, then one square root per sample; non-concentric radial
  and conic gradients retain the general point fallback. Specializations must
  remain equivalent to the general sampler within a documented linear-light
  tolerance.
- Conic angle quality is explicit: `ConicAngleMode::Exact` uses `atan2f` and is
  the default reference; `Fast` uses Skia's seventh-degree unit-angle
  polynomial. Fast mode has a measured circular error below `3e-5` turns and
  may shift a discontinuous seam within that bound, so it is never enabled
  implicitly.
- `LinearPaintSampler::is_opaque_linear` is conservative semantic metadata.
  Full-coverage opaque spans bypass destination loads and source-over; partial
  coverage never does. This store-only path is the first SIMD kernel boundary,
  while the trait default remains false for third-party samplers that cannot
  prove opacity over their complete domain.
- Internal linear-premultiplied `scale`, source-over, and interpolation rely on
  their closed-domain contract: colors are finite normalized premultiplied
  values and coverage/interpolation factors are in `[0, 1]`. Debug assertions
  and randomized invariant tests guard that contract, so release kernels do
  not repeat the public constructor's per-channel validation and clamping.

## Strokes

- The initial stroke width is positive, finite, and measured in device pixels
  after the path transform. Zero-width hairlines are a separate future mode.
- Open contours support butt, round, and square caps. Closed contours have no
  caps and join their final non-degenerate segment to the first.
- Joins support bevel, round, and miter. The miter limit is the maximum miter
  length divided by half the stroke width; an exceeded miter falls back to
  bevel.
- Repeated zero-length segments are ignored for tangent/join selection. A
  point-only contour is empty for butt caps and produces a centered shape for
  round or square caps.
- The scalar reference flattens transformed curves before expanding the
  centerline. A later desktop production stroker may offset curves directly,
  but must preserve the documented device-space result within its error bound.
- Stroke expansion can stream consistently wound fill contours into bounded
  caller-owned storage. It must not require an owned intermediate `Path`.
- `FixedStrokeOptions` and `stroke_polyline_fixed` provide the initial no-FPU
  Q24.8 path for all caps and joins. Integer square-root normalization and
  widened intersection tests preserve bounded arithmetic.
  `render_native_stroke_polyline_fixed` connects caller-owned edge/line scratch
  directly to fixed raster and paint. Round geometry shares the binary-angle
  CORDIC with fixed conic paint. Its explicit `round_segments` count is per
  half circle, making edge capacity and the chord error
  `r · (1 - cos(π / segments))` predictable without runtime transcendental
  functions.
- Dash patterns are added only after undashed contour semantics and capacity
  behavior are stable.

## Memory and failure

- Rendering never owns the destination; it borrows a caller-provided buffer.
- The desktop linear working target costs 16 bytes per pixel and the fast sRGB8
  presentation LUT costs 4096 bytes. The encoded compatibility target remains
  4 bytes per pixel; the MCU path must not inherit the desktop `f32` storage.
- Optional linear dirty tracking costs one bit per 16×16 tile and uses only
  caller-owned storage.
- Invalid dimensions, insufficient destination/scratch storage, non-finite
  geometry, coordinate overflow, and unsupported operations return errors.
- Library code must not panic for data-dependent input.
- The initial owned `Path` uses `alloc::vec::Vec`; rasterization consumes a
  segment slice so fixed-capacity and static paths require no owned `Path`.
- Allocation-free rasterizers accept caller-provided edge and coverage
  workspaces and report the required capacity when they are too small.
- Optional fixed retained coverage stores only non-empty 16-row strips. A
  12-byte strip descriptor indexes 12-byte uniform non-zero run records; the
  run's `u8` row is relative to its strip while x and length remain `u32`.
- Retained coverage capacity failure returns the first unavailable descriptor
  or run count. Partial storage is not exposed as a successful result; callers
  may grow or replace that bounded buffer and retry.
- Optional 16 × 16 fixed tiles omit empty regions, represent full regions with
  a descriptor and no fine runs, and store boundary coverage as 4-byte
  tile-local runs. The current strip-to-tile converter uses caller-owned
  8-byte sortable pieces and leaves formal tile outputs untouched on failure.
- The optimized direct tile path instead keeps 8-byte linked pieces for one
  active 16-row strip and three caller-owned `u32` arrays per tile column. It
  sorts only touched columns, emits tile-major output at strip boundaries, and
  never retains or reorders whole-frame fine coverage.
- The tile-aware solid compositor consumes full tiles directly and boundary
  runs exactly, but benchmarks do not justify making it the immediate-mode
  default. Tile construction must be amortized by reuse, batching, a more
  specialized pixel pipeline, or later parallel execution.

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

## Performance decisions

- SIMD remains measurement-gated. Per-pixel channel packing and four-pixel
  interleaved NEON kernels both regressed the scalar linear compositor. The
  current array-of-structures target and short spans do not amortize packing or
  deinterleaving; revisit SIMD with long batches or a structure-of-arrays tile
  working buffer.
- The benchmark harness reports span distributions when `UGL_SPAN_STATS=1`.
  The canonical rectangle scene has one-pixel boundary runs around 16–21-pixel
  interiors; full-coverage runs contain about 83% of covered pixels. Future
  batching should leave boundary runs scalar and convert layouts only for
  measured long interior work.
- A separate translucent full-coverage closure intended to omit `scale(1.0)`
  regressed both solid and gradient diagnostics. LLVM already removes the
  trivial scale from the compact general expression; that specialization stays
  rejected.
- Analytic slabs special-case all-vertical active sets only after ordering new
  edges by x. Vertical edges cannot cross pixel boundaries or one another, so
  crossing-event and midpoint-order passes are unnecessary. Sloped sets retain
  the numerically coalesced event algorithm and both ordering passes.

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

## Delivery sequence and gates

The first vertical slice was implemented and validated in this dependency
order. New backends should preserve the same gates:

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

Later stages build on this contract rather than bypassing it.

## Milestones

### M0 — Contract and test foundation

Status: substantially complete; broader golden and benchmark coverage remains.

- This design contract, crate scope, MSRV, and supported target matrix.
- `no_std` core build and independent feature checks.
- Geometry and color property tests.
- Golden-image and benchmark harness skeletons.

### M1 — Floating-point solid path fill

Status: complete.

- Validated paths and affine transforms.
- Adaptive quadratic and cubic flattening.
- Non-zero and even-odd filling with anti-aliased coverage.
- Solid source-over output into borrowed RGBA8888 storage.
- Degenerate, boundary, clipping, and randomized tests.

### M2 — Paint and clipping

Status: complete (2026-07-30).

- Rectangular and path clips.
- Linear, radial, and conic gradients.
- A bounded sampler contract and explicit color-space boundaries.
- Independent, allocation-free paint transforms.
- Golden, randomized-invariant, clipping-composition, and sampler benchmark
  coverage.

### M3 — Stroke

Status: undashed scalar reference implemented; reliability validation ongoing
(2026-07-30).

- Width, cap, join, and miter behavior.
- Degenerate subpaths and self-intersections.
- Allocation-free `Path -> flatten -> stroke expansion -> analytic coverage ->
  paint/composite` using caller-owned point, contour, edge, intersection, and
  row storage.
- Dash patterns only after the base stroke contract is stable.

### M4 — Fixed-point backend

Status: prototype implemented; production validation remains.

- A documented Q format and device-coordinate range.
- Proven intermediate widths and explicit rounding/overflow policy.
- Differential tests against the `f32` reference.
- Representative targets that build without hardware floating point.

### M5 — Fixed-memory rendering

Status: streaming, retained-strip, and retained-tile prototypes implemented.

- Caller-provided edge, coverage, and temporary workspaces.
- No allocation on the render path.
- Capacity errors report required resources where feasible.

### M6 — Performance engineering

Status: in progress.

- Reproducible comparisons with relevant CPU rasterizers.
- Time, peak scratch memory, allocation count, binary size, and image-quality
  results.
- Specialized/SIMD paths only where measurements justify them.

### M7 — Production release

Status: planned.

- Stable public contract and SemVer policy.
- Documented MSRV and supported targets.
- Fuzzing, unsafe review, examples, integration guidance, and real-device
  validation.

## Current status

| Area | Implemented | Remaining production work |
| --- | --- | --- |
| `f32` fill | Sampled and analytic coverage, persistent active edges, sparse row bins, both fill rules | Broader golden scenes and external fuzzing |
| Paint/color | Solid, linear, radial, conic, transforms, encoded compatibility, linear-light compositing | Additional formats and broader quality comparison |
| Stroke | Allocation-free undashed caps and joins | Dashes, fuzzing, and production reliability validation |
| Fixed raster | Checked Q24.8 transforms and path flattening, rational crossings, sparse strips/tiles, clipping, native fixed paint and all fixed stroke caps/joins | Fixed path strokes/dashes, real-device and range validation |
| Performance | Reproducible scalar, paint, stroke, active-edge, retained, and tile benchmarks | Cross-renderer methodology, code size, allocation instrumentation, justified SIMD |
| Release | MSRV and feature CI, 32-bit and no-FPU build coverage | Stable API/SemVer policy, integration guidance, exhaustive unsafe/fuzz review |

The fixed backlog includes `rasterize_path_clip_fixed`: it must convert
prepared fixed path coverage directly into a caller-owned `CoverageMaskMut`.
Fixed compositors already consume arbitrary masks, but current path-mask
generation uses the analytic `f32` backend.

Two cross-cutting API reviews also remain:

- Audit every public and internal `RGBA` use. Each boundary must make
  straight versus premultiplied alpha, encoded sRGB versus linear light,
  component width, and packed-value versus byte-layout semantics unambiguous.
  Replace bare `RGBA` where its contract relies on convention, and add focused
  conversion/boundary tests before removing compatibility aliases.
- Design a stateful `Canvas`/`Context` facade over the low-level rendering
  functions. It should own or borrow target state, current transform, paint,
  clip stack, fill/stroke options, and reusable workspaces where appropriate,
  while retaining the allocation-free low-level APIs. First classify and
  consolidate duplicated render entry points; do not merely move the existing
  API matrix into methods or hide capacity errors and fixed/floating backend
  selection.
- During that API review, evaluate grouping backend-specific implementation
  under `src/fixed/`. The decision must consider feature-gate clarity, module
  cohesion, compile-time dependencies, discoverability, and stable public
  paths. Keep genuinely shared geometry, color, coverage, and compositor
  contracts outside the backend directory; do not duplicate abstractions just
  to create a visually isolated tree. If files move, preserve intentional
  public paths through re-exports or treat changes explicitly as pre-1.0 API
  cleanup.
