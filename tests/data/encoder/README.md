# HT code-block encoder vectors

Each `.ojbe` file is a concatenation of records, one per call to OpenJPH
0.33.0's generic (non-SIMD) HT block encoder. The encoders are

```c++
// src/core/coding/ojph_block_encoder.cpp
void ojph_encode_codeblock32(ui32* buf, ui32 missing_msbs, ui32 num_passes,
  ui32 width, ui32 height, ui32 stride, ui32* lengths,
  ojph::mem_elastic_allocator *elastic, ojph::coded_lists *& coded);
void ojph_encode_codeblock64(ui64* buf, ui32 missing_msbs, ui32 num_passes,
  ui32 width, ui32 height, ui32 stride, ui32* lengths,
  ojph::mem_elastic_allocator *elastic, ojph::coded_lists *& coded);
```

The records are written by `oracle/enc_block_dump.cpp`. The oracle build
compiles `ojph_block_encoder.cpp` with both entry points renamed to `*_real`
(`-D`, no source change) and links the wrappers under the original names.
With `-DOJPH_DISABLE_SIMD` and no SIMD sources these generic versions are
the only encoders that can be dispatched. A wrapper copies the block's
samples, calls the real encoder, and appends a record to the file named by
`$OJPH_ENC_BLOCK_DUMP`. It also checks that the encoder did not modify its
input and that the `coded_lists` it got holds exactly `lengths[0]` bytes,
all used (`avail_size` 0); neither check has ever failed.

| file | encoder | records | `missing_msbs` | source |
|---|---|---|---|---|
| `enc32.ojbe` | 32 | 287 | 1 to 29 | `ojph_enc` runs (below) |
| `enc64.ojbe` | 64 | 168 | 30 to 34 | `ojph_enc` runs (below) |
| `synth.ojbe` | both | 563 | 0 to 61 | `oracle/enc_blocks_synth.cpp`, direct calls |

## Record layout (`OJBE`)

All integers are little-endian, with no padding between fields or records.
A record is a 40-byte header, then the input samples, then the coded bytes.

| offset | size | field | meaning |
|---|---|---|---|
| 0 | 4 | magic | ASCII `OJBE` |
| 4 | 1 | `bits` | 32 = `ojph_encode_codeblock32`, 64 = `ojph_encode_codeblock64` |
| 5 | 1 | `failed` | 1 if the call raised `OJPH_ERROR` (threw), else 0 |
| 6 | 2 | reserved | always 0 |
| 8 | 4 | `missing_msbs` | u32 argument |
| 12 | 4 | `num_passes` | u32 argument (always 1: the C++ asserts it) |
| 16 | 4 | `width` | u32 argument |
| 20 | 4 | `height` | u32 argument |
| 24 | 4 | `stride` | u32 argument: row pitch of `buf`, in samples |
| 28 | 4 | `length` | `lengths[0]` after the call (0 if `failed`) |
| 32 | 4 | `coded_len` | number of coded bytes that follow; `length`, or 0 if `failed` |
| 36 | 4 | `error_code` | the `OJPH_ERROR` code if `failed`, else 0 |
| 40 | `width*height*bits/8` | samples | `buf` before the call: `height` rows of `width` samples, each a u32 or u64 |
| 40 + samples | `coded_len` | coded | `coded->buf[0 .. lengths[0]]` after the call |

So a record is `40 + width*height*(bits/8) + coded_len` bytes long. The
stride padding of `buf` is not stored; the encoder reads only the
`height x width` samples. A port's test should still pass a buffer with the
recorded stride, with garbage in the padding.

Notes:

* The samples are what OpenJPH's encoder takes: sign-magnitude, the sign in
  the top bit and the magnitude below it, as written by `rev_tx_to_cb32/64`
  and `irv_tx_to_cb32/64` (`src/transform/codeblock_fun.rs`). With
  `p = 30 - missing_msbs` (32-bit) or `62 - missing_msbs` (64-bit), the
  encoder codes the magnitude bits from `p` up; the bits below `p` are
  ignored. In a codestream `missing_msbs = K_max - 1`, so `p` is the shift
  `rev_tx_to_cb` applied and the coded value is the quantised magnitude.
* `lengths[1]` is never written by the encoder, and the encoder writes only
  the cleanup pass (`num_passes` is 1). The caller,
  `codeblock::encode` in `ojph_codeblock.cpp`, sets `missing_msbs` and
  `num_passes`, and skips the call when every sample is zero, so the
  `ojph_enc` runs produce no records for all-zero blocks; `synth.ojbe` has
  them.
* `error_code` is known only in `synth.ojbe`: `enc_blocks_synth` installs
  an error handler that passes the code on. The codes are those of the
  encoder's fixed-size buffers: `0x00020001` (MEL, 192 bytes), `0x00020002`
  (VLC, 2880 bytes) and `0x00020005` (MagSgn, 17477 bytes for 32-bit and
  24030 for 64-bit). The other three codes, raised while terminating the
  segments, are not reached by any record.

## The `ojph_enc` runs (`enc32`, `enc64`)

`oracle/gen_enc_blocks.py` encodes 23 synthetic images with `ojph_enc` (see
`oracle/README.md`), 14 that need only the 32-bit encoder and 9 that need
only the 64-bit one (OpenJPH switches to 64 bits when `K_max > 30`). They
include 128x32 blocks with 5 levels, the RCT and NLT type 3 on 16- and
32-bit signed samples, bit depths 1 to 32, signed and unsigned, noise,
smooth and constant images, the 9/7 transform, tiles and subsampling,
and code blocks of 4x4, 8x8, 16x8, 8x16, 16x16, 32x32, 64x64, 32x128,
128x32, 1024x4 (full width) and 4x1024, plus every edge size they produce.
`INDEX.txt` names the run each record comes from.

Exact duplicate records are dropped. Each file has a record cap and a byte
cap, and records are chosen deterministically, diversity first: the first
record of each distinct (`width`, `height`, `missing_msbs`), then the first
record of each run, then round-robin across runs. `enc32` keeps 287 of 387
distinct records; `enc64` keeps all 168.

## The synthetic calls (`synth`)

`oracle/enc_blocks_synth.cpp` calls both encoders directly (through the
same wrappers) with blocks that encoding an image does not produce, or
rarely does:

* 19 small and edge sizes (1x1 to 64x3) with each of 9 patterns: all zero
  (some with the sign bit set, a signed zero), constant, noise in every bit
  (so also below bit `p`), noise of a limited number of bit planes, sparse,
  a ramp, every magnitude all ones, a single significant sample, and
  alternating zero and noise quad rows. `missing_msbs` is random in the
  supported range (32-bit: 0 to 29, 64-bit: 0 to 61);
* 11 large or full-width sizes (64x64 to 1024x4, 4x1024 and 1x1024),
  including widths 1021 to 1024;
* every `missing_msbs` of that range on an 8x8 block of all-ones and of
  noise samples. For the 64-bit encoder these reach `u_q > 32`, so the UVLC
  extension bits are coded;
* the calls that raise `OJPH_ERROR` (6 records), on full-size blocks at the
  top of the 64-bit range and on blocks of 1024 x 8 and 1024 x 16, larger
  than JPEG 2000 allows but accepted by the C++.

The stride is the width plus 0 to 8, and the padding is filled with
garbage, which the encoder never reads.

## Regenerating

`python3 oracle/gen_enc_blocks.py` after `oracle/build.sh`, or
`oracle/regen.sh` for everything. The output is the same on every run.
