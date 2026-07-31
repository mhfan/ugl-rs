# Blend2D comparison

This directory is the sole third-party comparison harness. It compares the f32
analytic RGBA8888 path with Blend2D's synchronous, single-threaded PRGB32 path.
Third-party source and build products are intentionally not vendored.

## Contract

- Scenes: `256x256`; 64 independent fractional-coordinate rectangles, an
  eight-cubic closed fill, and the same cubic path stroked at width 6 with
  butt caps and miter joins. All use non-zero fill and source-over color
  `(40, 120, 220, 192)`.
- Setup excluded: image allocation, path construction, context construction,
  and ugl-rs caller-owned scratch allocation.
- Timed frame: clear the complete destination and fill or stroke the retained
  path. Stroke flattening/expansion is included by both renderers.
- Sampling: 200 warm-up frames, then 9 samples of 2,000 frames; compare the
  median and retain min/max as a noise check.
- Output: each runner emits CSV, an FNV-1a checksum, and optionally normalized
  premultiplied RGBA bytes. The Rust runner reports exact-pixel rate, mean
  absolute channel error, and maximum channel error against Blend2D.

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
