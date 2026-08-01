#!/bin/sh
set -eu

if [ "$#" -lt 1 ] || [ "$#" -gt 4 ]; then
  echo "usage: benches/blend2d/run.sh /absolute/path/to/blend2d [warmup [iterations [samples]]]" >&2
  exit 2
fi

blend2d_dir=$1
warmup=${2:-500}
iterations=${3:-5000}
samples=${4:-9}
build_dir=${TMPDIR:-/tmp}/ugl-rs-blend2d-build
output_dir=${TMPDIR:-/tmp}/ugl-rs-blend2d-output
mkdir -p "$build_dir" "$output_dir"
results="$output_dir/results.csv"
: > "$results"

cmake -S benches/blend2d -B "$build_dir" \
  -DBLEND2D_DIR="$blend2d_dir" -DCMAKE_BUILD_TYPE=Release
cmake --build "$build_dir" --target blend2d_bench --config Release
cargo build --release --example compare_blend2d

for scene in \
  fill_rectangles_1 \
  fill_rectangles_16 \
  fill_rectangles_64 \
  fill_rectangle_large \
  fill_rectangle_linear_gradient \
  fill_rectangle_radial_gradient \
  fill_rectangle_conic_gradient \
  fill_rectangle_path_mask \
  fill_rectangle_path_mask_sparse \
  build_path_mask \
  fill_triangles_64 \
  fill_cubics_8 \
  fill_cubics_8_clip_rect \
  stroke_cubics_8 \
  stroke_polyline_32 \
  stroke_polyline_round_32
do
  "$build_dir/blend2d_bench" --scene "$scene" \
    --warmup "$warmup" --iterations "$iterations" --samples "$samples" \
    --output "$output_dir/blend2d-$scene.rgba" | tee -a "$results"
  target/release/examples/compare_blend2d --scene "$scene" \
    --warmup "$warmup" --iterations "$iterations" --samples "$samples" \
    --output "$output_dir/ugl-rs-$scene.rgba" \
    --compare "$output_dir/blend2d-$scene.rgba" | tee -a "$results"
  target/release/examples/compare_blend2d --backend fixed --scene "$scene" \
    --warmup "$warmup" --iterations "$iterations" --samples "$samples" \
    --output "$output_dir/ugl-rs-fixed-$scene.rgba" \
    --compare "$output_dir/blend2d-$scene.rgba" \
    --compare-f32 "$output_dir/ugl-rs-$scene.rgba" | tee -a "$results"
done

echo "results: $results"
