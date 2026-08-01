# Blend2D comparison

This directory is the sole third-party comparison harness. It compares the f32
analytic RGBA8888 path with Blend2D's synchronous, single-threaded PRGB32 path,
and reports the fixed backend separately as an embedded/no-FPU tradeoff.
Third-party source and build products are intentionally not vendored.

## Contract

- Scenes: `256x256`; 1, 16, and 64 nested-prefix independent
  fractional-coordinate rectangles, one
  large fractional rectangle with solid, linear-gradient, and concentric
  radial-gradient and conic-gradient paint, 64
  triangles, a large rectangle through a retained circular path mask, an
  eight-cubic closed fill, that
  fill under an integer rectangle clip, the cubic path stroked at width 6,
  and a 32-segment polyline stroked with both butt/miter and round cap/join.
  All use non-zero fill and source-over color `(40, 120, 220, 192)`.
- The linear-gradient scene uses a 256-entry ugl-rs ramp and black stops with
  alpha 32 and 224. This keeps interpolation-space differences out of the RGB
  channels while still exercising ramp lookup and varying-alpha composition.
- The conic scene explicitly selects ugl-rs's `Fast` angle policy, whose
  seventh-degree polynomial matches the approximation class used by production
  vector pipelines. `Exact` atan2/CORDIC remains the public default and is not
  silently weakened by the comparison harness.
- The path-mask scene excludes mask construction. ugl-rs consumes its retained
  8-bit coverage mask in one draw. Blend2D has no public free-path clip entry;
  its explicitly labeled equivalent renders the shape and applies a retained
  PRGB32 mask with `DST_IN`, including the extra image-composition pass.
  ugl-rs mask validation and non-zero-bound derivation are setup and the same
  cached `CoverageMask` value is reused by every timed frame.
- The sparse retained-mask variant uses the same construction and draw contract
  with a radius-24 rather than radius-100 circle, exposing whether cost follows
  non-zero mask density or an unavoidable full-image pass.
- `build_path_mask` separately times clear plus rasterization of the same
  circular path into an 8-bit ugl-rs mask or a white PRGB32 Blend2D image.
  RGBA normalization and file output remain outside the timed region.
- The shared alternating cubic arches stay within y=112..144 so width-6 stroke
  expansion remains in the common non-crossing domain. The inflected
  y=24..232 case
  is retained conceptually as a fixed-backend reliability case: its expanded
  outline currently returns `CrossingEdges` and is not a valid timing input.
- Dashed stroke is deliberately excluded from the cross-renderer table. The
  locked Blend2D revision accepts and retains `dash_array`/`dash_offset` state,
  but its raster stroker does not consume those fields; a configured dashed
  path therefore renders as an undashed stroke. ugl-rs f32/fixed dash costs
  remain covered by the `stroke_dash` Criterion groups instead of being
  compared with semantically different output.
- The matched fixed width-6 round-polyline scene uses four segments per half
  circle, selected with `--fixed-round-segments 4` (the runner default). This
  matches the f32 0.25 px chord tolerance more closely than the conservative
  fixed API default of eight. Alternate counts may be measured explicitly but
  must not be mixed into the synchronized table without relabeling it.
- Setup excluded: image allocation, path construction, context construction,
  and ugl-rs caller-owned scratch allocation.
- Timed frame: clear the complete destination and fill or stroke the retained
  path. Stroke flattening/expansion is included by both renderers.
- Sampling: 500 warm-up frames, then 9 samples of 5,000 frames; compare the
  median and retain min/max as a noise check.
- Cold latency: run nine independent processes with
  `--warmup 0 --iterations 1 --samples 1`; report the median first draw
  separately from warmed throughput.
- Process memory is a separately labeled `time -l` peak-RSS diagnostic. It
  includes runtime, allocator, and JIT state; use ugl-rs planners—not RSS—for
  exact caller-owned scratch requirements.
- Output: each runner emits CSV, an FNV-1a checksum, and optionally normalized
  premultiplied RGBA bytes. The Rust runner reports exact-pixel rate, mean
  absolute channel error, and maximum channel error against Blend2D.

Interpret f32 versus Blend2D as the desktop performance comparison. Interpret
fixed versus f32 as the cost and output delta of deterministic Q24.8 geometry;
fixed versus Blend2D is retained only as a same-host reference, not as evidence
about performance on an MCU or a target without an FPU.

The images are diagnostic, not expected to be byte-identical: the renderers use
different coverage quantization rules. Any performance claim must record CPU,
OS, compiler, Rust version, ugl-rs commit, Blend2D commit, AsmJit commit, and the
complete runner output. Run several times on an idle machine with fixed power
settings; do not compare a debug build with a release build.

## Run

Clone Blend2D and its required AsmJit checkout as described by Blend2D's build
documentation, then run from the ugl-rs repository root:

```sh
benches/blend2d/run.sh /absolute/path/to/blend2d
```

The Blend2D CMake target is static and Release. The simple `BLContext(image)`
constructor selects its synchronous renderer, so no asynchronous queue or
thread-pool work leaks outside the timed loop.
