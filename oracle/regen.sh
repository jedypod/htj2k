#!/bin/sh
# Builds the oracle and regenerates everything under tests/data/:
#   tests/data/matrix  (gen_matrix.py)
#   tests/data/strips  (gen_strips.py)
#   tests/data/blocks  (gen_blocks.py; reads the two directories above)
#   tests/data/encoder (gen_enc_blocks.py)
#   tests/data/profile (gen_profile.py)
#   tests/data/ours    (gen_ours.py; builds examples/encode_args with cargo)
# Exits non-zero if the build fails, a strip fails to encode, decode
# or round-trip, or
# decoder or encoder block generation fails, or a profile case does
# not encode, decode, or round-trip.  gen_matrix.py exits 1
# whenever any reversible case does not round-trip; it currently always
# does, because of the known OpenJPH encoder issues listed in
# oracle/README.md, so its status is printed but not fatal here -- check
# its MISMATCH lines against that list.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
"$HERE/build.sh"
python3 "$HERE/gen_matrix.py" || echo "regen.sh: gen_matrix.py reported problems (see MANIFEST.txt)"
python3 "$HERE/gen_strips.py"
python3 "$HERE/gen_blocks.py"
python3 "$HERE/gen_enc_blocks.py"
python3 "$HERE/gen_profile.py"
python3 "$HERE/gen_ours.py"
