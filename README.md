# htj2k

A High-Throughput JPEG 2000 (HTJ2K, ITU-T T.814 | ISO/IEC 15444-15)
codestream codec in safe Rust, ported from
[OpenJPH](https://github.com/aous72/OpenJPH) 0.33.0.

Work in progress. Decoding works: `codestream::Codestream` reads a
codestream held in memory and returns it line by line, following
`ojph::codestream` (`read_headers`, `create`, `pull`). The encoder is not
ported yet, and neither are OpenJPH's SIMD kernels.

## How it is checked

Every module is tested against output of the C++ it is translated from.
`oracle/` builds OpenJPH's generic code and regenerates the vectors in
`tests/data`; see `oracle/README.md`. The decoder reproduces OpenJPH's
output sample for sample on all 161 codestreams there, which include
16- and 32-bit signed samples under NLT type 3 in strips of four
components with 128x32 blocks.

## Differences from OpenJPH

The port follows OpenJPH closely enough to be read against it, with these
intended differences in behaviour:

* No input makes it panic or read out of bounds. Where the C++ would, it
  returns an error.
* `Codestream::set_memory_limit` bounds the memory a codestream can make
  the decoder allocate. The default is 4 GiB.
* The 64-bit block decoder computes the value of a sample found
  significant in the SigProp pass with a 64-bit shift. OpenJPH 0.33.0
  shifts in 32 bits, which is wrong for `missing_msbs` of 29 or less. Blocks
  with only a cleanup pass are not affected.
* NLT type 0, and type 3 on an unsigned reversible component, are treated
  as no nonlinearity, as OpenJPH does after 0.33.0.

## Licence

BSD 2-Clause. This is a derived work of OpenJPH; its copyright notice and
licence are in `LICENSE-OPENJPH` and apply to this crate.
