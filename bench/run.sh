#!/bin/sh
# Builds the four benchmark harnesses and reproduces the tables in
# bench/README.md:
#
#   oracle/build/bench/bench_codec_generic   OpenJPH, OJPH_DISABLE_SIMD=ON
#   oracle/build/bench/bench_codec_simd      OpenJPH, SIMD (run-time dispatch)
#   target/release/examples/bench_codec      htj2k, cargo build --release
#   target/native/release/examples/bench_codec
#                                            htj2k, -C target-cpu=native
#
# then runs bench/bench.py, which checks that the codestreams and decodes
# agree and measures cycles:u and instructions:u with perf stat on the
# least busy core. The results go to bench/results/.
#
# Usage: bench/run.sh [bench.py options], e.g.
#   bench/run.sh --profile          also record the top symbols
#   bench/run.sh --cpu 7            use CPU 7
#   bench/run.sh --only 3840x2160   a subset of the configurations
#   bench/run.sh --no-perf          only the correctness checks
#
# Needs g++, cmake, cargo, python3, perf and taskset, and OpenJPH's source
# (see oracle/build_bench.sh for OJPH_SRC).
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

sh oracle/build_bench.sh
cargo build --release --example bench_codec
RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=native" \
  cargo build --release --example bench_codec --target-dir target/native

mkdir -p bench/results
{
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "commit: $(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
  echo "cpu: $(sed -n 's/^model name[[:space:]]*: //p' /proc/cpuinfo | head -n 1)"
  echo "kernel: $(uname -sr)"
  echo "rustc: $(rustc -V)"
  echo "cargo profile: release (opt-level 3, no LTO, codegen-units 16)"
  echo "htj2k native: RUSTFLAGS=-C target-cpu=native"
  echo "perf: $(perf --version)"
  for b in generic simd; do
    echo "--- OpenJPH $b"
    cat "oracle/build/bench/$b/FLAGS.txt"
  done
} >bench/results/machine.txt
lscpu >bench/results/lscpu.txt

python3 bench/bench.py "$@"
