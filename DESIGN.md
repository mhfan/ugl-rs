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

The primary floating-point backend uses exact-area analytic `f32` coverage and
also serves as the behavioral reference for fixed differential tests. A
separate supersampled `f32` rasterizer remains a quality oracle. Geometry
containers are generic over their coordinate representation so the fixed-point
backend can reuse the scene representation; raster algorithms remain concrete
where operations, intermediate widths, rounding, overflow, or performance
differ.

### Core capabilities

1. Paths, affine transforms, curve flattening, filling, and clipping.
2. Solid paint and source-over compositing into premultiplied RGBA8888.
3. Linear, radial, and conic gradients through a bounded sampler interface.
4. Strokes with caps, joins, miter limits, and dash patterns.
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
- The primary exact-area and sampled-reference rasterizers use `f32`.
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
Edge, Paint, CoverageSink, and borrowed Pixmap contracts:

- **Desktop/mobile high performance:** sparse strips or tiles for locality,
  candidate-edge reduction, and empty/full rejection; analytic cell coverage
  at active boundaries; and optional ahead-of-time SIMD specialization.
- **MCU/fixed memory:** scanline spans or trapezoid decomposition, fixed-point
  analytic boundary area, caller-owned bounded workspace, and streaming
  compositing without a full intermediate mask.

Sparse strips are a spatial index, not a coverage algorithm: fixed rendering
may use bounded strip bins while retaining trapezoid/scanline area evaluation.
Both families may share edge preparation, area formulas, fill semantics, paint
sampling, and compositing. Backend-specific inverse slopes, cell accumulators,
strip IDs, and SIMD layouts do not enter the common `Edge` representation.
The compact `CoverageRun`, `CoverageStrip`, and borrowed `CoverageStrips` storage
contracts are likewise backend-neutral and live in `common`; fixed re-exports
their established names while each rasterizer remains responsible for producing
valid ordered runs.

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
- The fixed device-coordinate format is signed Q24.8 (`fixed::Scalar`): 8
  fractional bits align with 8-bit coverage and its
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
  state explicit. `float::linear::LinearPixmap` retains premultiplied
  linear-light `f32` through source-over and encodes only when presenting into
  RGBA8888. `common::Pixmap` remains the compact encoded-domain compatibility
  and performance path.
- Linear presentation has two explicit modes: `encode_into` is the exact
  transfer-function reference, while `encode_into_with` uses a caller-owned
  4096-entry `Srgb8Encoder` table and is constrained to one RGBA8 code value per
  channel of the reference by tests.
- `LinearPixmap::with_dirty_tiles` optionally borrows one bit per 16×16 tile.
  Coverage spans mark tiles during composition; incremental presentation
  consumes those bits and preserves untouched destination tiles. At 50% dirty
  tile area it switches to contiguous full-frame encoding. Known-dense callers
  should omit tracking to avoid its span-marking cost.
- Integer conversion maps channel extrema exactly and uses round-to-nearest.

## Paint and gradients

- `PaintSampler` returns `PremulSRGBA8` at device-space pixel centers
  and is statically dispatched without allocation.
- `LinearPaintSampler` is a separate explicit contract returning
  `LinearPremulRGBA<f32>` without an encoded round trip. Built-in solid and
  gradient paints implement both contracts; custom encoded samplers do not
  silently opt into linear compositing.
- Fixed streaming, retained-strip, and retained-tile coverage share the encoded
  `PaintSampler` compositor and rectangle/path-mask adapters. This establishes
  functional backend parity without claiming FPU-free paint evaluation:
  existing gradient samplers remain `f32`.
- `fixed::sampler::PaintSampler` is the explicit no-FPU contract.
  Fixed rectangle clips accept `Rect<fixed::Scalar>`, restrict rasterization to
  conservative integer bounds, and multiply boundary coverage in Q24.8/integer
  arithmetic. Native fixed path masks are also no-FPU.
  `fixed::sampler::LinearGradient` accepts Q24.8 endpoints and a caller-owned
  encoded ramp. It uses `i64`
  coordinate deltas and exact `i128` projection, spread mapping, and nearest
  ramp selection: the full Q24.8 endpoint difference squared reaches the edge
  of `i64`, so `i128` is required before summing two axes.
  `fixed::sampler::RadialGradient` supports increasing or decreasing concentric
  radii and general two-circle/focal geometry. Its reduced discriminant is proven within
  `i128` over the fixed device domain; adaptive integer square roots retain up
  to 16 fractional bits, and the same largest-valid-root policy as the `f32`
  reference handles focal cones. Ordinary values take `u64`/`i64` fast paths.
  Static ramps need no allocation or runtime color conversion on an MCU.
- `fixed::math::Angle` stores one binary turn in `u32`, avoiding unit ambiguity and
  floating-point conversion at the fixed conic API boundary.
  `fixed::sampler::ConicGradient` uses 16 integer CORDIC vectoring steps and direct
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
- `GradientStop::new` accepts straight encoded `SRGBA<u8>`. Linear
  interpolation and encoded-domain framebuffer compositing are intentionally
  separate stages.
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
- `fixed::stroke::Options` and `fixed::stroke::stroke_polyline` provide the initial no-FPU
  Q24.8 path for all caps and joins. Integer square-root normalization and
  widened intersection tests preserve bounded arithmetic.
  `fixed::canvas::render_stroke_polyline` connects caller-owned edge/line scratch
  directly to fixed raster and paint. Round geometry shares the binary-angle
  CORDIC with fixed conic paint. Its explicit `round_segments` count is per
  half circle, making edge capacity and the chord error
  `r · (1 - cos(π / segments))` predictable without runtime transcendental
  functions.
- Dash patterns borrow validated alternating lengths and preserve each
  backend's numeric contract. Each contour restarts the normalized phase;
  closed seams merge a continuing on-interval so it receives a join rather
  than two caps. Decomposition preflights caller-owned point/contour capacity
  before writing.

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
- Public fill/stroke/dash planners run geometry without a target and return
  exact backend-specific render capacities once caller-owned planning scratch
  is sufficient. Planning is staged because stroke expansion determines edges
  and actual edges determine row/strip index counts; no API presents a loose
  formula-derived upper bound as an exact requirement.
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

### Coverage execution policy

- Ordinary `Canvas` drawing streams coverage directly into the compositor.
- Retained path clips and reusable masks use 16-row sparse strips when their
  descriptors and runs are smaller than packed local coverage; otherwise they
  stay packed. Their non-zero bounds are cached when retained.
- Tiles are an explicit low-level representation for repeated replay or future
  parallel scheduling. The facade does not silently convert immediate coverage
  or strips into tiles, because current scalar measurements do not repay that
  construction cost.

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
unification cannot hide broken `no_std`, fixed-point, or allocation
configurations. The declared MSRV is Rust 1.93; CI also checks stable Rust,
32-bit Linux, and a Cortex-M target without an FPU.

## Performance decisions

- SIMD remains measurement-gated. Single-pixel packing and four-pixel
  interleaved NEON kernels regressed earlier experiments. A later two-pixel
  `u64` encoded-source-over loop, with a transparent-pair overwrite path,
  improved all synchronized f32 scenes by roughly 5–10% and is retained with
  randomized scalar-equivalence coverage. Wider target-specific SIMD still
  needs long-span dispatch or a structure-of-arrays tile working buffer to
  amortize packing and deinterleaving.
- Encoded paint samplers expose affine span traversal in addition to point
  sampling. Linear gradients advance one projection parameter per pixel; the
  fixed implementation does the same in widened integer arithmetic. Direct
  full-coverage writes over transparent destinations avoid redundant coverage
  multiplication. Together these changes reduced the matched large-gradient
  draw from 381.33 to 192.50 µs for f32 and 446.92 to 221.24 µs for fixed.
  Direct Pad-ramp traversal plus vertical-run emission reduced f32 to 62.60 µs.
  Fixed span projection narrows parameter, step, denominator, and terminal value
  together before entering its i64 ramp mapper, reducing the fixed draw from
  187.13 to 120.60 µs after paired sampled-pixel composition. Both backends
  remain byte-identical. Blend2D is still
  1.98× faster than f32, so future work should batch ramp lookup/output rather
  than further tune path coverage for this scene.
- Concentric radial samplers advance squared distance with a second-order
  difference across each span instead of rebuilding coordinates and products
  per pixel. Scheduling four recurrence values together, while preserving the
  scalar update order, lets the compiler overlap independent square roots. The
  output checksum is unchanged; the matched f32 median fell from 202.22 through
  123.98 to 115.96 µs, and fixed from 542.18 through 338.54 to 272.79 µs. Fixed
  reduction comes from returning Pad endpoints before the invariant ramp-index
  division, then dividing a concentric Pad row into constant left/right regions
  and one branch-free interior recurrence. A per-pixel squared-distance endpoint
  classifier was rejected because it nearly doubled the predominantly interior
  radius-180 sampler; geometric row partitioning keeps that diagnostic near
  441 µs while accelerating the radius-112 matched scene. Blend2D measures
  41.41 µs, leaving SIMD square-root throughput and encoded ramp/compositor
  batching as the measured paint costs. A public sampler callback that encoded
  repeated-color runs was also rejected: despite routing the constant regions
  through the existing solid-span blender, it changed the interior compositor
  from 274.17 to 306.49 µs. Constant-run transport therefore stays out of the
  public paint contract until a representation can preserve current codegen.
  Computing four independent complete integer roots was likewise rejected: it
  preserved every sample but raised the radius-112 diagnostic from about 372.5
  to 435.9 µs because extra Newton iterations outweighed instruction-level
  parallelism. Future batching must retain the nearby-root iteration bound.
- The matched conic scene explicitly uses the opt-in `Fast` angle policy while
  `Exact` remains the default. f32 uses the documented seventh-degree unit-angle
  approximation; fixed evaluates the same polynomial in widened integer turns
  instead of 16 CORDIC steps. Encoded span traversal reuses coordinates and
  direct ramp indexing. Keeping the Q32 normalization division widened while
  narrowing the provably bounded Horner products from i128 to i64 reduces the
  formal fixed median from 379.91 to 252.59 µs; f32 measures 184.19 µs and
  Blend2D 68.02 µs. Fixed differs from f32 at 2 of 65,536 pixels, each by one
  code value; the optimization adds neither allocation nor floating-point work.
- Retained path masks scan equal coverage runs in eight-byte words before
  forwarding spans. `CoverageMask` caches non-zero bounds at retained-resource
  construction, and both rasterizers constrain coverage work to that region;
  the f32 sink continues word-wise filtering inside it. Radius-24/radius-100
  density scenes measure 6.06/23.20 µs for f32, 7.74/31.86 µs for fixed,
  and 29.77/29.78 µs for Blend2D. This preserves the generic coverage-sink
  contract and fixed memory. Blend2D has
  no public free-path clip; its comparison is a retained PRGB32 `DST_IN`
  emulation and must remain labeled as such.
- Building the circular mask separately measures 20.51 µs for f32, 46.78 µs
  for fixed, and 9.04 µs for Blend2D. RGBA normalization is excluded. Direct
  disjoint-row emission closed much of the former gap; the remainder belongs
  to curve flattening and coverage rasterization, not retained-mask lookup or
  source-over composition.
- Integer rectangle clips classify and convert their boundaries once when the
  sink is constructed instead of repeating four `floor` checks for every span.
  Direct render entry points also pass their compositor straight to the
  region-bounded rasterizer for integral clips, removing the adapter branch;
  fractional boundaries retain exact antialiased multiplication. The matched
  clipped cubic now measures 11.10 µs f32 and 17.83 µs fixed with unchanged
  checksums.
- Nested-prefix 1/16/64-rectangle scenes separate fixed frame overhead from
  edge-count slope. Current f32 medians are 4.02/17.58/59.51 µs versus
  Blend2D's 3.39/11.63/33.20 µs. The widening fill gap belongs to repeated edge,
  coverage-run, and composition work rather than clear or runner overhead.
  Direct vertical-run emission removes dense cell scans for unchanged vertical
  active sets. Fixed initially measured 9.43/60.31/238.70 µs. Coverage attribution
  showed 203.61 µs in its raster stage; direct vertical-trapezoid boundary area
  reduced that to 144.04 µs; guarded direct trapezoid emission brings the
  current 1/16/64 draws to 4.82/28.38/106.70 µs. Sloped edges retain polygon
  clipping and exact rational
  crossings, while axis-aligned rectangles no longer pay that general cost.
- The benchmark harness reports span distributions when `UGL_SPAN_STATS=1`.
  The canonical rectangle scene has one-pixel boundary runs around 16–21-pixel
  interiors; full-coverage runs contain about 83% of covered pixels. Future
  batching should leave boundary runs scalar and convert layouts only for
  measured long interior work.
- Cold first-frame latency is sampled through nine independent processes with
  zero warm-up and one timed draw. Large solid/cubic/linear-gradient medians are
  47.38/49.21/96.25 µs for f32, 144.25/68.92/284.29 µs for fixed, and
  365.88/371.54/381.04 µs for Blend2D. This intentionally includes Blend2D's
  first pipeline JIT but excludes resource construction for every backend; it
  is a latency diagnostic and must not replace warmed throughput results.
- A warmed 64-rectangle `time -l` diagnostic reports 2.89 MiB peak RSS for the
  Blend2D process, 1.92 MiB for f32, and 2.55 MiB for fixed. Harness executable
  sizes are 1.87 MiB and 0.58 MiB respectively, but the latter links both Rust
  backends. These values include runtime/allocator/JIT state and never replace
  planner-derived scratch capacities or a target-specific code-size build.
- A separate translucent full-coverage closure intended to omit `scale(1.0)`
  regressed both solid and gradient diagnostics. LLVM already removes the
  trivial scale from the compact general expression; that specialization stays
  rejected.
- Analytic slabs order newly activated edges before specializing all-vertical
  sets. If an unchanged vertical active set spans the next complete row, a
  winding-aware emitter reconstructs boundary pixels and full spans directly;
  a pending boundary cell merges disjoint intervals that share one pixel.
  This avoids both reintegration and dense-cell rescans, reducing the matched
  64-rectangle draw from 83.67 to the current 59.51 µs without changing output.
- The core remains `no_std` capable, while default desktop builds enable
  `std`. Floating-point capability is independent: `std` uses platform
  floor/ceil, Arm `eabihf` targets select a hardware-friendly no_std
  implementation automatically, other no_std FPU targets opt into
  `native-float`, and soft-float targets use `libm`. The dependency's `arch`
  dispatch may select a tested target implementation where available and
  otherwise falls back to portable software. Release profiling measured the
  desktop native path about 19% faster after event-scan fusion.
- Every f32 math operation is selected by the private `float` backend rather
  than renderer modules calling `libm` directly. Basic FPU availability does
  not imply sin/cos/atan2/acos/pow hardware; no_std transcendental operations
  generally remain software unless `libm` or another target backend provides
  equivalent semantics and proven code generation. Platform-specific
  acceleration must not change the ABI or silently enable incompatible rustc
  target features.
- Open non-degenerate strokes emit one boundary contour instead of independent
  overlapping segment and join polygons. The matched 8-cubic stroke therefore
  expands to 65 centerline points and 130 rather than 480 edges. Current stage
  measurements put flatten, outline expansion, and row binning near 4 µs
  combined, sparse-cell coverage near 22.5 µs, coverage plus encoded blending
  near 29.9 µs, and the complete draw near 34.7 µs. Blend2D measures 14.6 µs
  on the same harness. Prepared stroke remains useful for retained content,
  but analytic coverage math and batching dominate the remaining desktop gap.
- The fixed stroker now uses the same compact-boundary policy for regular open
  polylines, with a pure-Q24.8 intersection and CORDIC arc implementation and
  the previous polygon-union path retained for repeated/reversing degeneracy.
  On the synchronized host benchmark this reduced the eight-cubic fixed stroke
  from 284.75 to 64.89 µs; butt/miter and round 32-segment polylines measure
  133.42 and 181.42 µs respectively. Full-height overlapping trapezoids now
  accumulate through the same integer clamp primitive as disjoint direct rows,
  retaining the caller-owned area row while avoiding per-pixel polygon
  clipping; partial-height and crossing slabs retain the general clipper. The
  matched width-6 round scene uses four
  fixed segments per half circle, corresponding to the f32 backend's 0.25 px
  tolerance at this radius. The fixed API default of eight remains conservative
  because one segment count cannot represent a pixel-error tolerance across all
  widths. Four segments reduce coverage from about 236 to 175 µs and also
  improve fixed-vs-f32 error from 1.184% / max 37 to 0.752% / max 1.
- Dash decomposition is benchmarked internally for both backends but excluded
  from the Blend2D matrix. The locked Blend2D revision retains dash state yet
  does not consume it in the raster stroker, so its apparent dashed timing is
  actually an undashed path and cannot support a valid comparison claim.
  Separate 64-point Criterion cases distinguish decomposition (2.324 µs f32,
  6.578 µs fixed) from decomposition plus outline expansion (5.017 µs f32,
  16.546 µs fixed). Fixed preserves exact integer length, rational endpoint
  interpolation, and a complete capacity preflight; replacing that contract
  with partial writes is not an acceptable benchmark-only optimization.
- The production analytic-cell path stops slabs only at edge starts, ends, and
  real crossings. It integrates boundary cells with the closed-form primitive
  of `clamp(edge_x - cell_x, 0, 1)`, records full intervals with two range
  deltas, and fuses the prefix scan with run emission. Dirty x bounds restrict
  clearing and emission; scratch is one 8-byte `Cell` per target column.
  Retained active edges stay ordered across rows, newly appended edges are
  merged into that prefix, and crossing candidates are rejected by
  multiplication before division. Numerically coalesced reversals use a cold
  split-integral path. Dense analytic, sparse analytic, and high-sample
  randomized references cover NonZero, EvenOdd, coincident, crossing, and
  self-intersecting geometry.
- Event-free full f32 rows with disjoint filled-span pixel envelopes bypass
  the cell array: boundary cells are integrated analytically and full
  interiors are emitted directly. Preflight rejects touching, overlapping,
  crossing, and partial-height spans, which use the general accumulator.
- Fixed full rows apply the same guarded direct-emission policy to ordered,
  disjoint trapezoids. Boundary coverage is evaluated with the integer
  piecewise primitive of `clamp(edge_x - pixel_x, 0, 256)`; no floating-point
  operation or dense row buffer is required. Full-row overlapping envelopes
  reuse the same closed-form Q24.8 area while accumulating into the row buffer;
  multi-slab or crossing geometry retains exact rational events and polygon
  accumulation.
  Rounded rational endpoints are cached once per trapezoid traversal rather
  than recomputed for validation, bounds, interiors, and boundary pixels; the
  narrowing the proven single-row integral from i128 to i64 and using the same
  integral for full-row overlap reduced the matched fixed triangle draw to
  130.68 µs while retaining its previous checksum.
- Rejected analytic experiments remain explicit decisions: generic polygon
  clipping and a whole-row difference accumulator did not amortize their work;
  removing midpoint ordering broke self-intersections; hybrid introsort
  regressed stable and ordinary churn; and factoring the duplicated hot
  fill-span traversal through a closure helper regressed current end-to-end
  rectangle and stroke diagnostics by roughly 2–3%. These hot loops stay
  specialized until code-generation evidence changes.
- A fixed conic `sample_span` that hoisted coordinate checks and incremented
  raw x regressed the synchronized draw from 386.78 to 424.40 µs. Isolated
  Fast-angle span traversal was also slower than the fully inlined point loop.
  The callback-shaped specialization is therefore rejected; future conic work
  must fuse sampling with composition or prove a batch representation first.
- Factoring four-way f32 concentric recurrence scheduling through another
  generic callback erased its end-to-end benefit. The encoded and linear hot
  loops therefore retain explicit batches: the former improves the matched draw
  by about 6.5%, while the latter more than halves its isolated span diagnostic.
- Replacing fixed linear Pad-ramp division with an i128 quotient/remainder
  recurrence did not improve the isolated span diagnostic and regressed the
  complete draw by roughly 2–3%. The existing narrowed i64 mapping remains.

### micro{gl} comparison hypothesis

This is an algorithmic prior, not benchmark evidence. The reviewed upstream
revision is `d7ddab9890ae6b391bc646b7086e695c06260abb`:

- `path::tessellateFill` and `tessellateStroke` cache their output until path or
  tessellation options change. Repeated draws can therefore exclude geometry
  decomposition, unlike the current ugl-rs immediate stroke benchmark.
- Path fill/stroke reaches `drawTriangles`; each triangle walks its clipped
  integer bounding box with incremental edge functions. This is compact and
  predictable, but cost follows the sum of triangle bounding-box areas rather
  than only emitted coverage. Long, skinny, overlapping, or heavily subdivided
  triangles can therefore pay rejection tests and overdraw that scanline spans
  avoid.
- Path AA is optional and examples commonly instantiate it as `false`. When
  enabled, triangle coverage uses a one-pixel signed-distance ramp on boundary
  edges. It is cheaper than exact analytic integration but does not implement
  the same coverage rule, so non-AA or approximate-AA results must not be placed
  in the existing Blend2D table.
- Geometry accepts configurable `Q` types, while the triangle rasterizer maps
  transformed coordinates to integer subpixels and selects 32/64-bit arithmetic
  through compile-time canvas options. This is conceptually close to ugl-rs
  fixed's deterministic integer pipeline, but the dominant algorithms differ:
  tessellation plus triangle bboxes versus directed edges plus sparse strips and
  analytic trapezoid area.
- micro{gl} has specialized circles, rectangles, rounded rectangles, and
  triangles. Such primitives may beat a generic path route. Comparisons must
  report specialized and path APIs separately instead of crediting one to the
  other.

Expected ordering by scene is consequently conditional:

| Scene property | Expected advantage | Reason |
| --- | --- | --- |
| cached simple mesh, AA off | micro{gl} | no retessellation and minimal integer inner loop |
| cached modest mesh, approximate AA | micro{gl} may lead | cheaper boundary rule, but different quality contract |
| many skinny/overlapping triangles | ugl-rs fixed may lead | span work follows covered rows rather than summed bboxes |
| dynamic complex path | workload-dependent | micro{gl} tessellation versus ugl-rs flatten/bin/raster costs |
| exact-area AA requirement | ugl-rs | micro{gl}'s reviewed triangle AA is not equivalent |
| retained sparse coverage or mask reuse | ugl-rs | explicit sparse strips/tiles and cached non-zero mask bounds |

A future matched harness must use the same RGBA8888 target, source-over alpha,
256×256 scenes, warm-up/sample protocol, and include clear. It must publish four
separate micro{gl} rows where supported: cached/non-cached tessellation crossed
with AA off/on. Quality deltas must accompany AA-on timing. Only the cached,
AA-on path row is a meaningful approximation to retained production drawing;
none should be inferred from desktop fixed-versus-Blend2D ratios.

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

Status: scalar f32/fixed dash, cap, join, and `CanvasRef` entry points implemented;
reliability validation ongoing (2026-07-31).

- Width, cap, join, and miter behavior.
- Degenerate subpaths and self-intersections.
- Allocation-free `Path -> flatten -> stroke expansion -> analytic coverage ->
  paint/composite` using caller-owned point, contour, edge, intersection, and
  row storage.
- Allocation-free dash decomposition with normalized phase, repeated odd
  patterns, and closed-contour seam merging.
- Non-accumulating segment-relative cut placement, explicit f32 precision
  exhaustion, and randomized f32/fixed bounded-output validation.
- Exact f32/fixed dash workspace requirements and transactional capacity
  preflight before caller-owned output is modified.

### M4 — Fixed-point backend

Status: prototype implemented; production validation remains.

- A documented Q format and device-coordinate range.
- Proven intermediate widths and explicit rounding/overflow policy.
- Differential tests against the `f32` reference.
- Representative targets that build without hardware floating point.
- The `f32` and `fixed` backends are independently selectable. A pure fixed
  no_std build compiles no f32 renderer or floating sampler and has no `libm`
  dependency.

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
| `f32` fill | Exact-area primary path, sampled reference, persistent active edges, sparse row bins, both fill rules | Broader golden scenes and external fuzzing |
| Paint/color | Solid, linear, radial, conic, transforms, encoded compatibility, linear-light compositing | Additional formats and broader quality comparison |
| Stroke | Allocation-free f32/fixed dashes, caps, joins, and path stroke pipelines | Fuzzing and production reliability validation |
| Fixed raster | Checked Q24.8 transformed path fill/stroke/dashing, rational crossings, sparse strips/tiles, clipping, and native fixed paint | Real-device and range validation |
| Performance | Reproducible scalar, paint, stroke, active-edge, retained, and tile benchmarks; matched Blend2D fill/stroke harness | More paint/clip scenes, incremental code size, allocation instrumentation, justified SIMD |
| Release | MSRV and feature CI, 32-bit and no-FPU build coverage | Stable API/SemVer policy, integration guidance, exhaustive unsafe/fuzz review |

Both analytic f32 and Q24.8 fixed paths can convert arbitrary path coverage
directly into caller-owned `CoverageMaskMut` storage. The fixed path-mask route
uses the existing bounded geometry and raster workspaces, so mask production
and consumption remain allocation-free and no-FPU.

### Backend feature split

The explicit backend split is implemented with these contracts:

- `src/common/` owns generic geometry plus backend-neutral color, target,
  coverage, and workspace protocols; it must not contain nested backend
  implementations;
- `src/float/` owns f32 math, edge preparation, dash/stroke expansion,
  rectangle clipping, rasterization, paint, and facades; `src/fixed/` owns the
  Q24.8 equivalents;
- an f32 feature gates the analytic rasterizer, floating stroke/dash/paint
  implementations, their public entry points, tests, examples, and benchmarks;
- `fixed` alone provides a complete renderer and compiles without floating
  rendering code or `libm`;
- `libm` becomes optional with the no_std f32 math backend that requires it,
  while hosted f32 builds continue to use `std` platform math;
- shared geometry, color, target, coverage, and facade code must not regain
  duplicated f32/fixed implementations merely to satisfy feature boundaries;
- CI checks hosted f32, no_std f32, hosted fixed, pure no_std fixed, and the
  combined configuration, including representative no-FPU targets;
- `cargo tree --no-default-features --features fixed -e normal` contains no
  `libm`; binary-size and compile-time tracking remain release work.

Cargo cannot activate an optional dependency from the absence of `std`, so the
positive `f32` feature enables optional `libm`; hosted builds additionally enable
`std`, while `fixed` alone selects neither.

### Clip/mask bounds optimization

Owned `Canvas` clips retain only their non-zero integer bounds. The internal coverage-mask view carries a device
origin, treats samples outside its storage rectangle as zero, and keeps the
logical target dimensions for validation. Rectangle and nested path
intersection therefore visit only the retained region; borrowed public masks
remain zero-copy and full-canvas by default.

Path clips derive conservative bounds from prepared edges and rasterize
directly into local storage without rebuilding geometry or allocating a
canvas-sized temporary mask. Empty masks, opaque integer rectangles, and
general coverage are classified once. The first two become empty/rectangle
clip state and bypass per-byte mask multiplication.

Both owning canvases use the same backend-neutral 16-row strip/run encoding,
ordered sparse intersection, dense multiplication, and encoded-byte selection
policy. They retain sparse coverage only when its exact record payload is
smaller, and consume it directly during fill, stroke, and dashed stroke. Dense
storage remains preferable for compact or highly fragmented masks. Rectangle
intersection walks only retained runs; f32 and Q24.8 provide their own
fractional boundary-coverage calculation.

A 512×512 diagnostic benchmark (`cargo bench --bench raster --all-features --
clip_mask --quick`) measures both initial classification/retention and cached
drawing. A one-pixel diagonal occupies 512 runs plus 32 strip headers: 6,528
payload bytes versus 262,144 dense bytes. The original diagnostic measured
about 148 µs for a f32 dense diagonal and 59 µs for fixed sparse storage; f32
now uses the same sparse representation, so the dense result is historical.
Empty and opaque-rectangle clips measured about 9–13 µs. Counting non-zero
samples lets both backends skip exact run counting when even the worst-case
encoding is smaller. These quick measurements identify trends rather than
release thresholds.

Dense/sparse clip multiplication and deterministic scattered-mask rectangle
intersection are checked pixel-for-pixel. New masks select their representation
before intersection, so two sparse masks use an ordered merge-join with work
proportional to their run counts. Deterministic randomized sparse/sparse,
sparse/dense, nested path/path, and `save`/`restore` tests compare against
independently rendered or scalar dense results. A longer state-machine test runs
eight deterministic 256-operation sequences across clear, rectangle, empty,
opaque, dense and sparse masks, free paths, and nested `save`/`restore`. Its
independent reference retains geometric rectangles until rasterization so the
comparison does not introduce an artificial second 8-bit quantization.

The isolated `clip_alloc` integration test installs a counting system allocator
without slowing the normal benchmark binary. For both backends, the 512×512
diagonal reports four allocations and 6,608 allocated/peak bytes for initial
retention and sparse rectangle intersection. A warmed draw and warmed
`save`/`restore` perform zero allocations. Cold/warm slender path construction
peaks at 25,300/80 bytes for f32 and 24,316/80 bytes for fixed. Mutating a saved
dense fixed 64×64 clip uses three allocations and peaks at 8,736 bytes while
copy-on-write storage and its normalized sparse result briefly coexist. Direct
borrowed-mask encoding was required here: the earlier
dense-copy-first route peaked at 524,304 bytes despite its small final mask.
Remaining work is nondeterministic/property fuzzing and real-device allocator/
code-size measurements. A public pre-encoded sparse-mask entry point is not
planned: exposing strip/run storage would leak a backend layout through the
Canvas facade. Internal producers feed retained sparse coverage directly when
profiling shows that dense encoding is material. Both path-clip pipelines now
write an owned run encoder, recognize opaque rectangles, retain compact runs,
or reconstruct a local dense mask only when it is smaller. For fixed, this
originally reduced clip construction
from about 375 to 39 µs for a slender path, 588–597 to 60 µs for a polygon, and
650 to 85 µs for a complex checker grid (roughly 7.6–9.8×). Cold construction
of that 512×512 slender path, including scratch growth, uses 11 allocations and
peaks at 24,316 bytes instead of allocating a 262,144-byte local mask.
`CanvasStorage` now recycles uniquely owned strip/run vectors across `clear_clip`
and `restore`; the warmed slender case consequently performs only the two 40-byte
`Rc` control-block allocations. Its approximately 39 µs latency is unchanged,
confirming that fixed rasterization rather than allocation is now dominant.

The f32 path benchmark measures warm/cold construction at about 18/80 µs for
slender coverage, 32/100 µs for the polygon, and 38/114 µs for the checker grid.
The earlier fixed measurements are about 39/99 µs, 61/132 µs, and 85/178 µs
respectively. Measured fixed
intersection costs are about 70–83 µs for path/rectangle, 142–150 µs for
path/dense-mask, 159–160 µs for path/path, and 73–74 µs for a warmed
`save`/path/`restore` loop. Drawing the same full-target shape costs 125–132 µs
without a clip, 151–160 µs with a rectangle, 192–198 µs through the slender
sparse path, and 146–154 µs after two paths reduce the surviving region.
The fixed sparse compositor now keeps a monotonic strip/row/run cursor for one
rasterization call instead of repeating binary and partition searches for every
span. The slender sparse-path draw fell to about 177–181 µs (roughly 8% by the
quick-run centers); rectangle and already-small nested clips were unchanged.

Sparse/dense selection deliberately compares logical encoded bytes rather than
vector capacity. Every short span pays for a complete `CoverageRun`, so this
already incorporates run count, fragmentation, and average run length. Spare
capacity remains owned by the Canvas in either representation; including it in
the decision would make identical coverage choose differently based on history.
The measured sparse replay cost does not yet justify an additional arbitrary
time-weighting constant.

The circular-mask stage benchmark isolates initial construction. F32 edge
building, row binning, and coverage integration measure about 1.44, 0.74, and
13.60 µs; fixed edge building, line preparation, and coverage measure about
1.29, 0.14, and 34.82 µs. Coverage owns roughly 88% of the visible f32 stages
and 96% of fixed. Single-span overlap specialization, single-pass internal
trapezoid generation, event-driven active-edge sorting, and the same-cell
closed-form edge integral reduce fixed coverage from about 41.60 to 37.00 µs
(about 11%); the corresponding f32 single-span specialization improves about
1%. A further split measures fixed strip binning at about 1.02 µs and synthetic
full-span run emission at 0.18 µs, confirming that active/event processing and
boundary integration own nearly all remaining time. Fixed `StripBins` can now
be prepared once in `RasterWorkspace` and replayed through
`rasterize_lines_binned`; binding checks reject reuse with a different target
height or line count. A dedicated two-active-edge trapezoid branch was also
benchmarked and rejected because it duplicated the general integration work
without improving the circular-mask result. Further path-mask work should
target active/event processing and boundary integration, not curve flattening,
binning, or run emission.

A structure-of-arrays active-edge workspace is not currently justified for the
dominant convex-mask case: nearly every slab has only its left and right edge,
while SoA would add parallel caller-owned buffers and reconstruction work to
all fixed renders. It remains a candidate for measured high-active-count paths,
not a default replacement for the compact `Segment` workspace.

Full-row trapezoids with a non-empty interior integrate only the left edge for
their left boundary pixels and only the right edge for their right boundary
pixels; the opposite edge is provably full or zero there. This reduces circular
coverage from about 36.69 to 34.82 µs in a same-run A/B and the formal fixed
mask build from 42.33 to 41.29 µs. Narrow trapezoids retain the general two-edge
integral, and the self-intersecting stress scene remains near 99.2 µs. Reusing
the previous slab's exact `bottom_x` as the next `top_x` was correct but slowed
circular coverage from 34.82 back to 36.69 µs; the added loop-carried state
outweighed one avoided intersection division and remains rejected.

The sparse f32 cell emitter now jumps over zero cell ranges after applying a
range delta instead of quantizing every full-interior pixel. Circular coverage
falls from roughly 15.66 to 13.60 µs (about 13%), while the new 16-bow-tie
EvenOdd stress scene improves more modestly from about 49.0 to 47.8 µs. Its
fixed counterpart measures about 98.4 µs; 1 px zig-zag stroke coverage measures
about 52.4 µs f32 and 121.7 µs fixed. These cases, plus the existing 256
short-edge grid, guard against optimizing only convex paths with long interiors.

Retained memory, initial mask allocation, rasterization, and subsequent
intersection now scale with the clipped region. Real-device allocator and
code-size measurements remain benchmark work.

The framebuffer boundary now distinguishes raw storage from valid color:
solid paint and gradient-stop inputs use straight encoded `SRGBA<u8>`;
`Pixmap::pixel_bytes` exposes physical RGBA bytes unchanged; and `pixel`
returns only validated `PremulSRGBA8`. Construction remains O(1) with
respect to image area and therefore does not scan caller-owned destination
contents. Compositing over existing bytes requires the caller to uphold the
premultiplied invariant.

The f32 facade stores `CompositeMode` as drawing state. Porter-Duff operators act
on premultiplied values; W3C color blend functions temporarily unpremultiply
RGB in the target's explicit working space and premultiply the result again.
The RGBA8888 compatibility target performs that work in an integer kernel: u8
storage is widened to `i32` using a UNORM15 scale for unpremultiplication and
blend evaluation, and is quantized only when the final premultiplied pixel is
written.
The f32 formulas remain the differential-test oracle rather than part of the
per-pixel hot path.
Coverage interpolates between the complete operator result and the original
destination, which preserves antialiased semantics for operators such as
`Clear`, `Copy`, and `DstIn`. `Pixmap` therefore blends in encoded-sRGB space,
while a linear target must use the separate linear-light compositor; the API
does not label encoded-domain compatibility output as linear compositing.

## Canvas and CanvasRef facade and backend organization

`Pixmap` owns or borrows compact RGBA8888 storage, while `LinearPixmap` owns or
borrows its linear working buffer. They remain separate concrete types so the
compositing domain and the explicit presentation boundary cannot be inferred
incorrectly through a generic pixel-format abstraction. Neither is a drawing
state machine. The bounded drawing facade is therefore named `CanvasRef`: it
borrows a target and
caller-supplied workspace slices by value, retains small drawing state, and
delegates to allocation-free functions. Those low-level functions remain
public expert APIs for retained coverage, custom sinks, exact capacity
planning, and applications that keep state elsewhere.

`Canvas` (implemented by `float::context::Canvas` and re-exported at crate root) and
`fixed::Canvas` are the ordinary allocation-backed facades. Each owns and reuses
backend-specific scratch, performs
exact planning and any growth before drawing, and then delegates to `CanvasRef`.
Consequently its public workflow does not expose edge, intersection, row-bin,
or coverage-row storage. `CanvasRef` remains the bounded zero-allocation
boundary; low-level workspace layout belongs to expert APIs.

`Canvas::new` owns a tightly packed zero-initialized RGBA8888 destination;
`Canvas::from_buffer` borrows an externally managed destination with explicit
stride. Both use the same internal target abstraction, and `target()` exposes
only layout and pixel bytes rather than a raster pipeline object.

The facade uses two concrete, deliberately parallel entry points:

- `float::context::CanvasRef` selects the analytic f32 geometry/raster path and the encoded
  compatibility compositor.
- `fixed::context::CanvasRef` selects Q24.8 geometry/rasterization and fixed paint sampling.
  Compatibility `PaintSampler` entry points remain available explicitly but
  must not be mistaken for a no-FPU path.

A public backend trait is intentionally avoided. Associated scalar, flatten,
stroke, sampler, workspace, and error types would expose implementation
machinery and make ordinary calls harder to infer. Instead, both borrowed facades reuse
a generic private/shared state record parameterized by coordinate, flatten,
and stroke option types. Their method names and state transitions stay
isomorphic where semantics match; concrete methods remain where the numeric or
paint contract genuinely differs. An internal sealed execution trait may be
introduced only after two implementations demonstrate a useful common body.

The first stable method vocabulary is small:

- `set_transform`, `set_fill_rule`, `set_flatten`, `set_stroke`, `set_color`,
  and byte-valued `set_global_alpha` update current state and return `&mut Self`
  for compact setup. Global alpha applies after sampling, so custom paints and
  solid colors follow the same rule.
- `fill` and `stroke` use the current solid paint.
- `fill_with` and `stroke_with` accept a statically dispatched sampler without
  storing trait objects or allocating.
- `stroke_dashed` and `stroke_dashed_with` accept a borrowed validated pattern
  rather than storing it in context state; their additional point/contour
  buffers are explicit in `float::context::Workspace`.
- clipping is context state, represented as no clip, an empty clip, one
  rectangle, or coverage. Both owning canvases retain accumulated local path
  masks; fixed storage may use dense coverage or sparse strips. Clip state is
  scoped with drawing state through `save`/`restore`.

Status: owning and borrowed f32/fixed fill/stroke/dash facades are implemented.
Both share generic state storage and parallel method
names; rectangle/mask clip state and statically dispatched custom paint are
supported. `Canvas::set_clip_path` provides ordinary owned path clipping;
bounded `CanvasRef` and low-level callers use `rasterize_path_clip` with a
caller-owned `CoverageMaskMut`, then borrow it with `set_clip_mask`.
Both owning canvases implement intersecting rectangle, mask, and free-path clips
without a temporary full-canvas mask. `fixed::Canvas` uses Q24.8 bounds and
integer mask intersection throughout, so this route retains its no-FPU contract.
The bounded `CanvasRef` deliberately retains a single borrowed clip;
callers that require a bounded clip stack own its mask storage explicitly.
Exact fill/stroke/dash planning is available
both through low-level functions and `CanvasRef` methods; path clips reuse the fill
planner through the semantic `path_clip_requirements` entry point.

All methods preserve existing error and mutation contracts. Geometry/capacity
failure before rasterization leaves the target unchanged. Once span emission
begins, sink/raster errors follow the documented low-level behavior. `CanvasRef`
construction performs no allocation and does not infer or resize workspace.
`Canvas` performs requirement planning before growing its reusable storage.

### Fixed source layout

Backend-specific implementation lives under `src/fixed/`:

```text
src/fixed/
    canvas.rs
    context.rs
    dash.rs
    flatten.rs
    math.rs
    raster.rs
    sampler.rs
    stroke.rs
    tile.rs
```

The f32 counterpart, including its adaptive curve flattener, lives under
`src/float/`. Shared generic geometry, edge/line sinks, coverage, encoded color,
target storage, and paint values remain under `src/common/`; sampler traits stay
backend-specific. The canonical public paths are rooted
at `fixed::*`; redundant crate-root aliases and `Fixed`/`_fixed` affixes are
omitted inside that namespace. Cross-backend call sites add local import aliases
only where names collide.

### Engineering gates for the facade

- Add API-level golden tests that render the same scenes through the facade and
  low-level functions; exact output must match.
- Keep the implemented f32/fixed facade differential scenes byte-identical for
  shared fill/stroke/dash state, rectangle/path clipping, and save/restore.
- Keep static dispatch and inspect benchmark deltas; facade calls should inline
  to the existing pipeline with no allocation and no measurable steady-state
  overhead.
- Fuzz state transitions, malformed paths, capacity errors, and save/restore
  underflow separately from raster geometry fuzzing.
- Document which fixed methods are completely no-FPU. Do not let an encoded
  compatibility sampler or f32 rectangle clip weaken that claim implicitly.
- Treat workspace structs and exhaustive error enums as pre-1.0 API until
  planner contracts stabilize and real MCU builds and code-size measurements
  are in release gates.
