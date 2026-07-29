# Raster benchmark baseline

Run the scalar comparison with:

```text
cargo bench --bench raster --all-features
```

The scene contains 64 fractional rectangles in a 256 × 256 premultiplied
RGBA8888 target. Path construction, fixed-line preparation, and all heap
allocation happen before Criterion starts each measured iteration. The
measured loop clears the destination and performs scan conversion plus
source-over compositing.

The initial caller-owned scratch budgets are:

| Backend | Edge/segment storage | Crossing storage | Row storage |
| --- | ---: | ---: | ---: |
| sampled `f32` | 128 `Edge` | 128 `Intersection` | 256 `f32` |
| analytic `f32` | 128 `Edge` | 128 `AnalyticIntersection` | 256 `f32` |
| Q24.8 fixed | 128 `FixedSegment` + 64 `FixedTrapezoid` | none | 256 `u64` |

Renderer allocation count inside the measured path is zero by API
construction: every mutable geometry, crossing, area, and destination buffer
is borrowed from the benchmark. Criterion's own allocations are outside that
contract. Record machine, compiler, commit, median time, and throughput when
publishing results; machine-specific numbers do not belong in this file.
