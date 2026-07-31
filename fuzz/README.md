# Fuzzing

The `fixed_tiles` target generates bounded fixed-point multi-contour scenes
from arbitrary bytes. For both fill rules it requires streaming coverage,
retained strips, and direct tiles to produce identical pixels or propagate the
same raster error. Every generated scene also exercises undersized line
preparation and requires the capacity error to leave caller-owned output
unchanged. Streaming fixed coverage is compared with the analytic f32 backend;
the strict per-pixel bound is applied to simple triangle inputs. Arbitrary
self-intersecting multi-contour scenes use exact stream/strip/tile equivalence,
because independently rounded fixed crossings do not have a geometry-independent
constant per-pixel difference from f32 event arithmetic.

Keep this independent workspace compiling after public fixed-module changes:

```text
cargo check --manifest-path fuzz/Cargo.toml
```

It uses a separate workspace so libFuzzer and its `std`-only dependencies do
not enter the renderer's normal dependency graph or `no_std` checks.

```text
cargo +nightly fuzz run fixed_tiles
```

For a short sanitizer smoke run:

```text
cargo +nightly fuzz run fixed_tiles -- -max_total_time=20
```
