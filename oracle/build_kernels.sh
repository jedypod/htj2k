#!/bin/sh
# Builds oracle/build/kernels_dump, the golden-vector generator for the
# src/transform kernels, and (with "run") regenerates tests/data/kernels.
#
# The OpenJPH sources it needs (transform/ojph_transform.cpp,
# transform/ojph_colour.cpp, codestream/ojph_codestream_gen.cpp and
# codestream/ojph_params.cpp with its dependencies) are compiled here
# directly from the read-only reference tree, generic C++ only
# (-DOJPH_DISABLE_SIMD, no *_sse*/*_avx*/... sources), asserts enabled,
# with the same flags as oracle/build.sh.
#
# Usage: oracle/build_kernels.sh [run]
# Environment: OJPH_SRC (default: ../../reference/OpenJPH
#              relative to this directory, as in build.sh), CXX (default g++), CC (default gcc).
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
OJPH=$(cd "${OJPH_SRC:-$HERE/../../reference/OpenJPH}" && pwd)
CORE=$OJPH/src/core
OUT=$HERE/build
OBJ=$OUT/kobj
CXX=${CXX:-g++}
CC=${CC:-gcc}

mkdir -p "$OBJ"
FLAGS="-O2 -g -std=c++14 -fexceptions -DOJPH_DISABLE_SIMD -DOJPH_ALIGNED_ALLOC_EXISTS -D_FILE_OFFSET_BITS=64 -I$CORE -I$CORE/openjph -I$CORE/shared -I$CORE/codestream -I$CORE/transform"

SRCS=$(cd "$CORE" && ls codestream/*.cpp coding/*.cpp others/*.cpp others/*.c transform/*.cpp \
  | grep -v -E '_(sse|sse2|ssse3|avx|avx2|avx512|wasm|vsx)\.cpp$')
OBJS=""
for s in $SRCS; do
  o=$OBJ/$(echo "$s" | tr '/' '_' | sed 's/\.cp*$/.o/')
  if [ ! -f "$o" ] || [ "$CORE/$s" -nt "$o" ]; then
    case "$s" in
      *.c) $CC $(echo "$FLAGS" | sed 's/-std=c++14/-std=c11/') -c "$CORE/$s" -o "$o" & ;;
      *)   $CXX $FLAGS -c "$CORE/$s" -o "$o" & ;;
    esac
  fi
  OBJS="$OBJS $o"
done
wait
$CXX $FLAGS "$HERE/kernels_dump.cpp" $OBJS -o "$OUT/kernels_dump" -lpthread
echo "built $OUT/kernels_dump"

if [ "${1:-}" = run ]; then
  mkdir -p "$ROOT/tests/data/kernels"
  "$OUT/kernels_dump" "$ROOT/tests/data/kernels"
  ls -l "$ROOT/tests/data/kernels"
fi
