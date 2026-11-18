# htj2k

htj2k is an encoder and decoder for High-Throughput JPEG 2000 (HTJ2K)
codestreams, written in safe Rust. HTJ2K is Part 15 of JPEG 2000, ITU-T
T.814 | ISO/IEC 15444-15. It replaces the block coder of Part 1 with a
faster one and keeps the rest of the JPEG 2000 codestream.

The crate is a port of [OpenJPH](https://github.com/aous72/OpenJPH)
0.33.0, the reference open-source HTJ2K implementation. The Rust follows
the C++ closely enough to be read side by side with it. The block
encoders and decoders, the wavelet and colour transforms, the marker
segments and the codestream structure each map to the OpenJPH file they
come from.

## Purpose

The crate reads and writes HTJ2K codestreams of any bit depth from Rust
without a C++ toolchain. Samples of up to 32 bits need 64-bit
coefficients in the block coder and the wavelet, so the crate ports
OpenJPH's 64-bit paths as well as its 32-bit ones.

The port holds to four rules.

- It contains no `unsafe` code.
- No input makes it panic or read out of bounds. A damaged codestream,
  an invalid configuration or a call out of order returns an error.
- A caller-set memory limit bounds what a codestream or a configuration
  can make it allocate.
- Its output is OpenJPH's output, except where OpenJPH's is wrong. The
  decoder returns the same samples, and the encoder writes the same
  bytes.

## Supported features

The decoder accepts codestreams whose code blocks all use the HT block
coder. The encoder writes every configuration that OpenJPH's encoder
writes.

- Components of 1 to 32 bits, signed or unsigned.
- The reversible 5/3 and irreversible 9/7 wavelets, with up to 32
  decomposition levels.
- The reversible (RCT) and irreversible (ICT) colour transforms.
- Reversible and irreversible components in one image.
- Quantisation by Qfactor or by explicit steps, per component.
- Nonlinear point transforms (NLT) of type 3, and of type 4 with a
  lookup table.
- Tiles, tile and image offsets, precincts, and per-component coding
  parameters (COC).
- All five progression orders.
- Tile parts at resolutions, at components or both, and TLM markers.
- Subsampled components.
- Planar exchange, one component at a time, and interleaved exchange,
  one line of each component in turn.
- Comment (COM) marker segments.

## Limitations

The crate handles the codestream only, as OpenJPH's library does.

- Part 1 block coding is not supported, so codestreams written by Part 1
  encoders, such as those in digital cinema packages, cannot be read or
  written.
- Codestreams that mix HT and Part 1 code blocks are refused, and so are
  the code-block style options other than vertically causal context.
- The JP2 and JPH file formats are not read or written. A codestream
  inside one has to be extracted first.
- The encoder has no rate control. The size of a lossy codestream
  follows from the quantisation step, not from a target size.
- The encoder writes one quality layer, only the cleanup pass of each
  code block, and no SOP or EPH markers.
- The encoder sets no Rsiz bits other than HT, EXT and NLT, so it
  writes no codestream profile.
- Interleaved exchange needs every component to have the same height.
  Components of different heights are exchanged planar.
- By default every path is OpenJPH's generic code. The `simd` feature
  adds AVX2 kernels on x86_64 (see [The `simd` feature](#the-simd-feature)).
  The 64-bit block coders and code-block conversions (taken by 32-bit
  reversible samples, for example) stay scalar, though 64-bit lines go
  through the same AVX2 wavelet and colour kernels as 32-bit ones.
  Other instruction sets (SSE-only, AVX-512, Arm NEON) have no SIMD
  versions.

## Decoding

`codestream::Codestream` reads a codestream held in memory and returns
it one line at a time. It follows OpenJPH's `ojph::codestream`.
`read_headers` parses the main header, `create` builds the decoder, and
`pull` returns the next line.

```rust
use htj2k::codestream::Codestream;

fn decode(data: &[u8]) -> htj2k::Result<Vec<Vec<i32>>> {
    let mut cs = Codestream::new();
    cs.set_memory_limit(256 << 20);
    cs.read_headers(data)?;
    cs.create()?;
    let mut planes = vec![Vec::new(); cs.access_siz().get_num_components() as usize];
    while let Some((comp, line)) = cs.pull()? {
        planes[comp as usize].extend_from_slice(line.i32());
    }
    Ok(planes)
}
```

Every sample comes back as an `i32`. An unsigned 32-bit sample above
`i32::MAX` comes back as its bit pattern. After `read_headers` the lines
come planar, or interleaved if the codestream uses the colour
transform. You change the order with `set_planar` before `create`.

## Encoding

`codestream::Encoder` writes a codestream into a caller's `Vec<u8>`. You
set the parameters, call `write_headers`, fill each line that `exchange`
returns, and call `flush`.

```rust
use htj2k::codestream::Encoder;
use htj2k::types::Point;

fn encode(width: u32, height: u32, rgb: &[Vec<i32>; 3]) -> htj2k::Result<Vec<u8>> {
    let mut enc = Encoder::new();
    let siz = enc.access_siz_mut()?;
    siz.set_image_extent(Point::new(width, height));
    siz.set_num_components(3);
    for c in 0..3 {
        siz.set_component(c, Point::new(1, 1), 16, true)?;
    }
    let cod = enc.access_cod_mut()?;
    cod.set_reversible(true);
    cod.set_color_transform(true);
    cod.set_block_dims(128, 32)?;
    cod.set_num_decomposition(5)?;
    cod.set_progression_order("RPCL")?;
    enc.set_planar(false)?;

    let mut out = Vec::new();
    enc.write_headers(&mut out, &[])?;
    let mut rows = [0usize; 3];
    while let Some((comp, samples)) = enc.exchange()? {
        let c = comp as usize;
        let start = rows[c] * width as usize;
        samples.copy_from_slice(&rgb[c][start..start + width as usize]);
        rows[c] += 1;
    }
    enc.flush(&mut out)?;
    Ok(out)
}
```

You set the parameters through `access_siz_mut`, `access_cod_mut`,
`access_qcd_mut` and `access_nlt_mut`, and through `set_planar`,
`set_tilepart_divisions`, `request_tlm_marker`, `set_memory_limit`,
`set_output_limit` and `set_version_comment`. Every one of these fails
once `write_headers` has been called. The codestream is appended to the
`Vec`, so the caller can write its own bytes first and reuse the
allocation for the next codestream. A failed call leaves the `Vec` as it
was.

`set_output_limit(bytes)` caps the length of the codestream. Once the
codestream would be longer, the encoder stops with error `0x00080070`
and does not code the rest of the image. `write_headers` stops if the
headers and the other fixed parts are already too long. `exchange`
stops as soon as those and the code blocks coded so far are too long.
It checks after each code block, so a strip of one row of code blocks
stops part way through the row. `flush` adds the packet headers and
stops otherwise, since it knows the exact length before it writes. A
codestream that fits is written exactly as without the limit.

The failing call appends nothing, but the main header from
`write_headers` is still in the `Vec`. A caller who then stores the
samples some other way truncates the `Vec` to its length before
`write_headers`.

The HT block encoders are public too, as `coding::encode_codeblock32`
and `coding::encode_codeblock64`. `tests/readme.rs` runs both examples
above and checks that their text here is the code it runs.

## Reusing a decoder or encoder

Some callers code many small images or strips, for example 32- or
256-line bands of a larger image. For them, most of the cost of a
codestream is building the coder's tree of tiles, resolutions, subbands
and code blocks, with its buffers. Both sides can keep that tree for
the next codestream:

- `Codestream::into_memory(self) -> DecoderMemory` takes the memory out
  of a decoder, at any point and after an error too.
  `Codestream::with_memory(memory: DecoderMemory) -> Codestream<'a>`
  starts a decoder with that memory, in the state `Codestream::new()`
  gives. The decoder borrows its input, so this pair lets one memory
  serve inputs with different lifetimes. `DecoderMemory::new()` (also
  `Default`) is empty.
- `Codestream::reset(&mut self)` does both in place, when the next input
  lives as long as the last.
- `Encoder::reset(&mut self)` returns an encoder to the state
  `Encoder::new()` gives, from any state, and keeps its memory.

Every parameter and setting goes back to its default, including the
memory limit, the output limit, resilience and the version comment. A
reused object differs from a new one only in what it allocates.

`create` (decoder) and `write_headers` (encoder) compare the parameters
the tree is built from with those of the kept tree:

- decoder: SIZ, COD/COC, QCD/QCC, NLT, DFS, the skipped resolutions
  and resilience;
- encoder: SIZ, COD/COC, QCD/QCC and NLT, after they are finalised.

If they are equal, they reset each node's state and zero the lines
handed to the caller. Otherwise they free the old tree and build a new
one. So a codestream of a new shape costs what it did before, and one of
the same shape allocates almost nothing in the tree.

Samples in the kept buffers are not cleared, because every sample is
written before it is read. The tests check this. They fill every kept
buffer with garbage before the next codestream, and every codestream of
the test data must then decode, or encode, exactly as with a new
object. They also run sequences of codestreams with the same shape and
with different shapes, a damaged, failed or abandoned codestream
followed by a valid one, and the robustness fuzz tests through a
reused object.

The memory limit applies to each codestream as to a new object. A kept
tree is charged, against the new limit, what building it took, so the
same codestreams and configurations are accepted and refused, with the
same error. A tree that does not match is freed before the new one is
built. Kept coded-byte capacity beyond what the limit has left is freed
too. So kept memory cannot let a codestream hold more than the limit
allows. The small allocations that the limits do not count (see
Differences from OpenJPH) are kept as well.

Rejected designs:

- A pool of buffers would still rebuild the tree each time. Buffers
  larger than asked for would also escape the limit.
- Keeping the encoder's parameters would write different bytes from a
  new encoder, because `write_headers` rewrites some of them.
- Clearing every kept buffer would cost most of what reuse saves.

`examples/reuse_cost.rs` measures the costs per codestream for a run of
same-shaped codestreams:

- allocations and the bytes they ask for, with a counting allocator;
- user-space instructions, with `perf stat`, as the difference between
  N and 2N codestreams.

The strips use the benchmark's strip coding: 5 levels, 128×32 blocks,
RPCL, TLM and tile parts per resolution. They are 16-bit signed with
NLT type 3, and the 4-component strip uses the RCT on the first three
components. The 3840×2160 image is 12-bit RGB with the RCT and OpenJPH's
defaults.

| Case | Side | Allocations, new → reused | Bytes, new → reused | Instructions, new → reused |
|---|---|--:|--:|--:|
| 2048×32, 1 component | decode | 177 → 5 | 375 KB → 1.0 KB | 6.53 M → 6.42 M (−1.7%) |
| | encode | 236 → 8 | 649 KB → 1.2 KB | 8.38 M → 8.23 M (−1.7%) |
| 256×32, 1 component | decode | 141 → 5 | 58 KB → 1.0 KB | 0.96 M → 0.90 M (−6.2%) |
| | encode | 174 → 8 | 72 KB → 1.2 KB | 1.20 M → 1.13 M (−5.8%) |
| 1920×32, 4 components | decode | 634 → 5 | 1.43 MB → 1.0 KB | 24.9 M → 24.4 M (−2.0%) |
| | encode | 844 → 8 | 2.24 MB → 1.2 KB | 31.8 M → 31.2 M (−1.9%) |
| 3840×2160, 3 components | decode | 872 → 4 | 9.6 MB → 0.2 KB | 2252 M → 2250 M (−0.1%) |
| | encode | 1272 → 7 | 60.5 MB → 0.4 KB | 2952 M → 2948 M (−0.1%) |

The allocations left come from the parameter objects: the decoder's
`read_headers` and the encoder's setters. The new bytes include
reallocations as the coded data grows.

## Memory use

The decoder allocates its code-block buffers at the size of the blocks
in the codestream, not at the nominal block size as OpenJPH does. Its
memory grows with the image width and the code-block height. It grows
much less with the image height and is far from proportional to the
image area.

`examples/peak_heap.rs` measures the decoder's peak heap with a counting
global allocator, from the start of `read_headers` to the last `pull`.
The count includes the `Codestream` object but not the input bytes. The
input is a synthesised codestream of one tile and one signed component,
reversible 5/3 with 5 decomposition levels, with every packet empty.
Memory use depends on the sizes and the coding parameters, not on the
coded data, so empty packets measure the same buffers as real ones.
16-bit components use the 32-bit path and 32-bit components the 64-bit
path.

| Image | Code blocks | 16-bit | 32-bit |
|---|---|--:|--:|
| 32×32 | 64×64 | 17.1 KiB | 24.6 KiB |
| 64×64 | 64×64 | 30.4 KiB | 51.2 KiB |
| 128×32 | 128×32 | 32.0 KiB | 54.1 KiB |
| 8192×32 | 128×32 | 1.40 MiB | 2.74 MiB |
| 8192×32 | 64×64 | 1.42 MiB | 2.76 MiB |
| 8192×256 | 64×64 | 5.42 MiB | 10.76 MiB |

`peak_heap -- slope` gives the cost of each column of width on the
64-bit path, from widths of 4096 and 8192. An image 32 lines high costs
349 to 367 bytes per column for any code-block size from 128×32 to
16×256. With 128×32 blocks a column costs 799 bytes at 256 lines and 924
bytes at 8192 lines. With 16×256 blocks it costs 2159 and 6200 bytes. A
fixed 8.3 KiB comes on top.

Both the encoder and the decoder take a limit through
`set_memory_limit`, 4 GiB by default. The decoder refuses a codestream
that needs more, and the encoder refuses a configuration or coded data
that needs more. Differences from OpenJPH lists what each limit counts.

## Performance

The benchmark in `bench/` counts the CPU cycles and instructions that
htj2k and OpenJPH 0.33.0 take per sample to decode and to encode.
OpenJPH is built twice, without its SIMD kernels (generic, the C++ code
paths htj2k is ported from) and with them (SIMD). htj2k is built three
times: the default build, a `-C target-cpu=native` build and a build
with the `simd` feature. Both sides code the same synthetic images in
memory, on one thread, through matching harnesses. Before timing, the
benchmark requires htj2k's codestreams, from every build, to be
OpenJPH generic's byte for byte, and its decodes to be OpenJPH
generic's sample for sample. All 23 configurations passed.

The machine is an AMD EPYC 7302P (Zen 2) with AVX2 and no AVX-512, so
OpenJPH SIMD runs its AVX2 kernels, and htj2k's `simd` build its AVX2
ones. It runs Linux 6.18 and is shared: the load average was 20.9 to
38.3 during the run. htj2k is built with rustc 1.99.0 in the release
profile, and OpenJPH with g++ 14.2.1 at `-O3`. The metric is
user-space cycles and instructions from `perf stat`, five runs each,
on the least busy core. Each harness runs at N and at 2N iterations.
The difference of the two counts, divided by N and by the number of
samples, is the cost per sample, so set-up, process start and exit
cancel. Because of the load, cycle ratios are good to about ±10 to
20%. Instruction counts vary by less than 0.005%.

The table gives the geometric means of the cycle ratios, for the
default build and for the `simd` build. A ratio above 1 means htj2k
takes more cycles.

| Images | Op | htj2k / generic | htj2k / SIMD | simd / generic | simd / SIMD |
|---|---|--:|--:|--:|--:|
| Strips, reversible, 16-bit | decode | 0.96 | 1.70 | 0.70 | 1.23 |
| | encode | 0.62 | 2.11 | 0.32 | 1.10 |
| Strips, reversible, 32-bit | decode | 1.08 | 1.25 | 0.95 | 1.09 |
| | encode | 0.55 | 0.59 | 0.55 | 0.59 |
| Strips, irreversible | decode | 0.99 | 2.98 | 0.44 | 1.33 |
| | encode | 0.90 | 2.37 | 0.31 | 0.81 |
| Strips, noise | decode | 0.88 | 1.19 | 0.63 | 0.85 |
| | encode | 0.61 | 1.00 | 0.42 | 0.69 |
| 3840×2160, reversible, 12-bit | decode | 0.52 | 1.26 | 0.65 | 1.59 |
| | encode | 0.76 | 2.60 | 0.36 | 1.23 |
| 3840×2160, irreversible, 12-bit | decode | 0.80 | 2.36 | 0.39 | 1.15 |
| | encode | 0.93 | 2.20 | 0.42 | 0.99 |

The default build executes fewer instructions than OpenJPH generic on
most configurations (0.55 to 0.70 times on the reversible encodes,
0.75 to 1.01 on the reversible decodes). Its cycles are 0.43 to 1.34
times OpenJPH generic's. Against OpenJPH SIMD, 16-bit and 12-bit
samples take 1.3 to 3.4 times the cycles to decode and 1.6 to 2.9
times to encode. The 3840×2160 reversible decode (0.52 against
generic, at 0.98 of its instructions, and slower with the `simd`
feature than without) is a noisy row on this run.

With the `simd` feature, htj2k executes 0.41 to 0.90 times the default
build's instructions. On 16-bit and 12-bit samples it is within 25% of
OpenJPH SIMD's cycles on every encode and on half the decodes, and
ahead of it on the irreversible strip encodes. The
largest remaining gaps are decoding: the 16-bit reversible strips
(1.23 times) and the Qfactor 90 strips (up to 1.69 times). There
htj2k's scalar VLC step and its 32-bit MagSgn kernel both take more
instructions than OpenJPH's AVX2 decoder, which also has a 16-bit
MagSgn path. For 32-bit reversible samples OpenJPH has no SIMD 64-bit
block coder. There the `simd` build encodes in 0.54 to 0.66 times
OpenJPH SIMD's cycles and decodes in 0.84 to 1.18 times (the default
build: 0.53 to 0.68 and 0.86 to 1.48).
[bench/README.md](bench/README.md) has the full tables, the profiles,
where the gaps are, and how to reproduce the run.

## The `simd` feature

The `simd` cargo feature, off by default, adds AVX2 versions of hot
kernels. Without it the crate is unchanged: the same code paths and no
dependencies.

```toml
htj2k = { version = "0.1", features = ["simd"] }
```

- On x86_64 the crate checks the CPU once per process and runs the AVX2
  kernels when it has AVX2, BMI1, BMI2, LZCNT and POPCNT (every CPU
  with AVX2 has the others). Elsewhere, and on other architectures, it
  runs the scalar code.
- There are two kinds of kernels. The sample-level kernels are plain
  Rust loops that the compiler vectorises, compiled once for the
  baseline and once for AVX2: the wavelet lifting steps, the colour
  transforms, and the conversions between lines, code blocks and the
  nonlinearities. The other kernels are ports of OpenJPH's AVX2
  intrinsics code: the HT block encoder; the cleanup pass of the
  32-bit block decoder, that is its VLC and MEL step and its MagSgn
  step, in 16-bit lanes for blocks of up to 14 bit-planes and 32-bit
  lanes otherwise, as OpenJPH chooses; and two float-to-integer
  conversions (the irreversible quantisation and
  `irv_convert_to_integer`). Those two were plain loops first, but they
  stayed well behind OpenJPH, because Rust's saturating float-to-integer
  cast does not vectorise on x86.
- 64-bit lines (32-bit reversible samples, for example) go through the
  AVX2 wavelet and colour kernels too: those kernels are written for
  both sample widths. The 64-bit block coders and the 64-bit
  code-block conversions have no AVX2 version, as in OpenJPH, and stay
  scalar. The SigProp and MagRef passes of the 32-bit block decoder
  also stay scalar.
- Build the feature for a baseline x86_64 target (the default
  `x86_64-unknown-linux-gnu` and the like, without `-C target-cpu`)
  to get every kernel. A build whose `-C target-cpu` or
  `-C target-feature` already enables all five features
  (`-C target-cpu=native` on such a CPU, or `x86-64-v3`) vectorises
  the plain loops for that target, but runs none of the intrinsics
  kernels: in their place it runs the scalar block coders and the plain
  loops of the two conversions. This is a consequence of the crate's
  `#![forbid(unsafe_code)]`, which holds under every feature and
  configuration. Rust requires the `#[target_feature]` attribute on any
  caller of the intrinsics kernels even then, and `multiversion` then
  calls a version of the dispatching function without it, so safe code
  cannot reach them.
- A crate that depends on htj2k does not need to forward the feature.
  Cargo unifies features across the dependency graph, so an
  application can turn it on for every use of htj2k in its build by
  naming htj2k in its own `Cargo.toml`, with the same version
  requirement as the crate that uses it:

  ```toml
  [dependencies]
  htj2k = { version = "0.1", features = ["simd"] }
  ```
- Decoding a code block with the feature takes up to about 45 kB of
  stack, against about 10.5 kB without: the AVX2 MagSgn step unstuffs
  the segment into a stack buffer first. The VLC step's buffer, about
  4 kB, is released before the MagSgn step runs, so the peak does not
  change.
- The output does not change: the encoder writes the same bytes and the
  decoder returns the same samples with the feature as without it.
  OpenJPH's own SIMD kernels round some float conversions differently
  from its generic code; the crate's follow the generic code.
- The crate stays free of `unsafe` code. The intrinsics kernels are
  safe `#[target_feature]` functions. Both kinds are selected through
  the [`multiversion`](https://crates.io/crates/multiversion) crate, which
  the feature adds as a dependency (with `rustversion`).
- The feature needs Rust 1.87, for safe intrinsics in
  `#[target_feature]` functions; on an older compiler it stops with an
  error that says so.

To test both builds, and the compile-time selection:

```sh
cargo test --release
cargo test --release --features simd
RUSTFLAGS="-C target-cpu=native" cargo test --release --features simd
cargo clippy --all-targets && cargo clippy --all-targets --features simd
```

With the feature, a unit test checks that the kernels are selected
exactly when the CPU has the features (and the build does not enable
them), so on such a CPU the tests run the AVX2 code, and kernel tests
compare every AVX2 kernel with the scalar code it replaces. The third
command checks that the feature still builds, and passes the tests,
where the target enables the features.

## Verification

Every module is tested against the output of the C++ it is translated
from. The `oracle/` directory builds OpenJPH's generic code and
regenerates the test vectors in `tests/data`. Its README gives the
OpenJPH commit and the commands.

- The decoder returns OpenJPH's samples for all 211 codestreams in
  `tests/data/matrix` and `tests/data/strips`. These include 16- and
  32-bit signed samples under NLT type 3 or 4 in strips of three or
  four components with 128×32 blocks.
- The encoder writes OpenJPH's bytes for 193 of the 198 codestreams
  that `ojph_enc` writes in the parameter matrix. The matrix covers the
  Qfactor, per-component steps and reversibility, NLT type 4 lookup
  tables and extra COM segments.
- The other 5 are reversible with no decompositions and differ on
  purpose. They and 40 random configurations of that kind
  (`tests/data/ours`) decode losslessly with OpenJPH's decoder and with
  the crate's.
- The encoder writes OpenJPH's bytes for all 13 codestreams in
  `tests/data/strips`. These are 300 samples wide and 8 to 200 lines
  high, or 64 wide, with 128×32 blocks. They have three or four
  components under NLT type 3 and the RCT or NLT type 4 and the ICT, or
  one component.
- The encoder writes OpenJPH's bytes for 353 more codestreams at large
  sizes and over quality sweeps (`tests/data/profile`), and the decoder
  returns `ojph_dec`'s samples for them.
- The profile covers short, wide images from 8192×256 down to 8192×1
  and 1×256 with 128×32 blocks, RPCL, TLM and tile parts at
  resolutions. They are reversible with NLT type 3 and the RCT, or
  irreversible with NLT type 4 and the ICT, some with reversible extra
  components.
- The profile also covers sizes of 1 mod 32 with a COM segment, every
  Qfactor, and 54 explicit per-component steps, on 16- and 32-bit
  signed samples.
- The encoder refuses each of the 16 configurations OpenJPH refuses,
  with OpenJPH's error code where OpenJPH gives one.
- The block decoders and encoders reproduce every recorded call in
  `tests/data/blocks` and `tests/data/encoder`.
- Damaged codestreams, random configurations, random samples and
  random call sequences never make either direction panic.
- What the encoder writes from random configurations decodes to planes
  of the sizes SIZ gives. With samples in range, every reversible
  component decodes to exactly its input.

## Differences from OpenJPH

The port changes OpenJPH's behaviour where the C++ would panic, read out
of bounds, loop forever, or write a codestream that is wrong or that no
decoder reads. Elsewhere it follows the C++.

### Both directions

No input makes the crate panic or read out of bounds. Where the C++
would, the crate returns an error.

- The block encoders return an error for arguments on which the C++
  asserts or has undefined behaviour: `num_passes` other than 1,
  `missing_msbs` above 29 (32-bit) or 61 (64-bit), a width above 1024,
  or a sample buffer too small.
- Canvas coordinates of 2^31 and more work in every progression order.
  OpenJPH's encoder and decoder search for the next precinct starting
  from `INT_MAX`, so with RPCL, PCRL or CPRL they assert on such
  images. Both sides here search without a sentinel, which gives the
  same order below 2^31.

### Decoder

The decoder differs from OpenJPH's in one refusal, in its memory, and
in two computations.

- `Codestream::create` refuses planar decoding of a codestream that
  uses the colour transform (`0x00070037`). Planar `pull` cannot invert
  the transform, which needs a line of all three components at once.
  OpenJPH returns components 1 and 2 uninverted.
- `Codestream::set_memory_limit` bounds the memory a codestream can make
  the decoder allocate. The default is 4 GiB.
- Code-block buffers are allocated at the size of the actual blocks,
  not the nominal size as in OpenJPH, so short images and strips, small
  tiles and subband edges take much less memory. OpenJPH's sizing would
  take 23 bytes per column times the nominal block height, against the
  figures in Memory use.
- The 64-bit block decoder computes the value of a sample found
  significant in the SigProp pass with a 64-bit shift. OpenJPH 0.33.0
  shifts in 32 bits, which is wrong for `missing_msbs` of 29 or less.
  Blocks with only a cleanup pass are not affected.
- NLT type 0, and type 3 on an unsigned reversible component, are
  treated as no nonlinearity, as OpenJPH does after 0.33.0.

### Encoder refusals

The encoder returns an error where OpenJPH asserts, loops forever,
wraps, or writes a codestream that no decoder reads.

| Configuration | Error | What OpenJPH does |
|---|---|---|
| LRCP or RLCP with components of different decomposition counts | `0x00080007` | Asserts in `tile_comp::write_precincts`. |
| The same, with tile parts at components | `0x0008000C` | Writes a tile-part count (TNsot) and tile-part indices (TPsot) that no decoder accepts, and that can wrap at 8 bits. |
| Interleaved exchange of components of different heights | `0x00080006` | Its `exchange` cannot do it, and `ojph_enc` reports it. |
| A component with no lines, followed by a component with lines | `0x00080005` | Planar `exchange` asks for a line of it and loops forever. Its decoder loops forever on such a codestream. |
| Precincts 1 sample wide or high above resolution 0 | `0x00080009` | Writes them, then cannot decode them. |
| A tile grid whose tile count does not fit in 32 bits | `0x0008000E` | Its decoder finds no tiles. |
| Decoder-side or unimplemented parameter state | `0x0008000D` | Ignores it, or writes markers the packets do not match. |
| Any setter or `access_*_mut` call after `write_headers` | `0x0008000B` | Changes the stream being written, which can leave it without tile parts, with a wrong TLM, or with a wrong image. |
| The colour transform without a `set_planar` call | `0x00030021` | Picks planar, which it cannot encode correctly. |
| A codestream longer than `set_output_limit` allows | `0x00080070` | Has no output limit. |

The encoder also refuses a tile part longer than its 32-bit length field
can hold (`0x00080060`), a comment longer than a COM segment can hold
(`0x0008000A`), and calls out of order. `exchange` before `write_headers` and `flush`
before every line is exchanged are calls out of order. A setter called
after `write_headers` changes nothing.

Components with no lines after the last one with lines, or with no lines
at all, are encoded as OpenJPH encodes them. `exchange` never asks for
their lines. Precincts 1 sample wide above resolution 0 are left when
precincts are set before the decomposition count is raised.

OpenJPH computes the number of tiles as (extent − tile offset + tile
size − 1) / tile size in 32 bits. Where that wraps, its decoder finds no
tiles. This includes the default tile (extent plus offset) when that sum
does not fit in 32 bits, which OpenJPH's encoder wraps and refuses with
a misleading error.

The parameter objects are shared with the decoder, so `access_*_mut`
reaches `read`, `set_skipped_resolutions` and similar calls. With
`0x0008000D` the encoder refuses skipped resolutions, Rsiz bits other
than HT, EXT and NLT, SOP or EPH markers, and more than one quality
layer. It also refuses code-block style options, ATK or DFS kernels, a
colour transform value other than 0 or 1, and a lookup-table NLT without
an encoding table (one read from a codestream).

### Planar exchange and the colour transform

If you never call `set_planar`, the exchange is interleaved without the
colour transform, as in OpenJPH. With the colour transform OpenJPH then
picks planar, which it cannot encode correctly. `write_headers` refuses
that with `0x00030021`, the error OpenJPH gives for an explicit
`set_planar(true)`. Call `set_planar(false)` to use the colour
transform.

### Encoder interface

The encoder's calls differ from OpenJPH's where a C++ call could leave
the encoder or its output in a broken state.

- A failed call leaves the output `Vec` as it was, and the encoder
  cannot be used further until `reset`.
- `set_output_limit` caps the length of the codestream (`0x00080070`).
  OpenJPH has no such cap.
- `Encoder::reset`, and `Codestream::reset`, `into_memory` and
  `with_memory` on the decoder, keep the tree of tiles and its buffers
  for the next codestream of the same shape (see Reusing a decoder or
  encoder). OpenJPH's `codestream::restart` keeps its allocator's
  storage but builds the tree again for every codestream. It has no
  memory limit to keep.
- `exchange()` takes no arguments. Each call returns the next line to
  fill, as `Option<(component, &mut [i32])>`, and takes back the
  previous one. The encoder keeps its line buffers, so the caller
  cannot pass back a line of the wrong size. OpenJPH passes the filled
  line back as an argument.
- The first COM segment reads `htj2k Ver <crate version>.` by default.
  `set_version_comment` replaces it, or removes it with `None`. Passing
  `"OpenJPH Ver 0.33.0."` gives OpenJPH's bytes exactly.
- The encoder has its own tree of tiles, resolutions, subbands and
  precincts (`src/codestream/write`), so the decoder's structures and
  memory use are unchanged by it. OpenJPH shares one tree between the
  two.

### Encoder memory limit

`Encoder::set_memory_limit` bounds the memory for the tiles (their
structures, line buffers and code-block buffers), the lines exchanged
with the caller, and the coded bytes held until `flush`. The default is
4 GiB. `write_headers` refuses an image whose tiles need more, and
`exchange` refuses coded data beyond it. As in the decoder, the limit is
checked before each allocation. It is not a hard cap on what the
encoder allocates, because it does not count these allocations.

- The TLM table takes 8 bytes per tile part and is allocated by
  `write_headers` before the limit applies. It is at most about 22 MB,
  for the 2,795,776 tile parts that TLM segments can index.
- The scratch space for the tag trees takes a few bytes per code block
  of one precinct. Each precinct's packet header takes a few bytes per
  code block. `reset` keeps both, and the TLM table's storage, for the
  next codestream.
- The coded bytes are kept in one `Vec<u8>`, which can briefly hold up
  to twice what is counted while it grows.
- The caller's output `Vec` and small fixed-size buffers are not
  counted.

A caller who raises the limit past what the system can supply gets the
allocator's failure, which aborts.

### Reversible coding with no decompositions

Reversible coding with no decompositions, for any component, writes one
more magnitude bit-plane in QCD than OpenJPH. OpenJPH's leaves one too
few there, so it loses the minimum sample, and an all-black unsigned
image decodes mid-grey. For 32-bit unsigned samples it also loses the
top bit of about half of the samples. These codestreams therefore differ
from OpenJPH's. They decode losslessly with OpenJPH's decoder and the
crate's (`tests/data/ours`, see `oracle/README.md`).

## Minimum Rust version

The crate builds with Rust 1.83 or later, and with the `simd` feature
with Rust 1.87 or later.

## Licence

BSD 2-Clause. The crate is a derived work of OpenJPH. OpenJPH's
copyright notice and licence are in `LICENSE-OPENJPH` and apply to this
crate.
