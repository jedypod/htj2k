# OpenJPH test oracle

These C++ tools and scripts produce the reference data in `tests/data/`.
They are built against OpenJPH, the C++ HTJ2K codec that this crate ports.
They are not part of the crate build.

## One command

```sh
oracle/regen.sh
```

This builds the tools into `oracle/build/`, which git ignores, and then
regenerates `tests/data/matrix`, `tests/data/strips`, `tests/data/blocks`,
`tests/data/encoder`, `tests/data/profile` and `tests/data/ours`. It
needs g++ (C++14), gcc, make, python3 and cargo, and takes a few
minutes. The output is deterministic: two runs produce byte-identical
files. `tests/data/kernels` and `tests/data/params` are regenerated
separately, by `oracle/build_kernels.sh run` and with
`tests/data/params/params_dump.cpp`.

You can also run the steps one at a time: `oracle/build.sh`, then
`python3 oracle/gen_matrix.py`, `python3 oracle/gen_strips.py`,
`python3 oracle/gen_blocks.py`, `python3 oracle/gen_enc_blocks.py`,
`python3 oracle/gen_profile.py` and `python3 oracle/gen_ours.py`.
`gen_blocks.py` reads the output of `gen_matrix.py` and
`gen_strips.py`; `gen_enc_blocks.py` and `gen_profile.py` need only the
build; `gen_ours.py` reads the matrix manifest and builds the crate's
`encode_args` example with cargo.

`gen_matrix.py` exits 1 whenever a reversible case fails to round-trip. In
0.33.0 that always happens, because of the encoder issues listed under
[Known issues](#known-issues). `regen.sh` reports that exit status but does
not stop on it.

## OpenJPH version

The tools use OpenJPH at commit `97a9579` ("Fix lines left unconverted for
NLT type 0 and unsigned type 3 (#421)"), which is release **0.33.0** plus
five commits. Only that one changes the library since the release. The
source is expected as a git checkout at `../../reference/OpenJPH`, relative
to this directory. To build against a different copy, set `OJPH_SRC`.

`build.sh` compiles the core sources directly into `libojph_oracle.a`,
without CMake. It never modifies the reference tree.

* It uses `-O2 -g -DOJPH_DISABLE_SIMD` and leaves out every
  `*_sse*`/`*_avx*`/`*_ssse3`/`*_wasm`/`*_vsx` source. So
  `ojph_codeblock_fun.cpp` always dispatches the generic C++ code paths,
  including `ojph_decode_codeblock32` and `ojph_decode_codeblock64`.
* Asserts stay enabled (no `-DNDEBUG`).
* The block decoders are instrumented without a patch.
  `coding/ojph_block_decoder32.cpp` and `64.cpp` are compiled with
  `-Dojph_decode_codeblockNN=ojph_decode_codeblockNN_real`, and
  `block_dump.cpp` supplies the original names as wrappers. Each wrapper
  appends a record to `$OJPH_BLOCK_DUMP` when that variable is set, and
  otherwise does nothing.
* The block encoders are instrumented the same way.
  `coding/ojph_block_encoder.cpp` is compiled with
  `-Dojph_encode_codeblockNN=ojph_encode_codeblockNN_real` (both widths),
  and `enc_block_dump.cpp` supplies the original names as wrappers, which
  append to `$OJPH_ENC_BLOCK_DUMP` when it is set. A separate variable
  keeps encoder and decoder records apart.
* The Makefile does not track compiler flags. If an object was built
  before its flags changed (for example a build from before the encoder
  was instrumented), delete `oracle/build/obj/` first.

## Tools

* **`ojph_enc -o out.j2c [options]`** synthesises a deterministic image and
  writes a raw codestream (`.j2c`, no JP2/JPH boxes). Run `ojph_enc --help`
  for the options. They cover the size, components, depth (1 to 32) and
  signedness (also per component, `--comp-fmt 16:1,32:1,32:0`),
  `--irrev` (9/7), `--qstep`, `--qstep-comp C:F` (a per-component step,
  QCC), `--qfactor Q`, `--rct`, `--levels`, `--block`, `--precincts`,
  `--coc` (per-component levels and block size), `--rev-comps` and
  `--irrev-comps` (per-component reversibility), `--prog`, `--tile`,
  `--tile-off`, `--img-off`, `--tlm`, `--tileparts R|C|RC`, `--nlt3` and
  `--nlt3-comps`, `--nlt4 C:N`, `--comment TEXT` (an extra COM segment,
  after the version comment), `--sub`, `--planar`, `--seed` and
  `--pattern smooth|noise|const|zero`. `--raw-out FILE` also writes the
  input samples in the `.raw` layout below.
  * The calls follow the options: `set_reversible(c, ...)` for
    `--rev-comps`/`--irrev-comps` after the COC options;
    `set_qfactor(Q)` if `--qfactor` is given, otherwise
    `set_irrev_quant(qstep)` if any component is irreversible; then
    `set_irrev_quant(c, F)` for each `--qstep-comp`.
  * `--nlt4 C:N` calls the LUT form of `set_nonlinear_transform` for
    component C with NLT type 4 (binary complement followed by a LUT):
    the decoded depth and signedness and `pt_val` are the component's
    own, `d_min` is 0, `d_max` 0xFFFFFFFF, and the encoder uses the
    decoder's LUT (`approx_encoding_nlt` false). The LUT has N points
    rising from 0 to M = 2^B - 1 for a component of depth B:
    `lut[i] = floor(M * i * (i + N - 1) / (2 * (N - 1)^2))`. OpenJPH
    requires it to be strictly increasing, so N can be at most about
    M/2 + 1, and the component to be irreversible.
  * The random numbers come from splitmix64 seeded with `--seed`. For each
    component, the tool draws the constant value, then a gradient phase,
    then one value per sample in raster order.
  * `noise` is uniform over the full legal range. `smooth` is a diagonal
    gradient over the full range plus noise of about ±1/64 of the range,
    clamped. `const` is one random legal value per component. `zero` is all
    zeros.
* **`ojph_dec in.j2c out.raw [--info]`** decodes with OpenJPH and writes
  exactly what `codestream::pull` returns. `--info` prints one line per
  component and a COD summary.
  * It decodes interleaved when the colour transform is used and every
    component has the same height, and planar otherwise. OpenJPH's
    decoder accepts planar mode with the colour transform, but its
    planar `pull` then returns components 1 and 2 without the inverse
    transform.
  * It handles both `i32` and `i64` line buffers.
  * For irreversible streams, OpenJPH 0.33.0's `pull` also returns
    integer lines, because OpenJPH rounds and clamps internally. `ojph_dec`
    dumps those lines, and it would stop with an error if a float line came
    back.

* **`enc_blocks_synth`** calls the two block encoders directly, through the
  dumping wrappers, with synthetic code blocks: all-zero blocks, every
  supported `missing_msbs`, edge sizes and blocks large enough to fill the
  encoder's buffers. It installs an error handler that passes OpenJPH's
  error code to the wrapper. Run it with `$OJPH_ENC_BLOCK_DUMP` set;
  `gen_enc_blocks.py` does.

## File formats

### `.raw`: decoded samples (from `ojph_dec`, and `ojph_enc --raw-out`)

The components follow each other in order. Each component is all its lines,
top to bottom, at the reconstructed size from SIZ:
`ceil((Xsiz)/XRsiz) - ceil(XOsiz/XRsiz)` by the same for y, so subsampled
components are smaller. There is no header. Each sample is 4 bytes
little-endian, holding the low 32 bits of the value: two's complement for a
signed component, a plain u32 for an unsigned one. For an unsigned 32-bit
component, `ojph_enc` passes values above `i32::MAX` to OpenJPH as their bit
pattern, the way OpenJPH's 32-bit line buffers take them.

### `tests/data/matrix/MANIFEST.txt`

Lines starting with `#` are comments. Every other line has these
`|`-separated fields:

```
name | status | j2c bytes | raw bytes | raw_fnv1a64 | command [| error]
```

The `status` values are:

* `lossless`: the decoded `.raw` equals the encoder input exactly.
* `lossy maxerr=N`: an `--irrev` case. N is max |decoded − input|, read as
  signed or unsigned per `--signed`.
* `MISMATCH ...`: a reversible case where decoded ≠ input. These are kept
  and listed under [Known issues](#known-issues).
* `REFUSED`: the case did not encode. The error text from OpenJPH or
  `ojph_enc` follows, and no files are kept.
* `DECODE-FAILED`: `ojph_dec` failed.

`command` is the exact `ojph_enc` command line, with paths relative to
`tests/data/matrix`. The script also passes `--raw-out`. `raw_fnv1a64` is
FNV-1a-64 (offset basis `0xcbf29ce484222325`, prime `0x100000001b3`) of the
`.raw` file, written as 16 hex digits. Each `<name>.j2c` has its
`<name>.raw` beside it.

### `tests/data/strips/MANIFEST.txt`

`gen_strips.py` encodes 13 images with `ojph_enc` and decodes them with
`ojph_dec`, as decoder vectors at sizes the matrix avoids. All use one
tile, RPCL, 128x32 code blocks, 5 decompositions, TLM and tile parts at
resolutions:

* 16-bit signed samples with NLT type 3 and the RCT on four
  components, 300 samples wide and 200, 32 (smooth and noise) and 8
  lines high, and the same with 32-bit signed samples at 64, 32 and 8
  lines;
* one 16-bit signed component with NLT type 3, 64 wide and 32 or 8
  high, smooth and constant;
* two irreversible ones: the ICT and NLT type 4 on three 16-bit
  components with a Qfactor and a reversible fourth with NLT type 3,
  at 300x32, and three 32-bit components with an explicit step at
  300x8.

The manifest has the matrix layout. Every `.j2c` is committed, but only
the decodes named in `COMMIT_RAW` (the 300x8 and 64-wide ones); the
others, about 2 MB, are checked by hash, and all of them are left in
`oracle/build/strips/`. `tests/decode.rs` decodes every strip, and
`tests/encode_matrix.rs` encodes every one and requires OpenJPH's bytes.

### `tests/data/profile/MANIFEST.txt`

`gen_profile.py` encodes 353 images with `ojph_enc` and decodes them
with `ojph_dec`:

* `wide_*`: one tile, RPCL, 128x32 code blocks, 5 decompositions, TLM
  and tile parts at resolutions, at sizes from 8192x256 down to 8192x1
  and 1x256, with 1, 3 or 4 components of 16-bit signed and 32-bit
  signed samples (both with NLT type 3) and 32-bit unsigned samples;
  reversible, with the RCT on three or four components, and planar with
  subsampled chroma (`--sub 1x1,2x2,2x2`). A few short or small images
  use fewer levels.
* `ict_*`: the same layout, irreversible with the ICT and NLT type 4
  (`--nlt4`, 513 points for 16-bit and 257 for 32-bit components) on
  the first three components, with a Qfactor or explicit
  per-component steps; at the sizes above, at sizes of 1 mod 32 (from
  8193x257 to 1x257, as replication padding to 2^5 n + 1 gives) with a
  COM segment recording the padding, with reversible extra components
  (32-bit signed with NLT type 3, 32-bit unsigned, 16-bit signed), and
  planar with a subsampled reversible fourth component (so no ICT).
* `sweep_*`: one 64x32 image of three components, irreversible with
  the ICT and NLT type 4, at every Qfactor from 1 to 100 and at 54
  explicit steps (base 0.006 * 0.85^k, components 1 and 2 at 0.96 and
  1.1 times the base), for 16-bit and 32-bit signed samples.

The codestreams total about 60 MB, so only sizes and hashes are
committed:

```
name | status | j2c bytes | j2c_fnv1a64 | header bytes | header_fnv1a64 | raw bytes | raw_fnv1a64 | command
```

`status` is `lossless` (the decode equals the encoder input) or `lossy`
(irreversible). The header is the main header, up to the first SOT, so
a test can tell a parameter difference (NLT, QCD, QCC, COC, COM) from
one in the coded data. The raw is `ojph_dec`'s decode.
`tests/encode_profile.rs` encodes every case and requires the same
header and codestream, then decodes it with the crate's decoder and
requires `ojph_dec`'s samples. The codestreams, decodes and encoder
inputs are left in `oracle/build/profile/`, where the test looks for
the codestream to report the first byte that differs.

### `tests/data/ours/MANIFEST.txt`

`gen_ours.py` checks codestreams that the crate's encoder writes
differently from OpenJPH, or that OpenJPH cannot write, against
OpenJPH's decoder. Each case is an `ojph_enc` command line; the crate
encodes it through `examples/encode_args.rs` (the same image and calls
as the encoder tests), and `ojph_dec` decodes the result, which must
equal the input:

```
name | status | j2c bytes | j2c_fnv1a64 | command
```

`tests/encode_ours.rs` re-encodes each case and requires the same size
and hash, so the recorded verdict applies to what the encoder writes
now, and decodes it with the crate's decoder too. After a deliberate
change to such a codestream, rerun `gen_ours.py`. The codestreams are
left in `oracle/build/ours/`.

The cases are the reversible ones with no decompositions (`levels0_*`;
see [Known issues](#known-issues)): every such case of the matrix, then
40 random configurations.

### `tests/data/blocks/*.ojbk`

These hold code-block decoder records. The byte layout and the coverage are
in `tests/data/blocks/README.md`, and `INDEX.txt` describes every record.

### `tests/data/encoder/*.ojbe`

These hold code-block encoder records: the input samples and the coded
bytes, or the error raised. The byte layout and the coverage are in
`tests/data/encoder/README.md`, and `INDEX.txt` describes every record.

## Known issues

These are found by the matrix and kept in it on purpose.

* **Levels-0 minimum sample (OpenJPH encoder).** Affects
  `levels0_noise_33x31` (1 sample), `blk4x1024_37x61` (2 samples) and
  component 2 of `coc_lv0_rpcl` (1 sample).
  * With 0 decompositions and the reversible path, a sample at the bottom
    of the range comes back as 0 instead of −2^(B−1). This is the input
    value 0 for unsigned data, after the DC shift.
  * OpenJPH writes `epsilon = B − 1` in QCD for the only band. For example,
    `levels0_noise_33x31` (B = 10) has exponent 9 with 1 guard bit, which
    leaves 9 magnitude bits, so the value 2^(B−1) cannot be represented.
  * The block dump shows the coded sample is 0. OpenJPEG 2.5.4 (through
    Pillow) decodes these codestreams to the same values as `ojph_dec`.
  * So the stored `.raw` is the correct decode of the codestream, and a
    decoder port should match it.
  * For 32-bit unsigned samples the band also lacks the top bit of
    about half of the samples, not only the minimum (none of the
    matrix cases has this; `ojph_enc --levels 0 --depth 32 --pattern
    noise` shows it).
  * The crate's encoder writes one more bit-plane for this band
    (`ParamQcd::set_rev_quant`), so for every reversible case with a
    component at 0 decompositions its codestream differs from OpenJPH's
    in QCD and in the coded blocks. These are checked by
    `tests/data/ours` instead of byte for byte.
* **`nlt3_unsigned`** is lossless since OpenJPH #421. In release 0.33.0,
  NLT type 3 on an unsigned reversible component left the lines
  unconverted on both the encoding and the decoding side, so this case
  encoded to all-empty code blocks and decoded to zeros.
* **Refused cases** (no files are kept; the error text is in the manifest):
  * `levels33_4x4`: OpenJPH allows at most 32 decompositions.
  * `blk2x2_illegal` and `blk128x64_illegal`: incorrect code-block
    dimensions.
  * `rct1_planar_illegal`: the planar interface cannot be used with the
    colour transform.
  * `rct1_2comps`: the colour transform needs 3 or more components.
  * `mixfmt_rct_illegal`: the colour transform needs the first 3
    components to have the same depth (0x00040014).
  * `tile_off_illegal`: the tile offset must be smaller than the image
    offset.
  * `qf_low_illegal` and `qf_high_illegal`: the Qfactor must be from 1
    to 100 (0x00050181). OpenJPH prints the rejected value, a float,
    with `%i`, so `gen_matrix.py` replaces that number in the error
    text to keep the manifest reproducible.
  * `nlt4_rev_illegal`: NLT type 4 needs an irreversible component
    (0x000501B1).
  * `nlt4_flat_illegal`: an NLT LUT must be strictly increasing
    (0x000501A6); 256 points cannot rise strictly from 0 to 255 with
    the `--nlt4` formula.
  * `irrev_comps_lrcp_illegal`: `set_reversible(c, ...)` gives component
    c a COC with the default 5 decompositions, more than the main COD's
    3, and OpenJPH's encoder asserts on that under LRCP, as for
    `coc_levels_blocks` below. The same parameters work with RPCL
    (`irrev_comps_rpcl`).
  * `sub_2x2_planar0`: the error comes from `ojph_enc`, not OpenJPH.
    OpenJPH's interleaved (non-planar) `exchange` steps through comp 0's
    height for every component, so it cannot take subsampled components.
    Use planar mode for them.
  * `zh_middle`: the error comes from `ojph_enc`. Component 1 has no
    lines but component 2 has; OpenJPH's planar `exchange` asks for a
    line of component 1, which it could not place (it would loop
    forever). The `zh_*` and `zw_last` cases with empty components last
    encode and decode normally.
  * `coc_levels_blocks` (LRCP) and `coc_levels_rlcp`: components with
    different decomposition counts fail the OpenJPH assertion
    `res_num <= num_decomps` in `tile_comp::write_precincts` under the
    layer-first progressions. The same parameters work with RPCL, PCRL
    and CPRL (`coc_levels_*`).
* **Tile-part count with per-component decompositions (OpenJPH
  encoder).** This case is not in the matrix; the encoder's robustness
  test found it.
  * The trigger is LRCP or RLCP, with tile parts at components (and so
    at resolutions too), and components with different numbers of
    decompositions. For example: `--size 33x27 --comps 3 --levels 3
    --coc 1:1:4x8 --prog LRCP --tileparts C`.
  * TNsot counts a tile part for every resolution of the component with
    the most, but none is written for resolutions that other components
    lack, and TPsot skips their numbers (here TNsot is 12, with 10 tile
    parts numbered 0 to 11 without 7 and 10).
  * `ojph_dec` rejects the result with 0x00030091, "wrong tile part
    index", and so does the crate's decoder. With more components the
    two counts can also wrap at 8 bits. The crate's encoder refuses
    this configuration with 0x0008000C rather than write it.
* **Planar decoding with the colour transform (OpenJPH decoder).**
  OpenJPH's decoder accepts `set_planar(true)` for a codestream that
  uses the colour transform, and its planar `pull` then returns
  components 1 and 2 without the inverse transform. `ojph_dec` used to
  choose planar whenever a component was subsampled, which decoded
  RCT images with a subsampled fourth component wrongly; it now decodes
  interleaved for the colour transform. The crate's decoder behaves as
  OpenJPH's here.
* **No multi-pass code blocks.** The OpenJPH encoder writes only the
  cleanup pass. See `tests/data/blocks/README.md`.
