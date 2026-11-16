# Synthetic HT code-block vectors (`synth.ojbh`)

OpenJPH's encoder writes only the cleanup pass, so the codestream-derived
records in `../*.ojbk` (format: `../README.md`) never exercise the
SigProp/MagRef passes, stripe-causal mode or failing calls. `synth.ojbh`
fills that gap. `synth_blocks.cpp` calls OpenJPH 0.33.0's generic
`ojph_decode_codeblock32` / `ojph_decode_codeblock64` directly, with these
inputs:

* real cleanup passes (records of `dec32_small`, `dec32_depths` and
  `dec64` with at most 160 coded bytes, about 265 of them picked
  deterministically), each used for
  1. 2 and 3 passes, with 1 to 200 arbitrary refinement bytes appended
     (some biased towards `0xFF`/`0x7F` to hit unstuffing), and a random
     `stripe_causal`;
  2. 1 to 3 bit flips in the cleanup pass, with 1 to 3 passes;
  3. a truncated cleanup pass, so `scup` is read from the new end;
  4. another `missing_msbs` (32-bit: 0 to 31; 64-bit: 26 to 61), with the
     cleanup pass only;
  5. a smaller width and height with the same data, sometimes with
     refinement passes;
* 400 blocks of pure noise, up to 40x40, with a plausible `scup`, with 1
  to 3 passes;
* 600 64-bit blocks of noise rich in zero bytes and 0/1 runs, with
  `missing_msbs` 34 to 61, mostly 1 pass. These make quads with
  `u_q > 32` decode successfully, so they exercise the 64-bit decoder's
  `u_q` extension (`u_ext`) in the initial and the later quad rows.

These calls are not what a real multi-pass encoder would produce: the
refinement bytes are noise. They pin the C++ decoder's exact behaviour on
such data, including for corrupt input.

Each call runs at the four byte alignments of `coded_data`, with different
garbage bytes around the data. All four runs must give the same result and
buffer, otherwise the case is dropped. No case has been dropped.
The 64-bit decoder is built from a copy of OpenJPH's source with one fix,
which `build.sh` applies: the SigProp value `3u << (p - 2)`
(`ojph_block_decoder64.cpp:1549`) is a 32-bit shift in OpenJPH 0.33.0. It
loses a bit for `missing_msbs` = 29 and is undefined for 28 or less. The
copy shifts in 64 bits, as the crate does. The fix changes the output of
34 of the calls, all of them 64-bit calls with 2 or 3 passes and
`missing_msbs` of 28 or 29.

`build.sh` first runs every case under clang ASan+UBSan and leaves out each
case that reports undefined behaviour. The sanitizer reports a source line
only once per process, so the script runs it again without the cases found
so far, until a run is clean. Four cases are left out, all 64-bit calls
with refinement passes and `missing_msbs` = 61, where `p - 2` wraps around
in a shift count. Both builds use `-DNDEBUG`, because corrupt data trips
OpenJPH's `assert`s.

## Record layout (`OJBH`)

The 40-byte header is the same as an `OJBK` record's, except that the magic
is `OJBH`. The coded bytes follow it, as in `OJBK`. Then comes one 8-byte
little-endian value instead of the samples: the FNV-1a-64 hash of the whole
decoded buffer after the call. The hash uses offset basis
`0xcbf29ce484222325` and prime `0x100000001b3`, and the buffer's bytes are
the samples written out little-endian.

The decoded buffer has `height + 4` rows of `stride` samples. The stride is
`width + 8 + (0..3)`. The buffer is all zeros before the call. So the hash
also covers:

* writes the C++ makes outside the block: row `height` for an odd height,
  and MagRef writes to the right of or below the block on corrupt data;
* the partial output of calls that return false.

A record is `40 + coded_len + 8` bytes long.

Regenerate with `OJPH_SRC=/path/to/OpenJPH WORK=/tmp/dir ./build.sh`.
You need `g++`, plus `clang++` with the sanitizer runtimes. The generator
is deterministic, so the output is the same on every run.

Contents: 2588 records. With 1 pass: 697 for the 32-bit decoder and 648
for the 64-bit one. With 2 passes: 425 and 198. With 3 passes: 448 and
172. 639 records have `stripe_causal`, and 456 records have result false.
