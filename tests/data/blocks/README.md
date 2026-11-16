# HT code-block decoder vectors

Each `.ojbk` file is a concatenation of records, one per call that OpenJPH
0.33.0 made to its generic (non-SIMD) HT block decoder while decoding one of
the codestreams in `../matrix` or `../strips`. The decoders are

```c++
// src/core/coding/ojph_block_decoder32.cpp
bool ojph_decode_codeblock32(ui8* coded_data, ui32* decoded_data,
  ui32 missing_msbs, ui32 num_passes, ui32 lengths1, ui32 lengths2,
  ui32 width, ui32 height, ui32 stride, bool stripe_causal);
// src/core/coding/ojph_block_decoder64.cpp
bool ojph_decode_codeblock64(ui8* coded_data, ui64* decoded_data,
  ui32 missing_msbs, ui32 num_passes, ui32 lengths1, ui32 lengths2,
  ui32 width, ui32 height, ui32 stride, bool stripe_causal);
```

The records are written by `oracle/block_dump.cpp`. It wraps both functions:
the oracle build compiles OpenJPH with `-DOJPH_DISABLE_SIMD` and without
any SIMD sources, so these generic versions are the only decoders that can
be dispatched. The records are selected and written here by
`oracle/gen_blocks.py`.

## Record layout

All integers are little-endian and there is no padding or alignment between
fields or records. A record has a 40-byte header, then the coded bytes, then
the decoded samples.

| offset | size | field | meaning |
|---|---|---|---|
| 0 | 4 | magic | ASCII `OJBK` |
| 4 | 1 | `bits` | 32 = `ojph_decode_codeblock32` was called, 64 = `ojph_decode_codeblock64` |
| 5 | 1 | `stripe_causal` | the `stripe_causal` argument (0 or 1) |
| 6 | 1 | `result` | the function's return value (1 = true, 0 = false) |
| 7 | 1 | reserved | always 0 |
| 8 | 4 | `missing_msbs` | u32 argument `missing_msbs` |
| 12 | 4 | `num_passes` | u32 argument `num_passes` |
| 16 | 4 | `lengths1` | u32 argument `lengths1`: bytes in the cleanup segment |
| 20 | 4 | `lengths2` | u32 argument `lengths2`: bytes in the SigProp/MagRef segment |
| 24 | 4 | `width` | u32 argument `width` |
| 28 | 4 | `height` | u32 argument `height` |
| 32 | 4 | `stride` | u32 argument `stride`: row pitch of `decoded_data`, in samples |
| 36 | 4 | `coded_len` | number of coded bytes that follow; always `lengths1 + lengths2` |
| 40 | `coded_len` | coded | `coded_data[0 .. lengths1 + lengths2]`, copied before the call |
| 40 + `coded_len` | `width*height*bits/8` | samples | `decoded_data` after the call: `height` rows of `width` samples, row by row, each a u32 (`bits` = 32) or a u64 (`bits` = 64) |

The next record starts right after the samples, so the size of a record is
`40 + coded_len + width*height*(bits/8)`.

Notes on the fields:

* `coded_data` points into OpenJPH's buffer for the code block. The
  generic decoders read it only through readers bounded by `lengths1` and
  `lengths2`, so the copied bytes are everything they read. `block_dump`
  checks that the decoder did not modify these bytes. In OpenJPH's own
  buffer, 8 bytes come before them and at least 16 bytes come after.
* The samples are written exactly as OpenJPH writes them, so they are not
  image values. Each one is sign-magnitude: the sign is in the top bit (bit
  31 or 63). A significant sample with magnitude `mu` gets
  `(2*mu + 1) << (p - 1)`, which is `mu << p` plus a half-bin bit, where
  `p = 30 - missing_msbs` (32-bit) or `p = 62 - missing_msbs` (64-bit).
  An insignificant sample is 0.
* Stride padding is not stored. OpenJPH also allocates an even number of
  rows, because the cleanup pass writes two rows per quad row. For an odd
  `height` the decoder does write row `height` (the row after the last;
  seen in the replay check below), which is not stored either. A port
  that fills a buffer of `height` rows must not write that extra row.
* OpenJPH uses the 64-bit decoder when the component's largest `K_max` plus
  2 is more than 32, that is when `K_max > 30`. With the colour transform,
  the first three components share the largest of their three values.
* OpenJPH never calls the decoder for a code block that is empty or has
  zero length: `codeblock::decode` skips the call unless
  `pass_length[0] > 0`, `num_passes > 0` and the coded buffer is not null.
  So there are no records for empty blocks. Such a block decodes to all
  zeros without any call. For stream-level coverage of that case,
  `../matrix/pat_zero_s16` and `../matrix/nlt3_unsigned` make no decoder
  calls at all: every block in them is empty.

Replay check (run once by hand, not part of the scripts): each record's
`coded` bytes alone were copied into a fresh buffer (8 bytes of 0x00 or
0xFF before, 16 after, at pointer offsets 0 to 3) and passed to the
generic decoders with the record's scalar arguments. For all 1309 records
this reproduced the stored samples and `result`.

## What the vectors cover, and what they cannot

OpenJPH 0.33.0's encoder writes **only the cleanup pass**. In
`codestream/ojph_codeblock.cpp`, `codeblock::encode` always sets
`num_passes = 1`, and `ojph_encode_codeblock32` asserts
`num_passes == 1`. Every vector here comes from a codestream that
OpenJPH's encoder wrote (`ojph_enc`, for both directories). So in every
record `num_passes = 1`, `lengths2 = 0`, `stripe_causal = 0` and
`result = 1`.

There are **no SigProp/MagRef (multi-pass) records** and none were
invented. To test those paths you need a codestream from another encoder,
such as Kakadu or OpenJPEG 2.5 or later with HT and refinement passes. The
encoder also has no option for the vertically causal mode, so no record has
`stripe_causal = 1`. No record has `result = 0`, because no failing call
happened.

| file | decoder | records | `missing_msbs` | sources |
|---|---|---|---|---|
| `dec32_small.ojbk` | 32 | 600 | 1 to 17 | 1x1 to 33x31 blocks: 4x4, 8x16 and 4x1024 nominal sizes, tiny images, 1-bit depth, const and zero images, precincts, tiles, subsampling, COC, and the two levels-0 streams described in `oracle/README.md` |
| `dec32_depths.ojbk` | 32 | 292 | 8 to 29 | depths 8 to 28 signed and unsigned, RCT, NLT, irreversible 9/7 streams |
| `dec32_large.ojbk` | 32 | 143 | 8 to 18 | full 64x64 and 128x32 blocks, 150x4 strips from 1024x4 blocks, the 64x64, 128x32 and 32x128 matrix cases, the 16-bit strips (NLT type 3 and the RCT) |
| `dec64.ojbk` | 64 | 283 | 29 to 34 | depths 29 to 32, RCT at 28, 31 and 32 bits, NLT at 32 bits, the 32-bit strips (NLT type 3 and the RCT) including full 128x32 blocks |

Exact duplicate records are dropped. If a group has more records than its
caps (a record count and a byte count, set in `gen_blocks.py`), the records
are chosen in four steps, so the result is the same on every run:

1. every record of the must-include streams;
2. the first record of each distinct (`bits`, `width`, `height`,
   `missing_msbs`);
3. the first record of each source stream;
4. round-robin across the source streams.

`INDEX.txt` has one line per record. It gives the file, the record number,
the byte offset, the record size, the source stream and the call number
within it, every scalar field, and the FNV-1a-64 hash of the sample bytes.
The hash uses offset basis `0xcbf29ce484222325` and prime `0x100000001b3`.

Regenerate with `python3 oracle/gen_blocks.py`, after `gen_matrix.py` and
`gen_strips.py`, or run `oracle/regen.sh` to do all of it.
