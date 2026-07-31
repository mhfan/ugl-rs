#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: benches/blend2d/run.sh /absolute/path/to/blend2d" >&2
  exit 2
fi

blend2d_dir=$1
build_dir=${TMPDIR:-/tmp}/ugl-rs-blend2d-build
output_dir=${TMPDIR:-/tmp}/ugl-rs-blend2d-output
mkdir -p "$build_dir" "$output_dir"

cmake -S benches/blend2d -B "$build_dir" \
  -DBLEND2D_DIR="$blend2d_dir" -DCMAKE_BUILD_TYPE=Release
cmake --build "$build_dir" --target blend2d_bench --config Release
cargo build --release --example compare_blend2d

"$build_dir/blend2d_bench" --output "$output_dir/blend2d.rgba"
target/release/examples/compare_blend2d \
  --output "$output_dir/ugl-rs.rgba" --compare "$output_dir/blend2d.rgba"
