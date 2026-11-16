# Golden vectors for the `src/transform` kernels

Produced by `oracle/kernels_dump.cpp` (build and regenerate with
`oracle/build_kernels.sh run`), which calls OpenJPH 0.33.0's generic
`ojph::local::gen_*` kernels, compiled with `-O2 -DOJPH_DISABLE_SIMD`
(asserts enabled) on x86-64, on deterministic pseudo-random inputs
(splitmix64). Read by the unit tests in `src/transform/tests.rs`.

## Container format

Each file is a sequence of records, all little endian:

    record := u32 field_count, field*
    field  := u8 tag, u32 element_count, element*

| tag | element |
|-----|---------|
| 1   | i32 |
| 2   | i64 |
| 3   | f32 (raw bit pattern; NaN payloads preserved) |
| 4   | u32 |
| 5   | u64 |
| 6   | u16 |

A *line* field is the whole backing vector of a line buffer with
`pre_size = 1`: element 0 is logical sample -1, element `1 + i` is logical
sample `i`, and the line's `size` is `element_count - 1` (one more than
the samples the kernel uses, for the extension sample written at the band
width). Unused elements hold a filler pattern, so kernels writing out of
place are caught. "before" fields are the lines before the call, "after"
fields the same lines after it; tests compare every element bit for bit.

`mode` 0 means moderate values; `mode` 1 mixes in extremes (type limits,
for floats also NaN, infinities, ±0.5, values near 2^31, denormals and
arbitrary bit patterns).

## Files

### `wavelet_horz.bin` (`gen_{rev,irv}_horz_{ana,syn}`)
1. u32 `[kind, width, even, ana, mode]`; kind 0 rev 32-bit, 1 rev 64-bit,
   2 irv float; `ana` 1 analysis, 0 synthesis.
2. kernel steps: i32 `[a, b, e]` per step (reversible) or f32
   `[K, a0, a1, ...]` (irreversible). Kernels are 5/3 and 9/7 from
   `param_atk::get_atk(1/0)`, plus two reversible kernels with the
   5/3 atk's steps overwritten: `(-1,3,2),(3,-5,3)` and `(1,0,0),(-2,1,1)`.
3. three lines before: analysis `ldst, hdst, src`; synthesis
   `dst, lsrc, hsrc`.
4. the same three lines after.

### `wavelet_vert.bin` (`gen_rev_vert_step`, `gen_irv_vert_step`, `gen_irv_vert_times_K`)
1. u32 `[kind, repeat, synthesis, alias, mode]`; kind 0 rev 32-bit,
   1 rev 64-bit, 2 irv step, 3 irv times K; `alias` 1 means `sig` was
   also passed as `other`.
2. step: i32 `[a, b, e]` (kinds 0, 1) or f32 `[a]` (kind 2) or f32 `[K]`
   (kind 3).
3. lines before: `sig, other, aug`.
4. `aug` after.

### `colour.bin` (`ojph_colour.cpp`)
Record 0: f32 `[ALPHA_RF, ALPHA_GF, ALPHA_BF, BETA_CbF, BETA_CrF,
GAMMA_CR2R, GAMMA_CB2B, GAMMA_CR2G, GAMMA_CB2G]` (`CT_CNST`).
Then records whose first field is u32 with `func` first:
- func 0 `rev_convert`, 1 `rev_convert_nlt_type3`: u32
  `[func, combo, src_off, dst_off, width, mode]` (combo 0: 32→32,
  1: 32→64, 2: 64→32), i64 `[shift]`, src line, dst line before, dst after.
- func 2 `irv_convert_to_integer`, 3 `..._nlt_type3`, 4
  `irv_convert_to_float`, 5 `..._nlt_type3`: u32
  `[func, bit_depth, is_signed, offset, width, mode]` (offset is the dst
  offset for 2/3, the src offset for 4/5), src line, dst before, dst after.
- func 6 `rct_forward`, 7 `rct_backward`: u32 `[func, wide, repeat, mode]`
  (wide: y/cb/cr are 64-bit), lines `r, g, b, y, cb, cr` before, then after.
- func 8 `ict_forward`, 9 `ict_backward`: u32 `[func, 0, repeat, mode]`,
  three f32 input arrays, three f32 output arrays.

### `nlt.bin` (`gen_irv_convert_to_integer_nlt`, `gen_irv_convert_to_float_nlt`)
Hand-filled `nlt_rec` objects with synthetic monotone tables.
1. u32 `[variant, Tnlt, bit_depth, is_signed, approx_encoding_nlt,
   precise_max_steps, num_points, width, offset, mode]`; variant 0
   to integer, 1 to float approximate, 2 to float precise one-step,
   3 to float precise search. The kernels are called with the unused
   `bit_depth`/`is_signed` arguments set to 7 and `!is_signed`.
2. f32 `[multiplier, fd_min, fd_max, delta, inv_delta, ft_min, ft_max]`.
3. f32 `dec_points`, f32 `approx_enc_points`, u16
   `precise_enc_dec_indices`, f32 `precise_dec_lut` (empty when unused).
4. src line, dst line before, dst line after.

### `codeblock.bin` (`ojph_codestream_gen.cpp`)
1. u32 `[func, K_max, count, mode]`; func 0 `rev_tx_to_cb32`,
   1 `rev_tx_to_cb64`, 2 `irv_tx_to_cb32`, 3 `rev_tx_from_cb32`,
   4 `rev_tx_from_cb64`, 5 `irv_tx_from_cb32`, 6 `irv_tx_from_cb64`.
2. f32 `[delta]` (`delta_inv` for the to-cb functions).
3. funcs 0–2: source array, `[max_val]` before, destination array after
   (prefilled with `0xDEADBEEF`), `[max_val]` after. Funcs 3–6: source
   array, destination array after.
