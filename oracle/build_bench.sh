#!/bin/sh
# Builds OpenJPH twice with its own CMake build, as static Release
# libraries, and links oracle/bench_codec.cpp against each:
#
#   oracle/build/bench/generic/   -DOJPH_DISABLE_SIMD=ON (generic C++ only)
#   oracle/build/bench/simd/      CMake defaults (SSE..AVX-512 kernels,
#                                 picked at run time for the CPU)
#
# giving oracle/build/bench/bench_codec_generic and bench_codec_simd.
# The harness is compiled with the flags CMake gives the library in a
# Release build (CMAKE_CXX_FLAGS_RELEASE, normally -O3 -DNDEBUG), plus the
# library's own definitions, so both sides of the call are optimised
# alike.  CMake builds out of tree; the reference tree is not modified.
#
# Each build directory gets a FLAGS.txt recording the compiler and flags.
#
# Environment: OJPH_SRC (default: oracle/build/ref/OpenJPH if present,
#              else ../../reference/OpenJPH), CXX (default g++),
#              CC (default gcc), JOBS (default nproc).
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
if [ -z "${OJPH_SRC:-}" ]; then
  if [ -d "$HERE/build/ref/OpenJPH" ]; then OJPH_SRC=$HERE/build/ref/OpenJPH
  else OJPH_SRC=$HERE/../../reference/OpenJPH; fi
fi
OJPH=$(cd "$OJPH_SRC" && pwd)
OUT=$HERE/build/bench
CXX=${CXX:-g++}
CC=${CC:-gcc}
JOBS=${JOBS:-$(nproc 2>/dev/null || echo 4)}
mkdir -p "$OUT"

build_one() {  # name, extra cmake args
  name=$1; shift
  dir=$OUT/$name
  cmake -S "$OJPH" -B "$dir" -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_CXX_COMPILER="$CXX" -DCMAKE_C_COMPILER="$CC" \
    -DBUILD_SHARED_LIBS=OFF -DOJPH_BUILD_EXECUTABLES=OFF \
    -DOJPH_ENABLE_TIFF_SUPPORT=OFF -DOJPH_BUILD_TESTS=OFF "$@" >"$dir.cmake.log"
  cmake --build "$dir" -j"$JOBS" >"$dir.build.log"
  lib=$(find "$dir" -name 'libopenjph*.a' | head -n 1)
  [ -n "$lib" ] || { echo "build_bench.sh: no libopenjph*.a in $dir" >&2; exit 1; }
  rel=$(sed -n 's/^CMAKE_CXX_FLAGS_RELEASE:[A-Z]*=//p' "$dir/CMakeCache.txt")
  fm=$dir/src/core/CMakeFiles/openjph.dir/flags.make
  defs=$(sed -n 's/^CXX_DEFINES = //p' "$fm")
  flags="$rel -std=c++14 -fexceptions $defs"
  # shellcheck disable=SC2086
  "$CXX" $flags -I"$OJPH/src/core" -I"$OJPH/src/core/openjph" \
    "$HERE/bench_codec.cpp" "$lib" -o "$OUT/bench_codec_$name" -lpthread
  {
    echo "compiler: $("$CXX" --version | head -n 1)"
    echo "cmake: $(cmake --version | head -n 1)"
    echo "cmake args: -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF $*"
    echo "library flags: $(sed -n 's/^CXX_FLAGS = //p' "$fm")"
    echo "library defines: $defs"
    echo "harness flags: $flags"
    echo "library: oracle/build/bench/${lib#"$OUT"/}"
  } >"$dir/FLAGS.txt"
  echo "built $OUT/bench_codec_$name"
}

build_one generic -DOJPH_DISABLE_SIMD=ON
build_one simd
