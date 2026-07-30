# Fuzzing

The `fixed_tiles` target generates bounded fixed-point multi-contour scenes
from arbitrary bytes. For both fill rules it requires streaming coverage,
retained strips, and direct tiles to produce identical pixels or propagate the
same raster error.

It uses a separate workspace so libFuzzer and its `std`-only dependencies do
not enter the renderer's normal dependency graph or `no_std` checks.

```text
cargo +nightly fuzz run fixed_tiles
```

For a short sanitizer smoke run:

```text
cargo +nightly fuzz run fixed_tiles -- -max_total_time=20
```
