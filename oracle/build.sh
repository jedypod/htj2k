#!/bin/sh
# Builds the OpenJPH oracle tools (ojph_enc, ojph_dec) into oracle/build.
#
# OpenJPH's core sources are compiled directly from the read-only reference
# tree (nothing there is modified), generic C++ only:
#   * -DOJPH_DISABLE_SIMD and none of the *_sse*/*_avx*/*_ssse3/*_wasm/*_vsx
#     sources, so ojph_codeblock_fun.cpp always selects the generic
#     ojph_decode_codeblock32 / ojph_decode_codeblock64;
#   * the two generic block decoders are compiled with their entry points
#     renamed to *_real; oracle/block_dump.cpp supplies the original names as
#     dumping wrappers (active only when $OJPH_BLOCK_DUMP is set);
#   * likewise the two generic block encoders, with oracle/enc_block_dump.cpp
#     as the wrappers (active only when $OJPH_ENC_BLOCK_DUMP is set).
# Besides ojph_enc and ojph_dec it builds enc_blocks_synth, which calls the
# block encoders directly with synthetic code blocks.
# Asserts stay enabled (no -DNDEBUG).
#
# Environment: OJPH_SRC (default: ../../reference/OpenJPH),
#              CXX (default g++), CC (default gcc), JOBS (default nproc).
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
OJPH=${OJPH_SRC:-$HERE/../../reference/OpenJPH}
OJPH=$(cd "$OJPH" && pwd)
CORE=$OJPH/src/core
OUT=$HERE/build
OBJ=$OUT/obj
CXX=${CXX:-g++}
CC=${CC:-gcc}
JOBS=${JOBS:-$(nproc 2>/dev/null || echo 4)}

mkdir -p "$OBJ"

COMMON="-O2 -g -fexceptions -DOJPH_DISABLE_SIMD -DOJPH_ALIGNED_ALLOC_EXISTS -D_FILE_OFFSET_BITS=64 -I$CORE -I$CORE/openjph -I$CORE/shared"
CXXFLAGS="$COMMON -std=c++14"
CFLAGS="$COMMON -std=c11"

# Generic sources only.
SRCS=$(cd "$CORE" && ls codestream/*.cpp coding/*.cpp others/*.cpp others/*.c transform/*.cpp \
  | grep -v -E '_(sse|sse2|ssse3|avx|avx2|avx512|wasm|vsx)\.cpp$')

# Write a Makefile so incremental rebuilds and parallelism come for free.
MK=$OUT/Makefile
{
  echo "OBJS ="
  for s in $SRCS; do
    o=$(echo "$s" | tr '/' '_' | sed 's/\.[ch]p*$/.o/')
    echo "OBJS += obj/$o"
    case "$s" in
      coding/ojph_block_decoder32.cpp) extra="-Dojph_decode_codeblock32=ojph_decode_codeblock32_real" ;;
      coding/ojph_block_decoder64.cpp) extra="-Dojph_decode_codeblock64=ojph_decode_codeblock64_real" ;;
      coding/ojph_block_encoder.cpp) extra="-Dojph_encode_codeblock32=ojph_encode_codeblock32_real -Dojph_encode_codeblock64=ojph_encode_codeblock64_real" ;;
      *) extra="" ;;
    esac
    case "$s" in
      *.c) printf 'obj/%s: %s\n\t%s %s -c $< -o $@\n' "$o" "$CORE/$s" "$CC" "$CFLAGS" ;;
      *)   printf 'obj/%s: %s\n\t%s %s %s -c $< -o $@\n' "$o" "$CORE/$s" "$CXX" "$CXXFLAGS" "$extra" ;;
    esac
  done
  printf 'obj/block_dump.o: %s\n\t%s %s -c $< -o $@\n' "$HERE/block_dump.cpp" "$CXX" "$CXXFLAGS"
  printf 'obj/enc_block_dump.o: %s\n\t%s %s -c $< -o $@\n' "$HERE/enc_block_dump.cpp" "$CXX" "$CXXFLAGS"
  printf 'libojph_oracle.a: $(OBJS) obj/block_dump.o obj/enc_block_dump.o\n\trm -f $@ && ar rcs $@ $^\n'
  printf 'ojph_enc: %s libojph_oracle.a\n\t%s %s $< libojph_oracle.a -o $@ -lpthread\n' "$HERE/ojph_enc.cpp" "$CXX" "$CXXFLAGS"
  printf 'ojph_dec: %s libojph_oracle.a\n\t%s %s $< libojph_oracle.a -o $@ -lpthread\n' "$HERE/ojph_dec.cpp" "$CXX" "$CXXFLAGS"
  printf 'enc_blocks_synth: %s libojph_oracle.a\n\t%s %s $< libojph_oracle.a -o $@ -lpthread\n' "$HERE/enc_blocks_synth.cpp" "$CXX" "$CXXFLAGS"
  printf 'all: ojph_enc ojph_dec enc_blocks_synth\n'
  printf '.DEFAULT_GOAL := all\n'
} > "$MK"

make -s -C "$OUT" -j"$JOBS"
echo "built $OUT/ojph_enc $OUT/ojph_dec $OUT/enc_blocks_synth"
