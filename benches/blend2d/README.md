# Blend2D comparison

This directory is the sole third-party comparison harness. It compares the f32
analytic RGBA8888 path with Blend2D's synchronous, single-threaded PRGB32 path,
and reports the fixed backend separately as an embedded/no-FPU tradeoff.
Third-party source and build products are intentionally not vendored.

## Contract

- Scenes: `256x256`; 64 independent fractional-coordinate rectangles, one
  large fractional rectangle with solid and linear-gradient paint, 64
  triangles, an eight-cubic closed fill, that
  fill under an integer rectangle clip, the cubic path stroked at width 6,
  and a 32-segment polyline stroked with both butt/miter and round cap/join.
  All use non-zero fill and source-over color `(40, 120, 220, 192)`.
- The shared alternating cubic arches stay within y=112..144 so width-6 stroke
  expansion remains in the common non-crossing domain. The inflected
  y=24..232 case
  is retained conceptually as a fixed-backend reliability case: its expanded
  outline currently returns `CrossingEdges` and is not a valid timing input.
- Setup excluded: image allocation, path construction, context construction,
  and ugl-rs caller-owned scratch allocation.
- Timed frame: clear the complete destination and fill or stroke the retained
  path. Stroke flattening/expansion is included by both renderers.
- Sampling: 500 warm-up frames, then 9 samples of 5,000 frames; compare the
  median and retain min/max as a noise check.
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
