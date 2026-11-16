#!/bin/sh
# Rebuilds synth.ojbh (in this directory). Compiles OpenJPH's generic block decoder sources
# (read-only reference tree) twice into $WORK: once with ASan+UBSan to list
# the cases whose C++ execution has undefined behaviour, and once with -O2
# -DNDEBUG (asserts off, as in a release build of OpenJPH: corrupt data
# trips its asserts) to write the vectors for the remaining cases.
# Seeds: the OJBK files of the parent directory (cleanup-only records).
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
# default: the reference tree next to the crate checkout
OJPH=${OJPH_SRC:-$HERE/../../../../../reference/OpenJPH}
CORE=$(cd "$OJPH/src/core" && pwd)
WORK=${WORK:-$(mktemp -d)}
CXX=${CXX:-g++}
SANCXX=${SANCXX:-clang++}  # needs the ASan/UBSan runtimes
# The 64-bit decoder is built from a copy with one fix: the SigProp value
# 3u << (p - 2) is a 32-bit shift in OpenJPH, wrong for
# missing_msbs <= 29; the crate shifts in 64 bits, and so must the vectors.
DEC64="$WORK/ojph_block_decoder64_fixed.cpp"
sed 's/ui64 val = 3u << (p - 2);/ui64 val = (ui64)3 << (p - 2);/' \
  "$CORE/coding/ojph_block_decoder64.cpp" > "$DEC64"
grep -q '(ui64)3 << (p - 2)' "$DEC64" || { echo "the SigProp fix did not apply" >&2; exit 1; }
SRCS="$CORE/coding/ojph_block_common.cpp $CORE/coding/ojph_block_decoder32.cpp $DEC64 $CORE/others/ojph_message.cpp $HERE/synth_blocks.cpp"
INC="-DOJPH_DISABLE_SIMD -I$CORE/coding -I$CORE -I$CORE/openjph -I$CORE/common -I$CORE/shared"
$SANCXX -std=c++14 -O1 -g -DNDEBUG -fsanitize=address,undefined $INC $SRCS -o "$WORK/synth_san"
$CXX -std=c++14 -O2 -DNDEBUG $INC $SRCS -o "$WORK/synth_rel"
SEEDS="$HERE/../dec32_small.ojbk $HERE/../dec32_depths.ojbk $HERE/../dec64.ojbk"
"$WORK/synth_san" list /dev/null $SEEDS 2> "$WORK/san.log"
N=$(grep -c '^CASE ' "$WORK/san.log")
# a case is bad if any sanitizer report follows its CASE line
BAD=$(awk '/^CASE /{c=$2; next} /runtime error|ERROR: AddressSanitizer/{if (c != "") bad[c]=1} END{s=""; for (k in bad) s = s (s==""?"":",") k; print s}' "$WORK/san.log")
echo "sanitizer run: $N cases; with undefined behaviour: ${BAD:-none}" >&2
"$WORK/synth_rel" "write:$BAD" "$HERE/synth.ojbh" $SEEDS
