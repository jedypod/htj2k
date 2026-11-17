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
`tests/data/encoder`, `tests/data/profile` and `tests/data/ours`.
It needs g++ (C++14), gcc, make, python3 and cargo, and takes a few
minutes. The
output is deterministic: two runs produce byte-identical files.

You can also run the steps one at a time: `oracle/build.sh`, then
`python3 oracle/gen_matrix.py`, `python3 oracle/gen_strips.py`,
`python3 oracle/gen_blocks.py`, `python3 oracle/gen_enc_blocks.py`,
`python3 oracle/gen_profile.py` and `python3 oracle/gen_ours.py`.
`gen_blocks.py` reads the output of the first two; `gen_enc_blocks.py`
and `gen_profile.py` need only the build; `gen_ours.py` reads the
matrix manifest and builds the crate's `encode_args` example with cargo.

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
  signedness (also per component, `--comp-fmt 16:1,32:1,32:0`), `--irrev` (9/7), `--qstep`, `--rct`, `--levels`, `--block`,
  `--precincts`, `--coc` (per-component levels and block size), `--prog`,
  `--tile`, `--tile-off`, `--img-off`, `--tlm`, `--tileparts R|C|RC`,
  `--nlt3` and `--nlt3-comps`, `--sub`, `--planar`, `--seed` and
  `--pattern smooth|noise|const|zero`. `--raw-out FILE` also writes the
  input samples in the `.raw` layout below.
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

`gen_strips.py` encodes 11 images with `ojph_enc` and decodes them with
`ojph_dec`, as decoder vectors at sizes the matrix avoids. All use one
tile, RPCL, 128x32 code blocks, 5 decompositions, TLM and tile parts at
resolutions, and all are reversible:

* 16-bit signed samples with NLT type 3 and the RCT on four
  components, 300 samples wide and 200, 32 (smooth and noise) and 8
  lines high, and the same with 32-bit signed samples at 64, 32 and 8
  lines;
* one 16-bit signed component with NLT type 3, 64 wide and 32 or 8
  high, smooth and constant.

The manifest has the matrix layout. Every `.j2c` is committed, but only
the decodes named in `COMMIT_RAW` (the 300x8 and 64-wide ones); the
others, about 2 MB, are checked by hash, and all of them are left in
`oracle/build/strips/`.

### `tests/data/profile/MANIFEST.txt`

`gen_profile.py` encodes 24 images with `ojph_enc` and decodes them
with `ojph_dec`:

* `wide_*`: one tile, RPCL, 128x32 code blocks, 5 decompositions, TLM
  and tile parts at resolutions, at sizes from 8192x256 down to 8192x1
  and 1x256, with 1, 3 or 4 components of 16-bit signed and 32-bit
  signed samples (both with NLT type 3) and 32-bit unsigned samples;
  reversible, with the RCT on three or four components, and planar with
  subsampled chroma (`--sub 1x1,2x2,2x2`). A few short or small images
  use fewer levels.

The codestreams total about 54 MB, so only sizes and hashes are
committed:

```
name | status | j2c bytes | j2c_fnv1a64 | header bytes | header_fnv1a64 | raw bytes | raw_fnv1a64 | command
```

`status` is `lossless`: the decode equals the encoder input. The header
is the main header, up to the first SOT, so a test can tell a parameter
difference from one in the coded data. The raw is `ojph_dec`'s decode.
The codestreams, decodes and encoder inputs are left in
`oracle/build/profile/`, where the encoder test looks for the
codestream to report the first byte that differs.

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
  * `tile_off_illegal`: the tile offset must be smaller than the image
    offset.
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
* **No multi-pass code blocks.** The OpenJPH encoder writes only the
  cleanup pass. See `tests/data/blocks/README.md`.
