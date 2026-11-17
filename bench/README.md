# Speed compared with OpenJPH

How many CPU cycles and instructions htj2k's decoder and encoder take per
sample, against OpenJPH 0.33.0 built with and without its SIMD kernels.

```sh
bench/run.sh            # build everything, check, measure: about 25 minutes
bench/run.sh --profile  # also the perf record profiles
```

The results land in `bench/results/`: `results.md` (the tables below),
`results.csv`, `log.txt` (every command, perf's output and `uptime`
before and after each perf run), `machine.txt`, `lscpu.txt` and, with
`--profile`, `profile.txt`.

## What is compared

| column | build |
|---|---|
| OpenJPH generic | OpenJPH's CMake, Release (`-O3 -DNDEBUG`), static, `-DOJPH_DISABLE_SIMD=ON`: the C++ code paths htj2k is ported from |
| OpenJPH SIMD | OpenJPH's CMake, Release, static, defaults: its SSE/AVX/AVX2/AVX-512 kernels, picked at run time for the CPU |
| htj2k | `cargo build --release` (opt-level 3, no LTO, 16 codegen units) |
| htj2k native | the same with `RUSTFLAGS="-C target-cpu=native"` |

OpenJPH is the tree in `oracle/build/ref/OpenJPH` (0.33.0 plus five
upstream commits, `git describe` 0.33.0-5-g97a9579; its `src/core` is
the code htj2k follows). `oracle/build_bench.sh` builds it out of tree,
twice, and links `oracle/bench_codec.cpp` against each with the same
`-O3 -DNDEBUG` and the library's own definitions.

## Harnesses

`examples/bench_codec.rs` and `oracle/bench_codec.cpp` do the same
work, in-process, on one thread, from memory to memory:

```
bench_codec check   <ojph_enc options>   encode once, decode once, print hashes
bench_codec dump    <ojph_enc options>   the decoded samples to stdout
bench_codec enc N   <ojph_enc options>   N encodes
bench_codec dec N   <ojph_enc options>   N decodes
```

* The options are `oracle/ojph_enc`'s, without `-o`. Both sides
  synthesise `ojph_enc`'s image (`oracle/enc_setup.h`,
  `tests/common/synth.rs`) and make `ojph_enc`'s calls
  (`tests/common/drive.rs`), with OpenJPH's version comment, so the
  codestreams can be compared byte for byte.
* The image is synthesised once, outside the loop, and converted to
  the 32-bit samples the encoders take. For `dec`, the codestream is
  encoded once, also outside the loop.
* Each encode creates the codestream object, sets the parameters,
  writes the headers into a new output buffer (`mem_outfile`, `Vec`),
  copies every line in through `exchange` and flushes.
* Each decode creates the codestream object, reads the headers from
  memory (`mem_infile`, a slice), creates the tiles and copies every
  line that `pull` returns into buffers allocated before the loop.
  Colour-transformed images are pulled interleaved, others planar, as
  `ojph_dec` does.
* Nothing is written to disk. The harness prints the size and
  FNV-1a-64 of the codestream and of the decode of the last iteration,
  and `bench.py` checks that they are the same for N and 2N
  iterations.

## Inputs

Synthetic, deterministic images (`ojph_enc --pattern smooth`): per
component, a diagonal gradient over the full range plus uniform noise
of about ±1/64 of the range. `noise` is uniform over the full range,
the worst case for the block coder. A sample is one pixel of one
component.

| configuration | image | parameters |
|---|---|---|
| `s{16,32}_{1,3,4}c_rev_8192x32`, `..._2048x256` | 16- or 32-bit signed, 1, 3 or 4 components | reversible 5/3, NLT type 3, the RCT on 3 and 4 components, 128x32 blocks, RPCL, 5 levels, TLM, tile parts at resolutions |
| `s{16,32}_3c_ict_q90_*` | the 3-component strips | irreversible 9/7, the ICT, NLT type 4 (a 513- or 257-point LUT, as in `tests/data/profile`), Qfactor 90 |
| `s{16,32}_3c_ict_qs002_8192x32` | the same | the same with a fixed step of 0.002 instead of the Qfactor |
| `s{16,32}_3c_rev_noise_8192x32` | noise | as the reversible strips |
| `u12_3c_rev_3840x2160` | 12-bit unsigned RGB | OpenJPH's defaults: 64x64 blocks, 5 levels, LRCP; reversible, the RCT |
| `u12_3c_ict_q90_3840x2160`, `u12_3c_ict_qs002_3840x2160` | the same | irreversible, the ICT, Qfactor 90 or step 0.002 |

Qfactor 90 weights the quantisation by visual sensitivity, and the
synthetic images' noise is white, so nearly all of it is high-frequency
detail that Qfactor 90 removes: those codestreams are well under a bit
per sample and most code blocks are empty. The fixed step of 0.002 keeps
about 5.5 bits per sample, closer to high-quality irreversible coding of
real images. The tables give the rate of every configuration.

## Measurement

The machine is shared and busy, so wall time and task-clock mean little.
The metric is user-space cycles and instructions from the hardware
counters:

```
taskset -c CPU perf stat -x, -e cycles:u,instructions:u,task-clock -r 5 HARNESS OP N ...
```

* **CPU.** `bench.py` samples `/proc/stat` for three seconds and takes
  the CPU whose physical core, counting its SMT sibling, was least busy.
* **Set-up removed.** Each harness and operation runs at N and at 2N
  iterations, five times each; the per-sample figure is
  (count(2N) − count(N)) / (N × samples). Synthesis, the encode before
  decoding, process start and exit cancel. N is chosen so that N
  iterations code about 16M samples (N = 1 for 3840x2160).
* **Variation.** perf's `-r 5` standard deviation of the mean, for N
  and for 2N, combined in quadrature and given as a percentage of the
  difference. Instructions vary by less than 0.005%. Cycles vary by 1
  to 12% (on half the rows the worst of the four is 6% or more),
  because other jobs use the core's other hardware thread and the
  shared L3. So a cycle ratio is good to about ±10 to 20%, and the
  instruction counts are the steadier guide. Some rows are further off:
  rerun alone, htj2k's 3-component 16-bit reversible encodes took 43 to
  61 cycles per sample (the 1- and 4-component ones 53 to 64), not the
  80 and 88 of the tables, and the
  `u12_3c_ict_qs002` decode row (1.55 against OpenJPH generic, at 0.97
  of its instructions) has 9 to 11% variation on every harness.

## Correctness checks

Before timing, `bench.py` runs `check` on all four harnesses for every
configuration and requires:

* htj2k's codestream, from both builds, to be OpenJPH generic's, byte
  for byte;
* every decode (htj2k, htj2k native, OpenJPH SIMD) to be OpenJPH
  generic's, sample for sample.

All passed: on all 23 configurations, htj2k's codestreams (from both
builds) are OpenJPH generic's byte for byte, and its decodes are OpenJPH
generic's sample for sample.

OpenJPH's two builds do not agree with each other on irreversible
coding, which the benchmark reports but does not require:

| configuration | OpenJPH SIMD against OpenJPH generic |
|---|---|
| `s16_3c_ict_q90_8192x32` | decode: 1242 of 786,432 samples differ, by 1 |
| `s16_3c_ict_q90_2048x256` | decode: 2491 of 1,572,864 samples differ, by 1 |
| `s16_3c_ict_qs002_8192x32` | decode: 1242 of 786,432 samples differ, by 1 |
| `s32_3c_ict_qs002_8192x32` | codestream differs; decode: 6322 of 786,432 samples differ, some by about 2^31 |
| `u12_3c_ict_q90_3840x2160` | decode: 795 of 24,883,200 samples differ, by 1 |
| `u12_3c_ict_qs002_3840x2160` | codestream differs; decode: 4014 of 24,883,200 samples differ, by up to 9 |

The differences of 1 are float rounding in the SIMD kernels. The 2^31
ones are not a SIMD fault as such: compared with the encoder input,
both OpenJPH builds (and so htj2k) decode about 0.1% of the samples of
`s32_3c_ict_qs002_8192x32` wrapped around the ends of the 32-bit range,
which the gradient reaches; the two builds wrap different ones.

## Machine

* **CPU:** AMD EPYC 7302P (Zen 2), 16 cores, 32 threads, boost on.
  AVX2 but no AVX-512, so OpenJPH SIMD runs its AVX2 kernels. 512 KiB
  L2 per core, 16 MiB L3 per four cores. The full `lscpu` is in
  `bench/results/lscpu.txt`.
* **System:** Linux 6.18, shared and busy, with a load average from
  24.7 to 43.1 during the run (`uptime` before and after each of the
  368 perf runs is in `log.txt`). The benchmark ran on CPU 29, whose
  core (with CPU 13) was 78% busy when it was chosen.
* **C++:** g++ 14.2.1, CMake 4.2.2. OpenJPH: `-O3 -DNDEBUG -std=gnu++14
  -fPIC -fexceptions` with `-DOJPH_ALIGNED_ALLOC_EXISTS
  -D_FILE_OFFSET_BITS=64`, plus `-DOJPH_DISABLE_SIMD` for generic; the
  SIMD build's kernel sources get `-msse`... `-mavx2` and `-mavx512f`
  file by file, as OpenJPH's CMake sets them. The harness: `-O3
  -DNDEBUG -std=c++14 -fexceptions` with the same definitions.
* **Rust:** rustc 1.99.0, release profile (opt-level 3, no LTO, 16
  codegen units); the native build adds `-C target-cpu=native`.
* **perf** 6.5.0. The details are in `bench/results/machine.txt`.

## Results

Run of 2026-11-17 at commit 2d3b32c (`bench/results/`), with the steps
of `run.sh` run one by one (the native build's `-C target-cpu=native`
given through a cargo `--config` file rather than `RUSTFLAGS`). Ratios above 1 mean htj2k takes
more. "± max" is the largest variation of the four harnesses on the
row. "native" is htj2k built with `-C target-cpu=native`; OpenJPH
generic was not, so the native / generic columns compare different
instruction sets (BMI, LZCNT, SSE4.1 rounding and AVX2 for the
compiler's own vectorisation, against the x86-64 baseline).

Geometric means of the cycle ratios, by group:

| group | op | htj2k / generic | htj2k / SIMD | native / generic | native / SIMD | instructions, htj2k / generic |
|---|---|--:|--:|--:|--:|--:|
| strips, reversible, 16-bit (6) | decode | 0.95 | 1.77 | 0.83 | 1.55 | 0.94 |
| | encode | 0.75 | 2.48 | 0.61 | 2.03 | 0.66 |
| strips, reversible, 32-bit (6) | decode | 1.04 | 1.18 | 0.82 | 0.93 | 0.98 |
| | encode | 0.66 | 0.68 | 0.57 | 0.59 | 0.57 |
| strips, irreversible (6) | decode | 1.03 | 2.86 | 0.89 | 2.46 | 1.01 |
| | encode | 0.89 | 2.53 | 0.82 | 2.33 | 0.98 |
| strips, noise (2) | decode | 0.76 | 1.37 | 0.67 | 1.20 | 0.82 |
| | encode | 0.66 | 1.02 | 0.63 | 0.97 | 0.58 |
| 3840x2160, reversible (1) | decode | 0.96 | 2.11 | 0.79 | 1.72 | 0.97 |
| | encode | 0.74 | 2.28 | 0.75 | 2.29 | 0.70 |
| 3840x2160, irreversible (2) | decode | 1.13 | 2.32 | 0.80 | 1.65 | 0.90 |
| | encode | 1.04 | 2.35 | 0.88 | 2.00 | 0.97 |

### Decode, all configurations

Cycles per sample (cycles:u), and htj2k's ratios:

| configuration | bits/sample | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k / generic | htj2k / SIMD | native / generic | native / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 13.43 | 33.5 | 18.3 | 35.4 | 26.8 | 1.06 | 1.93 | 0.80 | 1.47 | 5.5% |
| s16_3c_rev_8192x32 | 13.79 | 38.5 | 18.1 | 33.7 | 27.8 | 0.88 | 1.86 | 0.72 | 1.54 | 6.2% |
| s16_4c_rev_8192x32 | 13.69 | 41.2 | 23.6 | 43.2 | 35.1 | 1.05 | 1.83 | 0.85 | 1.49 | 11.2% |
| s32_1c_rev_8192x32 | 29.62 | 54.4 | 47.8 | 53.2 | 48.7 | 0.98 | 1.11 | 0.90 | 1.02 | 5.8% |
| s32_3c_rev_8192x32 | 29.78 | 58.3 | 54.6 | 64.2 | 48.3 | 1.10 | 1.17 | 0.83 | 0.88 | 5.2% |
| s32_4c_rev_8192x32 | 29.72 | 64.1 | 56.8 | 65.2 | 46.8 | 1.02 | 1.15 | 0.73 | 0.82 | 3.6% |
| s16_1c_rev_2048x256 | 12.53 | 32.7 | 18.3 | 33.2 | 25.9 | 1.02 | 1.82 | 0.79 | 1.41 | 7.5% |
| s16_3c_rev_2048x256 | 12.75 | 31.2 | 17.2 | 29.3 | 26.5 | 0.94 | 1.70 | 0.85 | 1.54 | 11.5% |
| s16_4c_rev_2048x256 | 12.65 | 39.6 | 20.3 | 30.9 | 38.5 | 0.78 | 1.52 | 0.97 | 1.89 | 9.2% |
| s32_1c_rev_2048x256 | 28.51 | 56.2 | 46.7 | 50.1 | 42.7 | 0.89 | 1.07 | 0.76 | 0.91 | 4.1% |
| s32_3c_rev_2048x256 | 28.73 | 61.7 | 51.1 | 62.8 | 46.0 | 1.02 | 1.23 | 0.75 | 0.90 | 10.2% |
| s32_4c_rev_2048x256 | 28.69 | 57.9 | 53.5 | 73.1 | 56.1 | 1.26 | 1.37 | 0.97 | 1.05 | 10.2% |
| s16_3c_ict_q90_8192x32 | 1.31 | 60.5 | 30.3 | 63.1 | 57.8 | 1.04 | 2.08 | 0.96 | 1.91 | 12.3% |
| s32_3c_ict_q90_8192x32 | 1.25 | 56.6 | 19.7 | 58.0 | 53.2 | 1.02 | 2.94 | 0.94 | 2.70 | 3.7% |
| s16_3c_ict_q90_2048x256 | 0.60 | 55.8 | 17.8 | 56.3 | 48.8 | 1.01 | 3.17 | 0.87 | 2.75 | 5.1% |
| s32_3c_ict_q90_2048x256 | 0.59 | 49.4 | 14.9 | 57.5 | 47.9 | 1.16 | 3.87 | 0.97 | 3.22 | 4.8% |
| s16_3c_ict_qs002_8192x32 | 5.29 | 73.5 | 24.7 | 68.4 | 62.6 | 0.93 | 2.76 | 0.85 | 2.53 | 9.0% |
| s32_3c_ict_qs002_8192x32 | 5.32 | 76.1 | 30.2 | 79.4 | 58.3 | 1.04 | 2.63 | 0.77 | 1.93 | 6.4% |
| s16_3c_rev_noise_8192x32 | 17.46 | 44.1 | 17.5 | 33.9 | 32.0 | 0.77 | 1.94 | 0.73 | 1.83 | 9.3% |
| s32_3c_rev_noise_8192x32 | 33.47 | 63.0 | 49.2 | 47.8 | 38.7 | 0.76 | 0.97 | 0.61 | 0.79 | 7.0% |
| u12_3c_rev_3840x2160 | 8.14 | 38.1 | 17.4 | 36.7 | 30.0 | 0.96 | 2.11 | 0.79 | 1.72 | 12.2% |
| u12_3c_ict_q90_3840x2160 | 0.04 | 18.4 | 6.8 | 15.0 | 12.2 | 0.82 | 2.22 | 0.66 | 1.80 | 12.3% |
| u12_3c_ict_qs002_3840x2160 | 3.82 | 33.3 | 21.3 | 51.6 | 32.3 | 1.55 | 2.43 | 0.97 | 1.52 | 11.0% |

Instructions per sample (instructions:u):

| configuration | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k / generic | htj2k / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 101.8 | 46.4 | 98.0 | 80.9 | 0.96 | 2.11 | 0.00% |
| s16_3c_rev_8192x32 | 107.2 | 47.0 | 99.6 | 81.8 | 0.93 | 2.12 | 0.00% |
| s16_4c_rev_8192x32 | 105.8 | 46.9 | 99.2 | 81.6 | 0.94 | 2.12 | 0.00% |
| s32_1c_rev_8192x32 | 184.0 | 155.3 | 177.4 | 143.5 | 0.96 | 1.14 | 0.00% |
| s32_3c_rev_8192x32 | 182.7 | 155.8 | 175.3 | 142.7 | 0.96 | 1.13 | 0.00% |
| s32_4c_rev_8192x32 | 183.0 | 155.6 | 176.0 | 143.0 | 0.96 | 1.13 | 0.00% |
| s16_1c_rev_2048x256 | 99.1 | 44.2 | 95.4 | 78.6 | 0.96 | 2.16 | 0.00% |
| s16_3c_rev_2048x256 | 104.2 | 44.9 | 96.9 | 79.4 | 0.93 | 2.16 | 0.00% |
| s16_4c_rev_2048x256 | 102.8 | 44.7 | 96.5 | 79.1 | 0.94 | 2.16 | 0.00% |
| s32_1c_rev_2048x256 | 179.9 | 151.1 | 181.1 | 146.4 | 1.01 | 1.20 | 0.00% |
| s32_3c_rev_2048x256 | 178.8 | 151.8 | 178.6 | 145.4 | 1.00 | 1.18 | 0.00% |
| s32_4c_rev_2048x256 | 179.1 | 151.6 | 179.1 | 145.6 | 1.00 | 1.18 | 0.00% |
| s16_3c_ict_q90_8192x32 | 121.7 | 38.3 | 125.3 | 102.3 | 1.03 | 3.28 | 0.00% |
| s32_3c_ict_q90_8192x32 | 121.1 | 37.8 | 124.9 | 102.0 | 1.03 | 3.30 | 0.00% |
| s16_3c_ict_q90_2048x256 | 104.8 | 27.9 | 104.3 | 83.7 | 1.00 | 3.74 | 0.00% |
| s32_3c_ict_q90_2048x256 | 104.8 | 27.9 | 104.3 | 83.7 | 1.00 | 3.74 | 0.00% |
| s16_3c_ict_qs002_8192x32 | 148.9 | 44.7 | 147.7 | 120.7 | 0.99 | 3.30 | 0.00% |
| s32_3c_ict_qs002_8192x32 | 149.0 | 44.6 | 147.8 | 120.7 | 0.99 | 3.31 | 0.00% |
| s16_3c_rev_noise_8192x32 | 114.7 | 48.2 | 102.6 | 84.7 | 0.89 | 2.13 | 0.00% |
| s32_3c_rev_noise_8192x32 | 193.1 | 166.2 | 143.7 | 116.0 | 0.74 | 0.86 | 0.00% |
| u12_3c_rev_3840x2160 | 93.0 | 37.4 | 90.6 | 75.0 | 0.97 | 2.42 | 0.00% |
| u12_3c_ict_q90_3840x2160 | 52.3 | 16.4 | 43.7 | 27.0 | 0.84 | 2.67 | 0.00% |
| u12_3c_ict_qs002_3840x2160 | 108.1 | 40.8 | 104.8 | 80.1 | 0.97 | 2.57 | 0.00% |

### Encode, all configurations

Cycles per sample (cycles:u), and htj2k's ratios:

| configuration | bits/sample | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k / generic | htj2k / SIMD | native / generic | native / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 13.43 | 79.8 | 23.5 | 48.3 | 44.7 | 0.61 | 2.06 | 0.56 | 1.90 | 3.6% |
| s16_3c_rev_8192x32 | 13.79 | 78.3 | 23.5 | 79.6 | 61.0 | 1.02 | 3.39 | 0.78 | 2.60 | 8.7% |
| s16_4c_rev_8192x32 | 13.69 | 89.6 | 25.7 | 49.3 | 49.5 | 0.55 | 1.92 | 0.55 | 1.93 | 7.4% |
| s32_1c_rev_8192x32 | 29.62 | 129.7 | 106.6 | 67.6 | 61.7 | 0.52 | 0.63 | 0.48 | 0.58 | 3.5% |
| s32_3c_rev_8192x32 | 29.78 | 112.6 | 105.6 | 84.7 | 68.8 | 0.75 | 0.80 | 0.61 | 0.65 | 4.3% |
| s32_4c_rev_8192x32 | 29.72 | 122.1 | 110.2 | 79.9 | 67.0 | 0.65 | 0.73 | 0.55 | 0.61 | 2.4% |
| s16_1c_rev_2048x256 | 12.53 | 77.3 | 23.0 | 53.7 | 41.8 | 0.69 | 2.33 | 0.54 | 1.81 | 4.8% |
| s16_3c_rev_2048x256 | 12.75 | 76.9 | 24.0 | 87.5 | 58.6 | 1.14 | 3.64 | 0.76 | 2.44 | 5.2% |
| s16_4c_rev_2048x256 | 12.65 | 80.1 | 25.3 | 52.3 | 41.8 | 0.65 | 2.07 | 0.52 | 1.65 | 4.7% |
| s32_1c_rev_2048x256 | 28.51 | 108.0 | 106.2 | 67.3 | 58.9 | 0.62 | 0.63 | 0.55 | 0.55 | 4.9% |
| s32_3c_rev_2048x256 | 28.73 | 106.4 | 116.4 | 83.7 | 65.6 | 0.79 | 0.72 | 0.62 | 0.56 | 10.0% |
| s32_4c_rev_2048x256 | 28.69 | 102.4 | 113.4 | 66.9 | 65.6 | 0.65 | 0.59 | 0.64 | 0.58 | 10.5% |
| s16_3c_ict_q90_8192x32 | 1.31 | 88.8 | 38.9 | 74.3 | 73.9 | 0.84 | 1.91 | 0.83 | 1.90 | 3.4% |
| s32_3c_ict_q90_8192x32 | 1.25 | 103.0 | 36.3 | 90.7 | 85.6 | 0.88 | 2.50 | 0.83 | 2.36 | 3.2% |
| s16_3c_ict_q90_2048x256 | 0.60 | 66.4 | 25.9 | 58.4 | 56.2 | 0.88 | 2.26 | 0.85 | 2.17 | 4.5% |
| s32_3c_ict_q90_2048x256 | 0.59 | 79.4 | 32.0 | 77.1 | 67.9 | 0.97 | 2.41 | 0.85 | 2.12 | 3.5% |
| s16_3c_ict_qs002_8192x32 | 5.29 | 116.4 | 40.8 | 120.8 | 97.5 | 1.04 | 2.96 | 0.84 | 2.39 | 4.3% |
| s32_3c_ict_qs002_8192x32 | 5.32 | 144.5 | 32.4 | 111.3 | 103.9 | 0.77 | 3.43 | 0.72 | 3.20 | 8.6% |
| s16_3c_rev_noise_8192x32 | 17.46 | 83.7 | 34.0 | 57.8 | 55.5 | 0.69 | 1.70 | 0.66 | 1.64 | 5.4% |
| s32_3c_rev_noise_8192x32 | 33.47 | 110.4 | 115.1 | 69.8 | 66.1 | 0.63 | 0.61 | 0.60 | 0.57 | 6.0% |
| u12_3c_rev_3840x2160 | 8.14 | 70.2 | 22.9 | 52.1 | 52.3 | 0.74 | 2.28 | 0.75 | 2.29 | 6.3% |
| u12_3c_ict_q90_3840x2160 | 0.04 | 16.0 | 9.3 | 19.9 | 16.3 | 1.24 | 2.14 | 1.02 | 1.76 | 7.6% |
| u12_3c_ict_qs002_3840x2160 | 3.82 | 79.5 | 26.7 | 68.8 | 60.7 | 0.86 | 2.58 | 0.76 | 2.28 | 3.9% |

Instructions per sample (instructions:u):

| configuration | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k / generic | htj2k / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 190.7 | 61.3 | 126.2 | 111.5 | 0.66 | 2.06 | 0.00% |
| s16_3c_rev_8192x32 | 196.6 | 62.0 | 127.8 | 112.3 | 0.65 | 2.06 | 0.00% |
| s16_4c_rev_8192x32 | 195.0 | 61.8 | 127.3 | 112.1 | 0.65 | 2.06 | 0.00% |
| s32_1c_rev_8192x32 | 278.6 | 254.0 | 158.8 | 132.0 | 0.57 | 0.63 | 0.00% |
| s32_3c_rev_8192x32 | 279.2 | 254.6 | 158.4 | 132.4 | 0.57 | 0.62 | 0.00% |
| s32_4c_rev_8192x32 | 279.0 | 254.4 | 158.4 | 132.2 | 0.57 | 0.62 | 0.00% |
| s16_1c_rev_2048x256 | 184.9 | 58.2 | 122.6 | 108.1 | 0.66 | 2.11 | 0.00% |
| s16_3c_rev_2048x256 | 190.3 | 58.7 | 124.0 | 108.9 | 0.65 | 2.11 | 0.00% |
| s16_4c_rev_2048x256 | 188.7 | 58.5 | 123.7 | 108.7 | 0.66 | 2.11 | 0.00% |
| s32_1c_rev_2048x256 | 270.6 | 245.9 | 154.4 | 127.8 | 0.57 | 0.63 | 0.00% |
| s32_3c_rev_2048x256 | 271.5 | 246.7 | 154.2 | 128.6 | 0.57 | 0.62 | 0.00% |
| s32_4c_rev_2048x256 | 271.3 | 246.6 | 154.3 | 128.5 | 0.57 | 0.63 | 0.00% |
| s16_3c_ict_q90_8192x32 | 154.1 | 57.3 | 160.6 | 133.3 | 1.04 | 2.80 | 0.00% |
| s32_3c_ict_q90_8192x32 | 152.9 | 57.0 | 160.2 | 133.0 | 1.05 | 2.81 | 0.00% |
| s16_3c_ict_q90_2048x256 | 124.2 | 40.5 | 129.7 | 104.6 | 1.04 | 3.20 | 0.00% |
| s32_3c_ict_q90_2048x256 | 124.2 | 40.5 | 130.1 | 105.0 | 1.05 | 3.22 | 0.00% |
| s16_3c_ict_qs002_8192x32 | 218.8 | 62.3 | 189.9 | 161.4 | 0.87 | 3.05 | 0.00% |
| s32_3c_ict_qs002_8192x32 | 219.0 | 62.0 | 190.2 | 161.8 | 0.87 | 3.07 | 0.00% |
| s16_3c_rev_noise_8192x32 | 210.5 | 66.6 | 130.1 | 114.6 | 0.62 | 1.95 | 0.00% |
| s32_3c_rev_noise_8192x32 | 293.1 | 268.4 | 161.8 | 135.4 | 0.55 | 0.60 | 0.00% |
| u12_3c_rev_3840x2160 | 170.0 | 53.3 | 118.7 | 105.3 | 0.70 | 2.23 | 0.00% |
| u12_3c_ict_q90_3840x2160 | 41.1 | 19.3 | 45.3 | 28.5 | 1.10 | 2.34 | 0.00% |
| u12_3c_ict_qs002_3840x2160 | 154.0 | 53.9 | 130.1 | 107.8 | 0.84 | 2.41 | 0.00% |

## Profiles

`perf record -e cycles:u` of htj2k and OpenJPH generic on five
configurations; `bench/results/profile.txt` has all of them. Self
time, top 5. The 3840x2160 profiles run only 4 iterations, so the
set-up (`synthesise`, `main` and, for decoding, the encode before the
loop) takes a large share of them; it is left out here.

**Decode, `s16_3c_rev_8192x32`** (htj2k 33.7, generic 38.5 cycles/sample)

| htj2k | % | OpenJPH generic | % |
|---|--:|---|--:|
| `coding::decoder32::decode_codeblock32` | 72.3 | `ojph_decode_codeblock32` | 71.9 |
| `transform::wavelet::rev_horz_syn` | 3.9 | `memmove` | 4.4 |
| `codestream::subband::Subband::pull_line` | 3.6 | `gen_rev_horz_syn` | 3.8 |
| `transform::colour::rev_convert_nlt_type3` | 2.9 | `gen_rev_tx_from_cb32` | 3.6 |
| `transform::wavelet::rev_vert_step` | 2.7 | `gen_rct_backward` | 3.6 |

`memmove` (2.4%) and `memset` (2.4%) follow in htj2k's profile. The
forward bit reader is now inlined into `decode_codeblock32`.

**Decode, `s32_3c_rev_8192x32`** (64.2 against 58.3)

| htj2k | % | OpenJPH generic | % |
|---|--:|---|--:|
| `coding::decoder64::decode_codeblock64` | 72.1 | `ojph_decode_codeblock64` | 68.2 |
| `transform::wavelet::rev_horz_syn` | 6.8 | `gen_rev_horz_syn` | 7.0 |
| `codestream::subband::Subband::pull_line` | 4.8 | `gen_rev_tx_from_cb64` | 6.8 |
| `transform::wavelet::rev_vert_step` | 3.8 | `memmove` | 5.1 |
| `memset` | 2.9 | `gen_rev_vert_step` | 4.1 |

**Decode, `s16_3c_ict_q90_8192x32`** (63.1 against 60.5)

| htj2k | % | OpenJPH generic | % |
|---|--:|---|--:|
| `transform::colour::irv_convert_to_integer_nlt` | 51.9 | `gen_irv_convert_to_integer_nlt` | 55.0 |
| `coding::decoder32::decode_codeblock32` | 27.1 | `ojph_decode_codeblock32` | 24.4 |
| `transform::wavelet::irv_horz_syn` | 4.6 | `gen_irv_horz_syn` | 4.6 |
| `transform::wavelet::irv_vert_step` | 2.8 | `gen_irv_vert_step` | 2.9 |
| `codestream::subband::Subband::pull_line` | 2.6 | `gen_irv_tx_from_cb32` | 2.7 |

**Decode, `u12_3c_rev_3840x2160`**: `decode_codeblock32` leads
htj2k's profile (42.1%, with the set-up encode in 16.8%), as
`ojph_decode_codeblock32` (35.0%) leads OpenJPH's.

**Encode, `s16_3c_rev_8192x32`** (79.6 against 78.3) and
**`s32_3c_rev_8192x32`** (84.7 against 112.6)

| htj2k, 16-bit | % | htj2k, 32-bit | % |
|---|--:|---|--:|
| `coding::encoder::encode` (32-bit) | 78.7 | `coding::encoder::encode` (64-bit) | 70.2 |
| `memmove` | 3.9 | `transform::wavelet::rev_horz_ana` | 5.6 |
| `memset` | 3.3 | `memmove` | 5.2 |
| `transform::wavelet::rev_horz_ana` | 2.8 | `memset` | 4.9 |
| `codestream::write::codeblock::Codeblock::push` | 2.1 | `codestream::write::codeblock::Codeblock::push` | 3.9 |

OpenJPH generic spends 87.6% in `ojph_encode_codeblock32` and 84.0% in
`ojph_encode_codeblock64`, then the same transforms.

**Encode, `s16_3c_ict_q90_8192x32`** (74.3 against 88.8)

| htj2k | % | OpenJPH generic | % |
|---|--:|---|--:|
| `transform::colour::irv_convert_to_float_nlt` | 42.5 | `gen_irv_convert_to_float_nlt` | 49.0 |
| `coding::encoder::encode` | 37.8 | `ojph_encode_codeblock32` | 38.7 |
| `codestream::write::codeblock::Codeblock::push` | 6.4 | `gen_irv_horz_ana` | 3.4 |
| `transform::wavelet::irv_horz_ana` | 3.6 | `gen_ict_forward` | 2.0 |
| `transform::wavelet::irv_vert_step` | 2.4 | `gen_irv_vert_step` | 1.7 |

**Encode, `u12_3c_ict_q90_3840x2160`** (19.9 against 16.0), without
the set-up

| htj2k | % | OpenJPH generic | % |
|---|--:|---|--:|
| `codestream::write::codeblock::Codeblock::push` | 22.5 | `ojph_encode_codeblock32` | 11.1 |
| `transform::wavelet::irv_horz_ana` | 10.9 | `gen_irv_horz_ana` | 10.6 |
| `coding::encoder::encode` | 10.8 | `gen_ict_forward` | 8.4 |
| `transform::wavelet::irv_vert_step` | 6.9 | `gen_irv_vert_step` | 7.0 |
| `memmove` | 4.6 | `gen_irv_tx_to_cb32` | 6.2 |

## Where the gaps are

Against the C++ it is ported from, htj2k now executes fewer
instructions than OpenJPH generic on most rows: 0.55 to 0.70 times on
the reversible encodes, 0.74 to 1.01 on the reversible decodes, and
1.00 to 1.05 on the Qfactor 90 strips, where the float conversions
dominate. In cycles, which on this run are noisy (see Variation), it
is 0.52 to 1.26 times OpenJPH generic on the strips, and the native
build is at or below OpenJPH generic on every row but one (1.02). The
large gaps are to OpenJPH's SIMD build, where OpenJPH has SIMD
kernels.

1. **16-bit and 12-bit samples (32-bit code blocks) against OpenJPH
   SIMD: about 1.8x to decode and 2.5x to encode.** OpenJPH's AVX2
   block encoder and its SIMD block decoder, wavelet and colour kernels
   take the reversible gradient images to 17 to 26 cycles per sample in
   either direction; htj2k needs 29 to 43 to decode and about 43 to 64
   to encode (the 80 and 88 of the 3-component encodes in the tables
   above are this run's load: two reruns gave 43 to 61, with the same
   instruction counts).
   The block encoder is 79% of htj2k's 16-bit encode, so a vectorised
   block encoder is still the largest single item.
2. **Irreversible coding: 1.9 to 3.9x against SIMD.** The conversions
   between integers and floats with NLT type 4 are half of both sides'
   time on the Qfactor 90 strips, and OpenJPH SIMD vectorises them.
   htj2k now truncates instead of calling libm's `floorf` (the two
   agree on every `f32` once saturated to `u32`), so the default build
   no longer needs SSE4.1 to match OpenJPH generic here.
3. **Empty code blocks in the encoder.** `Codeblock::push` is 22% of
   the 3840x2160 Qfactor 90 encode, where nearly every block is
   empty, against 6.2% for OpenJPH's `gen_irv_tx_to_cb32`. This is the
   one row where htj2k executes clearly more instructions than OpenJPH
   generic (1.10x). It also shows on the strips (6% of the irreversible
   encode).
4. **Zeroing and copying.** `memset` (2.4 to 4.9%) is in htj2k's
   top entries on all four reversible strip profiles and in none of
   OpenJPH's, and `Subband::pull_line`
   (2.6 to 4.8%) is a copy that OpenJPH avoids. They are small, but
   they are on every path.

For 32-bit reversible samples OpenJPH's SIMD build gains little over
its generic build, because its 64-bit block coders have no SIMD
version. There the default htj2k build encodes in 0.6 to 0.8 times OpenJPH SIMD's cycles
and decodes in 1.0 to 1.4 times, and the native build decodes in 0.8
to 1.05 times. For 16-bit and 12-bit samples, matching OpenJPH SIMD
needs SIMD block coders, wavelets, and colour and sample conversions:
about 2x for decoding and 2 to 3x for encoding.
