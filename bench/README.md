# Speed compared with OpenJPH

How many CPU cycles and instructions htj2k's decoder and encoder take per
sample, with and without its `simd` feature, against OpenJPH 0.33.0
built with and without its SIMD kernels.

```sh
bench/run.sh            # build everything, check, measure: about 30 minutes
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
| htj2k simd | `cargo build --release --features simd`, for the x86-64 baseline: the AVX2 kernels, picked at run time for the CPU |

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
  difference. Instructions vary by less than 0.005%. Cycles vary by
  0.4 to 26% (on 27 of the 46 rows the worst of the five is 6% or
  more), because other jobs use the core's other hardware thread and
  the shared L3. So a cycle ratio is good to about ±10 to 20%, and the
  instruction counts are the steadier guide. The 3840x2160
  irreversible rows are the least steady: 12 to 26% on the worst
  harness, with N = 1 and few cycles per sample.

## Correctness checks

Before timing, `bench.py` runs `check` on all five harnesses for every
configuration and requires:

* htj2k's codestream, from all three builds, to be OpenJPH generic's,
  byte for byte;
* every decode (htj2k, htj2k native, htj2k simd, OpenJPH SIMD) to be
  OpenJPH generic's, sample for sample.

All passed: on all 23 configurations, htj2k's codestreams (from all
three builds, the `simd` one included) are OpenJPH generic's byte for
byte, and its decodes are OpenJPH generic's sample for sample.

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
  20.9 to 38.3 (mean 28.0) during the run (`uptime` before and after
  each of the 460 perf runs is in `log.txt`). The benchmark ran on CPU
  27, whose core (with CPU 11) was 50% busy when it was chosen.
* **C++:** g++ 14.2.1, CMake 4.2.2. OpenJPH: `-O3 -DNDEBUG -std=gnu++14
  -fPIC -fexceptions` with `-DOJPH_ALIGNED_ALLOC_EXISTS
  -D_FILE_OFFSET_BITS=64`, plus `-DOJPH_DISABLE_SIMD` for generic; the
  SIMD build's kernel sources get `-msse`... `-mavx2` and `-mavx512f`
  file by file, as OpenJPH's CMake sets them. The harness: `-O3
  -DNDEBUG -std=c++14 -fexceptions` with the same definitions.
* **Rust:** rustc 1.99.0, release profile (opt-level 3, no LTO, 16
  codegen units); the native build adds `-C target-cpu=native`, and
  the simd build `--features simd` (multiversion 0.9, AVX2 chosen at
  run time).
* **perf** 6.5.0. The details are in `bench/results/machine.txt`.

## Results

Run of 2026-11-17 at commit 114370c (`bench/results/`), with the steps
of `run.sh` run one by one (the native build's `-C target-cpu=native`
given through a cargo `--config` file rather than `RUSTFLAGS`, and
`machine.txt` written from the same sources by a script). Ratios above
1 mean htj2k takes more. "± max" is the largest variation of the five
harnesses on the row. "native" is htj2k built with
`-C target-cpu=native`; OpenJPH generic was not, so the native /
generic columns compare different instruction sets (BMI, LZCNT, SSE4.1
rounding and AVX2 for the compiler's own vectorisation, against the
x86-64 baseline). "simd" is htj2k with the `simd` feature, built for
the baseline and running its AVX2 kernels; simd / SIMD compares it
with OpenJPH's own SIMD build.

Geometric means of the cycle ratios, by group, and of two instruction
ratios:

| group | op | htj2k / generic | htj2k / SIMD | native / generic | simd / generic | simd / SIMD | simd / htj2k | instructions, htj2k / generic | instructions, simd / SIMD |
|---|---|--:|--:|--:|--:|--:|--:|--:|--:|
| Strips, reversible, 16-bit (6) | decode | 0.96 | 1.70 | 0.88 | 0.70 | 1.23 | 0.73 | 0.95 | 1.35 |
|  | encode | 0.62 | 2.11 | 0.58 | 0.32 | 1.10 | 0.52 | 0.65 | 1.00 |
| Strips, reversible, 32-bit (6) | decode | 1.08 | 1.25 | 0.91 | 0.95 | 1.09 | 0.88 | 0.98 | 1.05 |
|  | encode | 0.55 | 0.59 | 0.51 | 0.55 | 0.59 | 1.01 | 0.57 | 0.55 |
| Strips, irreversible (6) | decode | 0.99 | 2.98 | 0.90 | 0.44 | 1.33 | 0.45 | 1.01 | 1.90 |
|  | encode | 0.90 | 2.37 | 0.80 | 0.31 | 0.81 | 0.34 | 0.98 | 1.40 |
| Strips, noise (2) | decode | 0.88 | 1.19 | 0.66 | 0.63 | 0.85 | 0.72 | 0.82 | 1.02 |
|  | encode | 0.61 | 1.00 | 0.55 | 0.42 | 0.69 | 0.69 | 0.58 | 0.73 |
| 3840x2160, reversible (1) | decode | 0.52 | 1.26 | 0.66 | 0.65 | 1.59 | 1.26 | 0.98 | 1.53 |
|  | encode | 0.76 | 2.60 | 0.72 | 0.36 | 1.23 | 0.47 | 0.70 | 1.02 |
| 3840x2160, irreversible (2) | decode | 0.80 | 2.36 | 0.60 | 0.39 | 1.15 | 0.49 | 0.91 | 1.28 |
|  | encode | 0.93 | 2.20 | 0.79 | 0.42 | 0.99 | 0.45 | 0.96 | 0.98 |

### Decode, all configurations

Cycles per sample (cycles:u), and htj2k's ratios:

| configuration | bits/sample | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k simd | htj2k / generic | htj2k / SIMD | native / generic | simd / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 13.43 | 28.4 | 16.3 | 30.5 | 26.6 | 25.4 | 1.07 | 1.87 | 0.94 | 1.56 | 9.5% |
| s16_3c_rev_8192x32 | 13.79 | 35.4 | 20.8 | 34.7 | 44.0 | 26.0 | 0.98 | 1.67 | 1.24 | 1.25 | 7.4% |
| s16_4c_rev_8192x32 | 13.69 | 38.3 | 19.1 | 34.3 | 28.2 | 22.4 | 0.90 | 1.79 | 0.74 | 1.17 | 6.1% |
| s32_1c_rev_8192x32 | 29.62 | 58.6 | 52.2 | 63.5 | 47.9 | 52.0 | 1.08 | 1.22 | 0.82 | 1.00 | 5.7% |
| s32_3c_rev_8192x32 | 29.78 | 65.0 | 52.0 | 65.5 | 57.9 | 55.0 | 1.01 | 1.26 | 0.89 | 1.06 | 6.9% |
| s32_4c_rev_8192x32 | 29.72 | 65.8 | 57.7 | 63.4 | 69.8 | 60.5 | 0.96 | 1.10 | 1.06 | 1.05 | 10.9% |
| s16_1c_rev_2048x256 | 12.53 | 30.0 | 17.8 | 28.7 | 26.2 | 21.3 | 0.96 | 1.62 | 0.87 | 1.20 | 11.9% |
| s16_3c_rev_2048x256 | 12.75 | 31.1 | 17.0 | 29.3 | 25.1 | 21.8 | 0.94 | 1.72 | 0.81 | 1.28 | 5.4% |
| s16_4c_rev_2048x256 | 12.65 | 34.3 | 20.6 | 31.3 | 26.4 | 20.4 | 0.91 | 1.52 | 0.77 | 0.99 | 6.3% |
| s32_1c_rev_2048x256 | 28.51 | 57.9 | 46.5 | 53.2 | 49.9 | 52.3 | 0.92 | 1.14 | 0.86 | 1.12 | 6.9% |
| s32_3c_rev_2048x256 | 28.73 | 52.4 | 48.9 | 64.2 | 51.1 | 56.8 | 1.23 | 1.31 | 0.98 | 1.16 | 5.3% |
| s32_4c_rev_2048x256 | 28.69 | 52.4 | 47.3 | 70.2 | 46.5 | 56.0 | 1.34 | 1.48 | 0.89 | 1.18 | 9.6% |
| s16_3c_ict_q90_8192x32 | 1.31 | 54.6 | 19.3 | 65.1 | 54.1 | 32.6 | 1.19 | 3.37 | 0.99 | 1.69 | 3.5% |
| s32_3c_ict_q90_8192x32 | 1.25 | 52.2 | 18.1 | 54.0 | 54.1 | 26.4 | 1.03 | 2.98 | 1.04 | 1.46 | 3.8% |
| s16_3c_ict_q90_2048x256 | 0.60 | 53.5 | 18.7 | 44.1 | 44.6 | 21.6 | 0.83 | 2.36 | 0.83 | 1.15 | 5.3% |
| s32_3c_ict_q90_2048x256 | 0.59 | 57.3 | 17.9 | 54.8 | 42.5 | 18.7 | 0.96 | 3.06 | 0.74 | 1.05 | 14.0% |
| s16_3c_ict_qs002_8192x32 | 5.29 | 70.1 | 21.1 | 63.6 | 60.9 | 29.6 | 0.91 | 3.02 | 0.87 | 1.41 | 3.4% |
| s32_3c_ict_qs002_8192x32 | 5.32 | 65.7 | 22.0 | 69.5 | 64.7 | 28.7 | 1.06 | 3.16 | 0.98 | 1.30 | 9.2% |
| s16_3c_rev_noise_8192x32 | 17.46 | 39.7 | 23.2 | 38.4 | 26.1 | 20.1 | 0.97 | 1.65 | 0.66 | 0.86 | 8.0% |
| s32_3c_rev_noise_8192x32 | 33.47 | 63.7 | 58.8 | 50.6 | 42.2 | 49.6 | 0.80 | 0.86 | 0.66 | 0.84 | 5.3% |
| u12_3c_rev_3840x2160 | 8.14 | 41.6 | 17.0 | 21.5 | 27.3 | 27.1 | 0.52 | 1.26 | 0.66 | 1.59 | 8.3% |
| u12_3c_ict_q90_3840x2160 | 0.04 | 18.7 | 6.2 | 13.1 | 12.2 | 8.5 | 0.70 | 2.13 | 0.65 | 1.38 | 22.2% |
| u12_3c_ict_qs002_3840x2160 | 3.82 | 58.1 | 20.5 | 53.6 | 32.2 | 19.5 | 0.92 | 2.62 | 0.55 | 0.95 | 26.2% |

Instructions per sample (instructions:u):

| configuration | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k simd | htj2k / generic | htj2k / SIMD | simd / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 101.8 | 46.4 | 98.2 | 81.1 | 63.0 | 0.97 | 2.12 | 1.36 | 0.00% |
| s16_3c_rev_8192x32 | 107.2 | 47.0 | 99.8 | 82.0 | 63.8 | 0.93 | 2.12 | 1.36 | 0.00% |
| s16_4c_rev_8192x32 | 105.8 | 46.9 | 99.4 | 81.8 | 63.6 | 0.94 | 2.12 | 1.36 | 0.00% |
| s32_1c_rev_8192x32 | 184.0 | 155.3 | 177.6 | 143.7 | 159.5 | 0.97 | 1.14 | 1.03 | 0.00% |
| s32_3c_rev_8192x32 | 182.7 | 155.8 | 175.5 | 142.9 | 158.5 | 0.96 | 1.13 | 1.02 | 0.00% |
| s32_4c_rev_8192x32 | 183.0 | 155.6 | 176.2 | 143.2 | 158.9 | 0.96 | 1.13 | 1.02 | 0.00% |
| s16_1c_rev_2048x256 | 99.1 | 44.2 | 95.6 | 78.7 | 59.9 | 0.96 | 2.16 | 1.35 | 0.00% |
| s16_3c_rev_2048x256 | 104.2 | 44.9 | 97.0 | 79.5 | 60.6 | 0.93 | 2.16 | 1.35 | 0.00% |
| s16_4c_rev_2048x256 | 102.8 | 44.7 | 96.6 | 79.3 | 60.4 | 0.94 | 2.16 | 1.35 | 0.00% |
| s32_1c_rev_2048x256 | 179.9 | 151.1 | 181.2 | 146.6 | 163.1 | 1.01 | 1.20 | 1.08 | 0.00% |
| s32_3c_rev_2048x256 | 178.8 | 151.8 | 178.8 | 145.6 | 161.8 | 1.00 | 1.18 | 1.07 | 0.00% |
| s32_4c_rev_2048x256 | 179.1 | 151.6 | 179.3 | 145.7 | 162.0 | 1.00 | 1.18 | 1.07 | 0.00% |
| s16_3c_ict_q90_8192x32 | 121.7 | 38.3 | 125.6 | 102.5 | 72.0 | 1.03 | 3.28 | 1.88 | 0.00% |
| s32_3c_ict_q90_8192x32 | 121.1 | 37.8 | 125.2 | 102.2 | 71.5 | 1.03 | 3.31 | 1.89 | 0.00% |
| s16_3c_ict_q90_2048x256 | 104.8 | 27.9 | 104.5 | 83.8 | 54.5 | 1.00 | 3.75 | 1.95 | 0.00% |
| s32_3c_ict_q90_2048x256 | 104.8 | 27.9 | 104.5 | 83.9 | 54.5 | 1.00 | 3.75 | 1.96 | 0.00% |
| s16_3c_ict_qs002_8192x32 | 148.9 | 44.7 | 148.0 | 120.9 | 82.6 | 0.99 | 3.31 | 1.85 | 0.00% |
| s32_3c_ict_qs002_8192x32 | 149.0 | 44.6 | 148.0 | 120.9 | 82.6 | 0.99 | 3.32 | 1.85 | 0.00% |
| s16_3c_rev_noise_8192x32 | 114.7 | 48.2 | 102.8 | 84.9 | 66.2 | 0.90 | 2.13 | 1.37 | 0.00% |
| s32_3c_rev_noise_8192x32 | 193.1 | 166.2 | 143.9 | 116.2 | 126.9 | 0.75 | 0.87 | 0.76 | 0.00% |
| u12_3c_rev_3840x2160 | 93.0 | 37.4 | 90.9 | 75.3 | 57.4 | 0.98 | 2.43 | 1.53 | 0.00% |
| u12_3c_ict_q90_3840x2160 | 52.3 | 16.4 | 44.1 | 27.5 | 18.2 | 0.84 | 2.69 | 1.11 | 0.00% |
| u12_3c_ict_qs002_3840x2160 | 108.1 | 40.8 | 105.2 | 80.4 | 60.7 | 0.97 | 2.58 | 1.49 | 0.00% |

### Encode, all configurations

Cycles per sample (cycles:u), and htj2k's ratios:

| configuration | bits/sample | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k simd | htj2k / generic | htj2k / SIMD | native / generic | simd / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 13.43 | 92.3 | 25.2 | 50.6 | 48.5 | 30.6 | 0.55 | 2.01 | 0.53 | 1.21 | 7.2% |
| s16_3c_rev_8192x32 | 13.79 | 91.1 | 24.7 | 40.0 | 47.1 | 26.0 | 0.44 | 1.62 | 0.52 | 1.05 | 12.1% |
| s16_4c_rev_8192x32 | 13.69 | 82.7 | 32.1 | 65.0 | 50.9 | 29.9 | 0.79 | 2.03 | 0.62 | 0.93 | 5.6% |
| s32_1c_rev_8192x32 | 29.62 | 109.4 | 94.0 | 63.7 | 66.6 | 60.6 | 0.58 | 0.68 | 0.61 | 0.64 | 3.3% |
| s32_3c_rev_8192x32 | 29.78 | 100.4 | 109.2 | 58.2 | 51.1 | 72.0 | 0.58 | 0.53 | 0.51 | 0.66 | 3.9% |
| s32_4c_rev_8192x32 | 29.72 | 149.6 | 118.1 | 64.8 | 63.9 | 67.3 | 0.43 | 0.55 | 0.43 | 0.57 | 9.9% |
| s16_1c_rev_2048x256 | 12.53 | 77.0 | 19.5 | 48.2 | 47.4 | 23.2 | 0.63 | 2.47 | 0.62 | 1.19 | 5.1% |
| s16_3c_rev_2048x256 | 12.75 | 83.9 | 24.3 | 55.3 | 47.5 | 29.5 | 0.66 | 2.28 | 0.57 | 1.22 | 6.2% |
| s16_4c_rev_2048x256 | 12.65 | 73.6 | 22.8 | 54.3 | 47.7 | 23.8 | 0.74 | 2.38 | 0.65 | 1.04 | 5.4% |
| s32_1c_rev_2048x256 | 28.51 | 105.6 | 98.2 | 59.4 | 51.7 | 54.8 | 0.56 | 0.60 | 0.49 | 0.56 | 5.7% |
| s32_3c_rev_2048x256 | 28.73 | 109.4 | 109.4 | 64.6 | 58.6 | 63.4 | 0.59 | 0.59 | 0.54 | 0.58 | 6.7% |
| s32_4c_rev_2048x256 | 28.69 | 109.9 | 107.8 | 63.5 | 58.0 | 59.3 | 0.58 | 0.59 | 0.53 | 0.55 | 2.2% |
| s16_3c_ict_q90_8192x32 | 1.31 | 83.8 | 38.2 | 85.8 | 71.4 | 30.8 | 1.02 | 2.25 | 0.85 | 0.81 | 6.7% |
| s32_3c_ict_q90_8192x32 | 1.25 | 97.2 | 42.0 | 88.7 | 78.4 | 34.0 | 0.91 | 2.11 | 0.81 | 0.81 | 6.4% |
| s16_3c_ict_q90_2048x256 | 0.60 | 67.5 | 27.2 | 59.4 | 54.4 | 22.4 | 0.88 | 2.18 | 0.81 | 0.82 | 4.9% |
| s32_3c_ict_q90_2048x256 | 0.59 | 83.3 | 31.4 | 76.6 | 67.1 | 23.4 | 0.92 | 2.44 | 0.81 | 0.74 | 6.0% |
| s16_3c_ict_qs002_8192x32 | 5.29 | 136.1 | 42.0 | 105.9 | 99.0 | 34.3 | 0.78 | 2.52 | 0.73 | 0.82 | 5.4% |
| s32_3c_ict_qs002_8192x32 | 5.32 | 133.3 | 42.2 | 117.6 | 105.7 | 37.5 | 0.88 | 2.78 | 0.79 | 0.89 | 3.8% |
| s16_3c_rev_noise_8192x32 | 17.46 | 74.5 | 29.2 | 49.0 | 44.2 | 25.6 | 0.66 | 1.68 | 0.59 | 0.88 | 7.9% |
| s32_3c_rev_noise_8192x32 | 33.47 | 114.8 | 107.5 | 64.1 | 57.6 | 57.7 | 0.56 | 0.60 | 0.50 | 0.54 | 5.7% |
| u12_3c_rev_3840x2160 | 8.14 | 69.7 | 20.4 | 53.1 | 50.3 | 25.0 | 0.76 | 2.60 | 0.72 | 1.23 | 6.1% |
| u12_3c_ict_q90_3840x2160 | 0.04 | 14.4 | 10.5 | 17.5 | 12.9 | 8.7 | 1.21 | 1.67 | 0.90 | 0.83 | 11.8% |
| u12_3c_ict_qs002_3840x2160 | 3.82 | 91.6 | 22.4 | 64.9 | 62.9 | 26.2 | 0.71 | 2.90 | 0.69 | 1.17 | 13.2% |

Instructions per sample (instructions:u):

| configuration | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k simd | htj2k / generic | htj2k / SIMD | simd / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 190.7 | 61.3 | 125.6 | 110.9 | 61.4 | 0.66 | 2.05 | 1.00 | 0.00% |
| s16_3c_rev_8192x32 | 196.6 | 62.0 | 127.1 | 111.6 | 62.1 | 0.65 | 2.05 | 1.00 | 0.00% |
| s16_4c_rev_8192x32 | 195.0 | 61.8 | 126.7 | 111.4 | 61.9 | 0.65 | 2.05 | 1.00 | 0.00% |
| s32_1c_rev_8192x32 | 278.6 | 254.0 | 158.1 | 131.2 | 140.3 | 0.57 | 0.62 | 0.55 | 0.00% |
| s32_3c_rev_8192x32 | 279.2 | 254.6 | 157.7 | 131.8 | 140.9 | 0.56 | 0.62 | 0.55 | 0.00% |
| s32_4c_rev_8192x32 | 279.0 | 254.4 | 157.8 | 131.7 | 140.8 | 0.57 | 0.62 | 0.55 | 0.00% |
| s16_1c_rev_2048x256 | 184.9 | 58.2 | 122.5 | 108.0 | 57.9 | 0.66 | 2.10 | 0.99 | 0.00% |
| s16_3c_rev_2048x256 | 190.3 | 58.7 | 123.9 | 108.7 | 58.5 | 0.65 | 2.11 | 1.00 | 0.00% |
| s16_4c_rev_2048x256 | 188.7 | 58.5 | 123.5 | 108.5 | 58.2 | 0.65 | 2.11 | 0.99 | 0.00% |
| s32_1c_rev_2048x256 | 270.6 | 245.9 | 154.2 | 127.7 | 136.4 | 0.57 | 0.63 | 0.55 | 0.00% |
| s32_3c_rev_2048x256 | 271.5 | 246.7 | 154.0 | 128.5 | 137.2 | 0.57 | 0.62 | 0.56 | 0.00% |
| s32_4c_rev_2048x256 | 271.3 | 246.6 | 154.1 | 128.3 | 137.1 | 0.57 | 0.63 | 0.56 | 0.00% |
| s16_3c_ict_q90_8192x32 | 154.1 | 57.3 | 160.0 | 132.7 | 78.3 | 1.04 | 2.79 | 1.37 | 0.00% |
| s32_3c_ict_q90_8192x32 | 152.9 | 57.0 | 159.6 | 132.3 | 77.9 | 1.04 | 2.80 | 1.37 | 0.00% |
| s16_3c_ict_q90_2048x256 | 124.2 | 40.5 | 129.5 | 104.4 | 60.1 | 1.04 | 3.20 | 1.48 | 0.00% |
| s32_3c_ict_q90_2048x256 | 124.2 | 40.5 | 130.0 | 104.9 | 60.1 | 1.05 | 3.21 | 1.48 | 0.00% |
| s16_3c_ict_qs002_8192x32 | 218.8 | 62.3 | 189.3 | 160.8 | 83.4 | 0.86 | 3.04 | 1.34 | 0.00% |
| s32_3c_ict_qs002_8192x32 | 219.0 | 62.0 | 189.6 | 161.2 | 83.1 | 0.87 | 3.06 | 1.34 | 0.00% |
| s16_3c_rev_noise_8192x32 | 210.5 | 66.6 | 129.4 | 114.0 | 65.4 | 0.61 | 1.94 | 0.98 | 0.00% |
| s32_3c_rev_noise_8192x32 | 293.1 | 268.4 | 161.2 | 134.7 | 144.4 | 0.55 | 0.60 | 0.54 | 0.00% |
| u12_3c_rev_3840x2160 | 170.0 | 53.3 | 118.5 | 105.1 | 54.2 | 0.70 | 2.22 | 1.02 | 0.00% |
| u12_3c_ict_q90_3840x2160 | 41.1 | 19.3 | 45.2 | 28.5 | 18.3 | 1.10 | 2.34 | 0.95 | 0.00% |
| u12_3c_ict_qs002_3840x2160 | 154.0 | 53.9 | 129.9 | 107.6 | 55.1 | 0.84 | 2.41 | 1.02 | 0.00% |

## Profiles

`perf record -e cycles:u` of OpenJPH generic, OpenJPH SIMD, htj2k and
htj2k simd on five configurations; `bench/results/profile.txt` has all
of them. Self time, top entries. The 3840x2160 profiles run only 4
iterations, so the set-up (`synthesise`, `main` and, for decoding, the
encode before the loop) takes a large share of them; it is left out
here. Symbol names are shortened: `vectorised::rev_horz_syn` is the
AVX2 clone that multiversion compiles,
`vectorised::rev_horz_syn::rev_horz_syn_avx2_bmi1_bmi2_lzcnt_popcnt_version`.

The default build's profiles have the same shape as OpenJPH generic's:
`decode_codeblock32` takes 71.1% of the `s16_3c_rev_8192x32` decode
(`ojph_decode_codeblock32` 69.6%), `encoder::encode` 77.9% of its
encode (`ojph_encode_codeblock32` 87.6%), and the NLT type 4
conversions about half of the Qfactor 90 strips on both sides.

**Decode, `s16_3c_rev_8192x32`** (htj2k simd 26.0, OpenJPH SIMD 20.8
cycles/sample)

| htj2k simd | % | OpenJPH SIMD | % |
|---|--:|---|--:|
| `coding::decoder_avx2::magsgn` | 37.0 | `decode_cb_step2_32bit` | 37.9 |
| `coding::decoder32::decode_codeblock32` | 29.5 | `decode_cb_step1_vlc` | 31.1 |
| `vectorised::rev_horz_syn` | 4.3 | `memmove` | 5.9 |
| `memset` | 4.3 | `avx2_rev_horz_syn` | 4.2 |
| `vectorised::rev_tx_from_cb32` | 3.5 | `avx2_rev_tx_from_cb32` | 3.8 |

Here `decode_codeblock32` is the scalar VLC and MEL step around the
AVX2 MagSgn kernel; OpenJPH's `decode_cb_step1_vlc`, part of its AVX2
block decoder, does the same work in fewer instructions (13.5 a
sample against htj2k's 21.2; see below).

**Encode, `s16_3c_rev_8192x32`** (26.0 against 24.7)

| htj2k simd | % | OpenJPH SIMD | % |
|---|--:|---|--:|
| `coding::encoder_avx2::proc_ms_encode` | 35.5 | `ojph_encode_codeblock_avx2` | 32.8 |
| `coding::encoder_avx2::encode32` | 31.8 | `proc_ms_encode` | 31.1 |
| `memmove` | 6.5 | `proc_pixel` | 8.1 |
| `memset` | 5.1 | `memmove` | 8.0 |
| `vectorised::rev_horz_ana` | 3.5 | `avx2_rev_horz_ana` | 4.5 |

(htj2k inlines `proc_pixel` into `encode32`.)

**Decode, `s16_3c_ict_q90_8192x32`** (32.6 against 19.3)

| htj2k simd | % | OpenJPH SIMD | % |
|---|--:|---|--:|
| `coding::decoder32::decode_codeblock32` | 28.1 | `avx2_irv_convert_to_integer_nlt` | 26.7 |
| `vectorised::irv_convert_to_integer_nlt2or4` | 26.8 | `decode_cb_step1_vlc` | 26.7 |
| `coding::decoder_avx2::magsgn` | 14.0 | `decode_cb_step2_16bit` | 12.4 |
| `vectorised::irv_horz_syn` | 6.8 | `avx_irv_horz_syn` | 7.3 |
| `vectorised::irv_vert_step` | 4.6 | `avx_irv_vert_step` | 5.6 |

OpenJPH decodes these blocks with its 16-bit MagSgn path
(`decode_cb_step2_16bit`), which htj2k does not have; htj2k runs its
32-bit one.

**Encode, `s16_3c_ict_q90_8192x32`** (30.8 against 38.2)

| htj2k simd | % | OpenJPH SIMD | % |
|---|--:|---|--:|
| `vectorised::irv_convert_to_float_nlt2or4_precise_1step` | 27.1 | `avx2_irv_convert_to_float_nlt` | 38.8 |
| `coding::encoder_avx2::encode32` | 26.2 | `ojph_encode_codeblock_avx2` | 23.4 |
| `coding::encoder_avx2::proc_ms_encode` | 21.1 | `proc_ms_encode` | 12.8 |
| `vectorised::irv_horz_ana` | 5.7 | `proc_pixel` | 6.0 |
| `vectorised::irv_vert_step` | 4.1 | `avx_irv_horz_ana` | 5.0 |

**32-bit reversible samples** (`s32_3c_rev_8192x32`) take the 64-bit
block coders, which neither side vectorises: `decode_codeblock64` is
75.7% of the simd decode, `ojph_decode_codeblock64` 75.0% of OpenJPH
SIMD's, and the encoders 73.4% and 88.9%.

**3840x2160**: the reversible decode is led by `decoder_avx2::magsgn`
(19.2%) and `decode_codeblock32` (17.6%), against OpenJPH SIMD's
`decode_cb_step1_vlc` (14.1%) and `decode_cb_step2_16bit` (9.6%). In
the Qfactor 90 decode, where almost every block is empty, the default
build spends 17.3% in `irv_convert_to_integer`. With the feature that
is the intrinsics kernel `vectorised::avx2::irv_convert_to_integer`
(2.9%, OpenJPH SIMD's `avx2_irv_convert_to_integer` 1.9%), and the
wavelet leads, as on OpenJPH SIMD: `irv_horz_syn` 7.1% and
`irv_vert_step` 5.9%, against 5.2% and 6.1%.

## Where the gaps are

**The default build against OpenJPH generic.** htj2k executes fewer
instructions than the C++ it is ported from on most rows: 0.55 to 0.70
times on the reversible encodes, 0.75 to 1.01 on the reversible
decodes, and 1.00 to 1.05 on the Qfactor 90 strips, where the float
conversions dominate. In cycles, which on this run are noisy (see
Variation), it is 0.43 to 1.34 times OpenJPH generic on the strips.
The native build is at or below OpenJPH generic on all but three rows
(1.04 to 1.24, with the same instructions as the rows around them).

**The `simd` build against OpenJPH SIMD.** With the feature, htj2k
takes 0.25 to 1.09 times OpenJPH generic's cycles, and executes 0.41
to 0.90 times the default build's instructions. Against OpenJPH SIMD,
the 16-bit and 12-bit rows take 0.86 to 1.69 times its cycles to
decode and 0.81 to 1.23 times to encode. What is left:

1. **The block decoder.** On the 16-bit reversible decodes (1.35x in
   instructions, 1.23x in cycles) the gap is split about evenly.
   Measured by symbol on `s16_3c_rev_8192x32` (perf N/2N,
   instructions per sample):
   - htj2k's MagSgn kernel takes 29.7 against OpenJPH's 22.5
     (`decoder_avx2::magsgn` and `row_u_q` against
     `decode_cb_step2_32bit`);
   - its scalar VLC and MEL step takes 21.2 (`decode_codeblock32`)
     against 13.5 for OpenJPH's `decode_cb_step1_vlc`, part of its
     AVX2 block decoder.
   The whole decode is 63.7 against 46.9.
2. **OpenJPH's 16-bit MagSgn path.** For blocks whose magnitudes fit
   in 16 bits, OpenJPH decodes with `decode_cb_step2_16bit`, half the
   width of its 32-bit path. htj2k has only the 32-bit path. On the
   3840x2160 reversible decode (1.53x in instructions; its 1.59x in
   cycles is a noisy row), MagSgn takes htj2k 25.4 instructions per
   sample. OpenJPH takes 12.5 there, 7.9 in its 16-bit path and 4.5 in
   its 32-bit one. Its VLC step takes 15.3 against htj2k's 22.1.
3. **The NLT type 4 conversions.** These are table lookups. htj2k's
   versions are plain loops that the compiler vectorises around scalar
   lookups. They execute four to five times OpenJPH's instructions
   (about 21 a sample against 4 on decoding, about 24 against 6 on
   encoding), but take about as many cycles, since OpenJPH waits on
   its gathers. So the irreversible strips run 1.90x OpenJPH's
   instructions to decode but 1.33x its cycles, and 1.40x the
   instructions to encode but 0.81x the cycles.
4. **Zeroing and copying.** `memset` takes 1.7 to 5.1% of every simd
   strip profile and is in none of OpenJPH's. `Codeblock::pull_line` is a
   copy that OpenJPH avoids.

For 32-bit reversible samples neither side has SIMD block coders,
because they take the 64-bit paths, so the feature changes only the
wavelet, colour and line conversion kernels there (0.88 to 0.90 times the default
build's instructions to decode). htj2k simd encodes them in 0.54 to
0.66 times OpenJPH SIMD's cycles and decodes them in 0.84 to 1.18
times.
