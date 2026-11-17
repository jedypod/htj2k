# htj2k

A High-Throughput JPEG 2000 (HTJ2K, ITU-T T.814 | ISO/IEC 15444-15)
codestream codec in safe Rust, ported from
[OpenJPH](https://github.com/aous72/OpenJPH) 0.33.0.

Both directions work, with OpenJPH's generic code paths; its SIMD
kernels are not ported.

* **Decoding.** `codestream::Codestream` reads a codestream held in
  memory and returns it line by line, following `ojph::codestream`
  (`read_headers`, `create`, `pull`).
* **Encoding.** `codestream::Encoder` follows `ojph::codestream` when
  it writes: set the parameters (`access_siz_mut`, `access_cod_mut`,
  `access_qcd_mut`, `access_nlt_mut`, `set_planar`,
  `set_tilepart_divisions`, `request_tlm_marker`, `set_memory_limit`),
  then `write_headers`, `exchange` for every line, and `flush`. The
  codestream is appended to a caller's `Vec<u8>`. Everything OpenJPH's
  encoder does is ported: every progression order, tiles and offsets,
  subsampling, precincts, per-component COC, tile parts at resolutions
  and/or components, TLM, reversible and irreversible coding, the
  colour transforms, NLT type 3, and planar and interleaved exchange.
  The HT block encoders are also public, as `coding::encode_codeblock32`
  and `encode_codeblock64`.

## How it is checked

Every module is tested against output of the C++ it is translated from.
`oracle/` builds OpenJPH's generic code and regenerates the vectors in
`tests/data`; see `oracle/README.md`. The decoder reproduces OpenJPH's
output sample for sample on all 209 codestreams there, which include
16- and 32-bit signed samples under NLT type 3 in strips of four
components with 128x32 blocks.

The encoder reproduces OpenJPH's codestreams byte for byte:
* all 198 that `ojph_enc` writes in the parameter matrix
  (`tests/data/matrix`), which include the Qfactor, per-component
  steps and reversibility, NLT type 4 LUTs and extra COM segments, except the 5 reversible ones with no
  decompositions, which differ on purpose (see below) and decode
  losslessly with OpenJPH's decoder, as do 40 random configurations of
  that kind (`tests/data/ours`);
* all 11 in `tests/data/strips`, 300 samples wide and 8 to 200 lines
  high, or 64 wide, with four components under NLT type 3 and the RCT,
  or one, and 128x32 blocks;
* 24 more at large sizes (`tests/data/profile`): short, wide images
  from 8192x256 down to 8192x1 and 1x256 with 128x32 blocks, RPCL, TLM
  and tile parts at resolutions, reversible with NLT type 3 and the RCT.

It also refuses each of the 16 configurations OpenJPH refuses, with
OpenJPH's error code where OpenJPH gives one. Its block encoder
reproduces the coded bytes of every recorded call in
`tests/data/encoder`. Random configurations, samples and call
sequences never make it panic, and what it encodes decodes to planes of
the sizes SIZ gives; with samples in range, every reversible component
decodes to exactly its input.

## Differences from OpenJPH

The port follows OpenJPH closely enough to be read against it, with these
intended differences in behaviour:

* No input makes it panic or read out of bounds. Where the C++ would, it
  returns an error.
* The block encoders return an error for arguments on which the C++
  asserts or has undefined behaviour: `num_passes` other than 1,
  `missing_msbs` above 29 (32-bit) or 61 (64-bit), a width above 1024, or
  a sample buffer too small.
* `Codestream::create` refuses planar decoding of a codestream that
  uses the colour transform (0x00070037). Planar `pull` cannot invert
  the transform, which needs a line of all three components at once;
  OpenJPH returns components 1 and 2 uninverted. Interleaved, the
  default after `read_headers` for such a codestream, decodes it.
* `Codestream::set_memory_limit` bounds the memory a codestream can make
  the decoder allocate. The default is 4 GiB.
* Code-block buffers are allocated at the size of the actual blocks, not
  the nominal size as in OpenJPH, so short images and chunks, small
  tiles and subband edges take much less memory. A chunk 32 lines high
  needs about 350 bytes per column of width per component with 64-bit
  samples and 5 decomposition levels, for any block size; OpenJPH's
  sizing would take 23 bytes per column times the nominal block height.
* The 64-bit block decoder computes the value of a sample found
  significant in the SigProp pass with a 64-bit shift. OpenJPH 0.33.0
  shifts in 32 bits, which is wrong for `missing_msbs` of 29 or less. Blocks
  with only a cleanup pass are not affected.
* NLT type 0, and type 3 on an unsigned reversible component, are treated
  as no nonlinearity, as OpenJPH does after 0.33.0.

The encoder differs as follows:

* It returns an error where OpenJPH asserts, loops forever or wraps.
  * LRCP or RLCP with components of different decomposition counts.
    Without tile parts at components OpenJPH asserts in
    `tile_comp::write_precincts` (0x00080007 here). With them it writes
    a tile-part count (TNsot) and tile-part indices (TPsot) that no
    decoder accepts, and that can wrap at 8 bits (0x0008000C here).
  * Interleaved exchange of components of different heights;
    OpenJPH's `exchange` cannot do it, and `ojph_enc` reports it.
  * A component with no lines (subsampled more than the image is high)
    followed by a component with lines (0x00080005). OpenJPH's planar
    `exchange` asks for a line of it and then loops forever, and its
    decoder loops forever on such a codestream. Components with no lines
    after the last one with lines, or no lines at all, are encoded as
    OpenJPH encodes them; `exchange` never asks for their lines.
  * Precincts 1 sample wide or high above resolution 0. This is left
    when precincts are set before the decomposition count is raised.
    OpenJPH writes it, and then cannot decode it.
  * A tile part longer than its 32-bit length field can hold, or a
    comment longer than a COM segment can hold.
  * Calls out of order, such as `exchange` before `write_headers` or
    `flush` before every line is exchanged.
  * Parameter state that the decoder uses but the writer does not
    implement (0x0008000D). The parameter objects are shared with the
    decoder, so `access_*_mut` reaches `read`, `set_skipped_resolutions`
    and similar calls. Refused are skipped resolutions, Rsiz bits other
    than HT, EXT and NLT, SOP or EPH markers, more than one quality
    layer, code-block style options, ATK or DFS kernels, a colour
    transform value other than 0 or 1, and a LUT nonlinearity without
    an encoding table (one read from a codestream). OpenJPH would
    ignore them, or write markers the packets do not match.
  * Any setter or `access_*_mut` call after `write_headers`
    (0x0008000B); the call changes nothing. In OpenJPH such calls
    change the stream being written, which can leave it without tile
    parts, with a wrong TLM, or with a wrong image.
  * A tile grid that OpenJPH's decoder cannot count (0x0008000E).
    OpenJPH computes the number of tiles as (extent - tile offset +
    tile size - 1) / tile size in 32 bits; where that wraps, its
    decoder finds no tiles. This includes the default tile (extent plus
    offset) when that sum does not fit in 32 bits, which OpenJPH's
    encoder wraps and refuses with a misleading error.
* Canvas coordinates of 2^31 and more work in every progression order.
  OpenJPH's encoder and decoder search for the next precinct starting
  from `INT_MAX`, so with RPCL, PCRL or CPRL they assert on such images;
  here both sides search without a sentinel, which gives the same order
  below 2^31.
* A failed call leaves the output `Vec` as it was, and the encoder
  cannot be used further.
* `exchange()` takes no arguments. Each call returns the next line to
  fill, as `Option<(component, &mut [i32])>`, and takes back the
  previous one, so the encoder keeps its line buffers and the caller
  cannot pass back a line of the wrong size. OpenJPH passes the filled
  line back as an argument.
* If `set_planar` is never called, the exchange is interleaved without
  the colour transform, as in OpenJPH. With the colour transform OpenJPH
  then picks planar, which it cannot encode correctly; `write_headers`
  refuses that with 0x00030021, the error OpenJPH gives for an explicit
  `set_planar(true)`. Call `set_planar(false)` to use the colour
  transform.
* `set_memory_limit` bounds the memory for the tiles (their structures,
  line buffers and code-block buffers), the lines exchanged with the
  caller, and the coded bytes held until `flush`. The default is 4 GiB.
  An image whose tiles need more is refused by `write_headers`, and
  coded data beyond it by `exchange`. As in the decoder, the limit is
  checked before each allocation. It is not a hard cap on what the
  encoder allocates. It does not count:
  * the TLM table, 8 bytes per tile part, allocated by `write_headers`
    before the limit applies (at most about 22 MB, for the 2,795,776
    tile parts that TLM segments can index);
  * the scratch space for the tag trees and the header of the packet
    `flush` is writing (a few bytes per code block of one precinct);
  * the spare capacity of growing vectors: the coded bytes are kept in
    one `Vec<u8>`, which can briefly hold up to twice what is counted
    while it grows;
  * the caller's output `Vec`, and small fixed-size buffers.

  A caller who raises the limit past what the system can supply gets
  the allocator's failure, which aborts.
* The first COM segment reads `htj2k Ver <crate version>.` by default.
  `set_version_comment` replaces it, or removes it with `None`. Passing
  `"OpenJPH Ver 0.33.0."` gives OpenJPH's bytes exactly.
* The encoder has its own tree of tiles, resolutions, subbands and
  precincts (`src/codestream/write`), so the decoder's structures and
  memory use are unchanged. It is not shared with the decoder, as
  OpenJPH's is.
* Reversible coding with no decompositions (for any component) writes
  one more magnitude bit-plane in QCD than OpenJPH. OpenJPH's leaves one
  too few there, so it loses the minimum sample (an all-black unsigned
  image decodes mid-grey) and, for 32-bit unsigned samples, the top bit
  of about half of them. These codestreams therefore differ from
  OpenJPH's; they decode losslessly with OpenJPH's decoder and the
  crate's (`tests/data/ours`, see `oracle/README.md`).

## Licence

BSD 2-Clause. This is a derived work of OpenJPH; its copyright notice and
licence are in `LICENSE-OPENJPH` and apply to this crate.
