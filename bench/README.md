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
| htj2k | `cargo build --release --no-default-features`, the scalar build without the `simd` feature (opt-level 3, no LTO, 16 codegen units) |
| htj2k native | the same with `RUSTFLAGS="-C target-cpu=native"` |
| htj2k simd | `cargo build --release --features simd` (the default features), for the x86-64 baseline: the AVX2 kernels, picked at run time for the CPU |

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
  0.1 to 22% (on 31 of the 46 rows the worst of the five is 6% or
  more), because other jobs use the core's other hardware thread and
  the shared L3. So a cycle ratio is good to about ±10 to 20%, and the
  instruction counts are the steadier guide. On this run the least
  steady rows are spread over the configurations (up to 22% on
  `s32_3c_rev_2048x256` encoding and `s16_3c_rev_8192x32` decoding).

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
  7.5 to 35.7 (mean 17.6) during the run (`uptime` before and after
  each of the 460 perf runs is in `log.txt`). The benchmark ran on CPU
  29, whose core (with CPU 13) was 28% busy when it was chosen.
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

Run of 2026-11-17 at commit 3cc0f65 (`bench/results/`), with the steps
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
with OpenJPH's own SIMD build. On this shared machine single cycle
counts move by up to about 20% from one harness run to the next (see
"± max"), so a single row's cycle ratio can be well off; the
instruction counts, which vary by under 0.005%, are the figures to
compare row by row.

Geometric means of the cycle ratios, by group, and of two instruction
ratios:

| group | op | htj2k / generic | htj2k / SIMD | native / generic | simd / generic | simd / SIMD | simd / htj2k | instructions, htj2k / generic | instructions, simd / SIMD |
|---|---|--:|--:|--:|--:|--:|--:|--:|--:|
| Strips, reversible, 16-bit (6) | decode | 0.94 | 1.78 | 0.75 | 0.57 | 1.08 | 0.61 | 0.94 | 1.09 |
|  | encode | 0.72 | 2.33 | 0.64 | 0.34 | 1.08 | 0.47 | 0.65 | 1.00 |
| Strips, reversible, 32-bit (6) | decode | 1.05 | 1.16 | 0.88 | 0.91 | 1.00 | 0.87 | 0.98 | 1.04 |
|  | encode | 0.57 | 0.61 | 0.53 | 0.62 | 0.66 | 1.08 | 0.57 | 0.55 |
| Strips, irreversible (6) | decode | 0.88 | 2.69 | 0.82 | 0.32 | 0.97 | 0.36 | 1.01 | 1.55 |
|  | encode | 0.88 | 2.49 | 0.80 | 0.35 | 0.99 | 0.40 | 0.98 | 1.40 |
| Strips, noise (2) | decode | 1.04 | 1.76 | 0.77 | 0.69 | 1.16 | 0.66 | 0.82 | 0.93 |
|  | encode | 0.79 | 1.03 | 0.72 | 0.58 | 0.76 | 0.74 | 0.58 | 0.73 |
| 3840x2160, reversible (1) | decode | 0.90 | 1.75 | 0.82 | 0.50 | 0.97 | 0.56 | 0.97 | 1.06 |
|  | encode | 0.76 | 2.30 | 0.67 | 0.34 | 1.03 | 0.45 | 0.70 | 1.02 |
| 3840x2160, irreversible (2) | decode | 1.16 | 2.83 | 0.80 | 0.50 | 1.21 | 0.43 | 0.90 | 0.99 |
|  | encode | 1.00 | 2.45 | 0.86 | 0.45 | 1.10 | 0.45 | 0.96 | 0.98 |

### Decode, all configurations

Cycles per sample (cycles:u), and htj2k's ratios:

| configuration | bits/sample | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k simd | htj2k / generic | htj2k / SIMD | native / generic | simd / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 13.43 | 30.4 | 16.4 | 28.1 | 23.4 | 19.1 | 0.93 | 1.71 | 0.77 | 1.16 | 7.0% |
| s16_3c_rev_8192x32 | 13.79 | 32.5 | 20.6 | 47.4 | 27.0 | 19.6 | 1.46 | 2.30 | 0.83 | 0.95 | 21.5% |
| s16_4c_rev_8192x32 | 13.69 | 33.3 | 17.9 | 30.5 | 30.0 | 20.1 | 0.92 | 1.71 | 0.90 | 1.13 | 6.5% |
| s32_1c_rev_8192x32 | 29.62 | 51.8 | 44.3 | 59.9 | 42.2 | 50.7 | 1.16 | 1.35 | 0.81 | 1.14 | 6.1% |
| s32_3c_rev_8192x32 | 29.78 | 54.9 | 65.6 | 62.6 | 70.5 | 75.0 | 1.14 | 0.96 | 1.28 | 1.14 | 16.4% |
| s32_4c_rev_8192x32 | 29.72 | 98.0 | 77.4 | 78.7 | 71.5 | 57.9 | 0.80 | 1.02 | 0.73 | 0.75 | 19.4% |
| s16_1c_rev_2048x256 | 12.53 | 27.5 | 15.4 | 26.5 | 23.5 | 17.3 | 0.96 | 1.72 | 0.85 | 1.13 | 2.4% |
| s16_3c_rev_2048x256 | 12.75 | 31.9 | 16.6 | 32.0 | 21.3 | 26.1 | 1.00 | 1.92 | 0.67 | 1.57 | 15.2% |
| s16_4c_rev_2048x256 | 12.65 | 53.6 | 22.1 | 31.1 | 28.5 | 16.2 | 0.58 | 1.41 | 0.53 | 0.73 | 15.2% |
| s32_1c_rev_2048x256 | 28.51 | 36.1 | 42.9 | 50.1 | 42.1 | 48.9 | 1.39 | 1.17 | 1.17 | 1.14 | 17.8% |
| s32_3c_rev_2048x256 | 28.73 | 65.8 | 45.2 | 68.0 | 44.1 | 40.5 | 1.03 | 1.51 | 0.67 | 0.90 | 14.8% |
| s32_4c_rev_2048x256 | 28.69 | 84.8 | 71.9 | 76.0 | 68.1 | 73.6 | 0.90 | 1.06 | 0.80 | 1.02 | 7.7% |
| s16_3c_ict_q90_8192x32 | 1.31 | 73.4 | 28.1 | 63.5 | 48.2 | 23.3 | 0.87 | 2.26 | 0.66 | 0.83 | 5.6% |
| s32_3c_ict_q90_8192x32 | 1.25 | 64.8 | 27.4 | 55.0 | 56.1 | 25.7 | 0.85 | 2.00 | 0.87 | 0.94 | 10.1% |
| s16_3c_ict_q90_2048x256 | 0.60 | 53.9 | 14.9 | 53.9 | 58.2 | 11.2 | 1.00 | 3.61 | 1.08 | 0.75 | 13.1% |
| s32_3c_ict_q90_2048x256 | 0.59 | 50.0 | 19.1 | 45.2 | 42.0 | 18.6 | 0.90 | 2.37 | 0.84 | 0.97 | 12.5% |
| s16_3c_ict_qs002_8192x32 | 5.29 | 67.5 | 21.9 | 71.2 | 59.8 | 24.0 | 1.06 | 3.25 | 0.89 | 1.09 | 10.3% |
| s32_3c_ict_qs002_8192x32 | 5.32 | 99.1 | 22.1 | 66.7 | 65.9 | 29.6 | 0.67 | 3.03 | 0.67 | 1.34 | 17.4% |
| s16_3c_rev_noise_8192x32 | 17.46 | 51.7 | 20.9 | 49.4 | 40.1 | 27.1 | 0.96 | 2.36 | 0.77 | 1.30 | 11.0% |
| s32_3c_rev_noise_8192x32 | 33.47 | 53.0 | 46.1 | 60.3 | 40.5 | 47.8 | 1.14 | 1.31 | 0.76 | 1.04 | 17.1% |
| u12_3c_rev_3840x2160 | 8.14 | 31.8 | 16.4 | 28.6 | 26.2 | 16.0 | 0.90 | 1.75 | 0.82 | 0.97 | 6.3% |
| u12_3c_ict_q90_3840x2160 | 0.04 | 15.3 | 6.1 | 19.7 | 11.3 | 8.6 | 1.29 | 3.26 | 0.74 | 1.41 | 18.9% |
| u12_3c_ict_qs002_3840x2160 | 3.82 | 38.4 | 16.3 | 40.1 | 33.2 | 16.8 | 1.05 | 2.46 | 0.86 | 1.03 | 4.1% |

Instructions per sample (instructions:u):

| configuration | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k simd | htj2k / generic | htj2k / SIMD | simd / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 101.8 | 46.4 | 98.0 | 80.9 | 51.0 | 0.96 | 2.11 | 1.10 | 0.00% |
| s16_3c_rev_8192x32 | 107.2 | 47.0 | 99.6 | 81.8 | 51.8 | 0.93 | 2.12 | 1.10 | 0.00% |
| s16_4c_rev_8192x32 | 105.8 | 46.9 | 99.2 | 81.6 | 51.6 | 0.94 | 2.12 | 1.10 | 0.00% |
| s32_1c_rev_8192x32 | 184.0 | 155.3 | 177.4 | 143.5 | 159.3 | 0.96 | 1.14 | 1.03 | 0.00% |
| s32_3c_rev_8192x32 | 182.7 | 155.8 | 175.3 | 142.7 | 158.3 | 0.96 | 1.13 | 1.02 | 0.00% |
| s32_4c_rev_8192x32 | 183.0 | 155.6 | 176.0 | 143.0 | 158.6 | 0.96 | 1.13 | 1.02 | 0.00% |
| s16_1c_rev_2048x256 | 99.1 | 44.2 | 95.4 | 78.6 | 47.6 | 0.96 | 2.16 | 1.08 | 0.00% |
| s16_3c_rev_2048x256 | 104.2 | 44.9 | 96.9 | 79.4 | 48.4 | 0.93 | 2.16 | 1.08 | 0.00% |
| s16_4c_rev_2048x256 | 102.8 | 44.7 | 96.5 | 79.1 | 48.1 | 0.94 | 2.16 | 1.08 | 0.00% |
| s32_1c_rev_2048x256 | 179.9 | 151.1 | 181.1 | 146.4 | 162.9 | 1.01 | 1.20 | 1.08 | 0.00% |
| s32_3c_rev_2048x256 | 178.8 | 151.8 | 178.7 | 145.4 | 161.6 | 1.00 | 1.18 | 1.06 | 0.00% |
| s32_4c_rev_2048x256 | 179.1 | 151.6 | 179.1 | 145.6 | 161.8 | 1.00 | 1.18 | 1.07 | 0.00% |
| s16_3c_ict_q90_8192x32 | 121.7 | 38.3 | 125.3 | 102.3 | 59.2 | 1.03 | 3.28 | 1.55 | 0.00% |
| s32_3c_ict_q90_8192x32 | 121.1 | 37.8 | 124.9 | 102.0 | 58.9 | 1.03 | 3.30 | 1.56 | 0.00% |
| s16_3c_ict_q90_2048x256 | 104.8 | 27.9 | 104.3 | 83.7 | 46.9 | 1.00 | 3.74 | 1.68 | 0.00% |
| s32_3c_ict_q90_2048x256 | 104.8 | 27.9 | 104.3 | 83.7 | 47.0 | 1.00 | 3.74 | 1.68 | 0.00% |
| s16_3c_ict_qs002_8192x32 | 148.9 | 44.7 | 147.7 | 120.7 | 63.7 | 0.99 | 3.30 | 1.43 | 0.00% |
| s32_3c_ict_qs002_8192x32 | 149.0 | 44.6 | 147.8 | 120.7 | 63.7 | 0.99 | 3.31 | 1.43 | 0.00% |
| s16_3c_rev_noise_8192x32 | 114.7 | 48.2 | 102.6 | 84.7 | 54.3 | 0.89 | 2.13 | 1.12 | 0.00% |
| s32_3c_rev_noise_8192x32 | 193.1 | 166.2 | 143.7 | 116.0 | 126.7 | 0.74 | 0.86 | 0.76 | 0.00% |
| u12_3c_rev_3840x2160 | 93.0 | 37.4 | 90.6 | 75.0 | 39.8 | 0.97 | 2.42 | 1.06 | 0.00% |
| u12_3c_ict_q90_3840x2160 | 52.3 | 16.4 | 43.7 | 27.0 | 16.0 | 0.84 | 2.67 | 0.98 | 0.00% |
| u12_3c_ict_qs002_3840x2160 | 108.1 | 40.8 | 104.8 | 80.1 | 41.1 | 0.97 | 2.57 | 1.01 | 0.00% |

### Encode, all configurations

Cycles per sample (cycles:u), and htj2k's ratios:

| configuration | bits/sample | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k simd | htj2k / generic | htj2k / SIMD | native / generic | simd / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 13.43 | 72.1 | 20.7 | 55.5 | 42.7 | 24.7 | 0.77 | 2.68 | 0.59 | 1.19 | 5.2% |
| s16_3c_rev_8192x32 | 13.79 | 83.7 | 21.4 | 50.3 | 45.6 | 24.6 | 0.60 | 2.34 | 0.55 | 1.15 | 11.2% |
| s16_4c_rev_8192x32 | 13.69 | 74.4 | 22.4 | 56.4 | 41.7 | 25.0 | 0.76 | 2.52 | 0.56 | 1.12 | 6.5% |
| s32_1c_rev_8192x32 | 29.62 | 112.3 | 98.2 | 59.0 | 51.6 | 54.6 | 0.53 | 0.60 | 0.46 | 0.56 | 11.4% |
| s32_3c_rev_8192x32 | 29.78 | 141.9 | 68.3 | 62.8 | 71.2 | 61.7 | 0.44 | 0.92 | 0.50 | 0.90 | 4.9% |
| s32_4c_rev_8192x32 | 29.72 | 115.7 | 105.3 | 66.5 | 60.7 | 65.8 | 0.58 | 0.63 | 0.52 | 0.63 | 3.7% |
| s16_1c_rev_2048x256 | 12.53 | 68.9 | 23.0 | 46.7 | 41.4 | 21.3 | 0.68 | 2.03 | 0.60 | 0.92 | 9.0% |
| s16_3c_rev_2048x256 | 12.75 | 77.0 | 29.9 | 41.2 | 50.9 | 22.8 | 0.53 | 1.38 | 0.66 | 0.76 | 12.9% |
| s16_4c_rev_2048x256 | 12.65 | 69.6 | 21.9 | 78.8 | 66.3 | 32.9 | 1.13 | 3.60 | 0.95 | 1.50 | 5.2% |
| s32_1c_rev_2048x256 | 28.51 | 101.0 | 97.3 | 59.5 | 47.8 | 55.1 | 0.59 | 0.61 | 0.47 | 0.57 | 2.7% |
| s32_3c_rev_2048x256 | 28.73 | 135.5 | 128.9 | 49.0 | 53.7 | 98.3 | 0.36 | 0.38 | 0.40 | 0.76 | 22.3% |
| s32_4c_rev_2048x256 | 28.69 | 74.9 | 147.5 | 93.8 | 74.4 | 89.9 | 1.25 | 0.64 | 0.99 | 0.61 | 12.4% |
| s16_3c_ict_q90_8192x32 | 1.31 | 115.2 | 46.1 | 95.2 | 85.8 | 44.3 | 0.83 | 2.06 | 0.75 | 0.96 | 3.1% |
| s32_3c_ict_q90_8192x32 | 1.25 | 95.6 | 32.9 | 82.5 | 71.8 | 35.5 | 0.86 | 2.51 | 0.75 | 1.08 | 5.5% |
| s16_3c_ict_q90_2048x256 | 0.60 | 63.8 | 26.6 | 63.3 | 51.7 | 23.4 | 0.99 | 2.38 | 0.81 | 0.88 | 5.5% |
| s32_3c_ict_q90_2048x256 | 0.59 | 100.5 | 35.3 | 80.0 | 77.5 | 33.0 | 0.80 | 2.27 | 0.77 | 0.93 | 11.0% |
| s16_3c_ict_qs002_8192x32 | 5.29 | 112.2 | 35.8 | 92.4 | 85.5 | 32.2 | 0.82 | 2.58 | 0.76 | 0.90 | 5.4% |
| s32_3c_ict_qs002_8192x32 | 5.32 | 129.8 | 37.8 | 126.7 | 123.2 | 47.5 | 0.98 | 3.35 | 0.95 | 1.26 | 8.9% |
| s16_3c_rev_noise_8192x32 | 17.46 | 47.1 | 28.8 | 47.3 | 43.9 | 27.3 | 1.00 | 1.65 | 0.93 | 0.95 | 15.0% |
| s32_3c_rev_noise_8192x32 | 33.47 | 105.8 | 102.1 | 65.3 | 58.6 | 62.1 | 0.62 | 0.64 | 0.55 | 0.61 | 5.0% |
| u12_3c_rev_3840x2160 | 8.14 | 64.9 | 21.3 | 49.0 | 43.8 | 21.9 | 0.76 | 2.30 | 0.67 | 1.03 | 4.5% |
| u12_3c_ict_q90_3840x2160 | 0.04 | 11.3 | 7.6 | 14.9 | 12.8 | 9.0 | 1.32 | 1.96 | 1.13 | 1.18 | 4.3% |
| u12_3c_ict_qs002_3840x2160 | 3.82 | 89.6 | 22.3 | 68.2 | 58.7 | 22.9 | 0.76 | 3.05 | 0.66 | 1.03 | 8.3% |

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

The scalar build's profiles have the same shape as OpenJPH generic's:
`decode_codeblock32` takes 71.0% of the `s16_3c_rev_8192x32` decode
(`ojph_decode_codeblock32` 72.3%), `encoder::encode` 81.6% of its
encode (`ojph_encode_codeblock32` 89.9%), and the NLT type 4
conversions about half of the Qfactor 90 strip decode on both sides
(52.1% against 56.9%).

**Decode, `s16_3c_rev_8192x32`** (htj2k simd 19.6, OpenJPH SIMD 20.6
cycles/sample)

| htj2k simd | % | OpenJPH SIMD | % |
|---|--:|---|--:|
| `coding::decoder_avx2::rows` | 35.3 | `decode_cb_step2_32bit` | 41.2 |
| `coding::vlc_avx2::decode_vlc_mel32` | 32.1 | `decode_cb_step1_vlc` | 31.5 |
| `memset` | 4.3 | `avx2_rev_horz_syn` | 4.3 |
| `vectorised::rev_horz_syn` | 4.1 | `memmove` | 4.3 |
| `vectorised::rev_vert_step` | 3.2 | `avx2_rev_vert_step` | 3.1 |

`decoder_avx2::rows` is the MagSgn step (`row_u_q`, 2.4%, computes its
`U_q`), `vlc_avx2::decode_vlc_mel32` the VLC and MEL step: the two
halves of the cleanup pass, as in OpenJPH's AVX2 decoder.

**Encode, `s16_3c_rev_8192x32`** (24.6 against 21.4)

| htj2k simd | % | OpenJPH SIMD | % |
|---|--:|---|--:|
| `coding::encoder_avx2::proc_ms_encode` | 37.1 | `ojph_encode_codeblock_avx2` | 34.9 |
| `coding::encoder_avx2::encode32` | 35.4 | `proc_ms_encode` | 33.2 |
| `vectorised::rev_horz_ana` | 4.2 | `proc_pixel` | 9.7 |
| `memmove` | 3.9 | `memmove` | 4.7 |
| `vectorised::rev_vert_step` | 3.1 | `avx2_rev_horz_ana` | 3.1 |

(htj2k inlines `proc_pixel` into `encode32`.)

**Decode, `s16_3c_ict_q90_8192x32`** (23.3 against 28.1)

| htj2k simd | % | OpenJPH SIMD | % |
|---|--:|---|--:|
| `vectorised::irv_convert_to_integer_nlt2or4` | 28.7 | `avx2_irv_convert_to_integer_nlt` | 29.0 |
| `coding::vlc_avx2::decode_vlc_mel32` | 27.5 | `decode_cb_step1_vlc` | 27.1 |
| `coding::decoder_avx2::rows` | 7.8 | `decode_cb_step2_16bit` | 12.4 |
| `vectorised::irv_horz_syn` | 6.9 | `avx_irv_horz_syn` | 7.8 |
| `vectorised::irv_vert_step` | 5.3 | `avx_irv_vert_step` | 5.6 |

Both decode these blocks in 16-bit MagSgn lanes.

**Encode, `s16_3c_ict_q90_8192x32`** (44.3 against 46.1)

| htj2k simd | % | OpenJPH SIMD | % |
|---|--:|---|--:|
| `vectorised::irv_convert_to_float_nlt2or4_precise_1step` | 25.7 | `avx2_irv_convert_to_float_nlt` | 39.1 |
| `coding::encoder_avx2::encode32` | 24.1 | `ojph_encode_codeblock_avx2` | 22.1 |
| `coding::encoder_avx2::proc_ms_encode` | 20.1 | `proc_ms_encode` | 13.0 |
| `vectorised::irv_horz_ana` | 5.8 | `proc_pixel` | 6.6 |
| `vectorised::irv_vert_step` | 4.7 | `avx_irv_horz_ana` | 5.1 |

**32-bit reversible samples** (`s32_3c_rev_8192x32`) take the 64-bit
block coders, which neither side vectorises: `decode_codeblock64` is
78.0% of the simd decode, `ojph_decode_codeblock64` 76.3% of OpenJPH
SIMD's, and the encoders 75.6% and 89.3%.

**3840x2160**: the reversible decode is led by
`vlc_avx2::decode_vlc_mel32` (16.2%) and the two `decoder_avx2::rows`
versions, 16-bit and 32-bit lanes (8.6% and 4.5%), against OpenJPH
SIMD's `decode_cb_step1_vlc` (14.2%), `decode_cb_step2_16bit` (8.8%)
and `decode_cb_step2_32bit` (4.4%). In the Qfactor 90 decode, where
almost every block is empty, the scalar build spends 15.9% in
`irv_convert_to_integer`. With the feature that is the intrinsics
kernel `vectorised::avx2::irv_convert_to_integer` (2.6%, OpenJPH
SIMD's `avx2_irv_convert_to_integer` 2.4%), and the wavelet leads, as
on OpenJPH SIMD: `irv_horz_syn` 6.5% and `irv_vert_step` 5.8%,
against 4.9% and 5.8%.

## Where the gaps are

**The scalar build against OpenJPH generic.** htj2k executes fewer
instructions than the C++ it is ported from on most rows: 0.55 to 0.70
times on the reversible encodes, 0.74 to 1.01 on the reversible
decodes, and 1.00 to 1.05 on the Qfactor 90 strips, where the float
conversions dominate. In cycles, which on this run are noisy (see
Variation), it is 0.36 to 1.46 times OpenJPH generic on the strips.
The native build is at or below OpenJPH generic on all but four rows
(1.08 to 1.28, with the same instructions as the rows around them).

**The `simd` build against OpenJPH SIMD.** With the feature, htj2k
executes 0.37 to 0.90 times the scalar build's instructions. On the
16-bit and 12-bit rows it executes 0.95 to 1.12 times OpenJPH SIMD's
instructions on every reversible and noise row, encode and decode, and
on the 3840x2160 irreversible ones; only the irreversible strips are
further off (1.43 to 1.68 to decode, 1.34 to 1.48 to encode). In
cycles the 16-bit and 12-bit rows take 0.73 to 1.57 times OpenJPH
SIMD's to decode and 0.76 to 1.50 times to encode. What is left, measured by symbol with perf at N and 2N
iterations (instructions per sample; the split between the symbols of
one side moves by one or two instructions from run to run, the totals
do not):

1. **The NLT type 4 conversions.** These are table lookups. htj2k's
   versions are plain loops that the compiler vectorises around scalar
   lookups. They execute four to five times OpenJPH's instructions
   (21.6 a sample against 4.0 on the `s16_3c_ict_q90_8192x32` decode,
   about 24 against 6 on encoding), but take about as many cycles,
   since OpenJPH waits on its gathers. This is most of what separates
   the irreversible strips: 59.2 instructions a sample against 38.1 to
   decode, at 0.83 times OpenJPH SIMD's cycles on this row.
2. **The block decoder.** htj2k's cleanup pass now has OpenJPH's AVX2
   structure: the VLC and MEL step with the unstuffed VLC buffer and
   the wide UVLC table, and the MagSgn step in 16-bit or 32-bit lanes.
   On `s16_3c_rev_8192x32` the two steps take about 42 instructions a
   sample against about 37 for OpenJPH's, of a whole decode of 52.0
   against 46.6; on the 3840x2160 reversible decode about 31 against
   28, of 39.8 against 37.1. What remains is mostly in the VLC step:
   MEL decoding stays the scalar reader's, out of line, with its state
   in memory, and the loop keeps a few more values on the stack than
   OpenJPH's. Pulling MEL's runs into registers batch by batch was
   tried and executed more instructions, not fewer. The MagSgn loops
   are at OpenJPH's instruction count per pair of quads.
3. **Zeroing and copying.** `memset` takes 2.3 instructions a sample
   on the `s16_3c_rev_8192x32` decode and 2.5 on the Qfactor 90 one
   (1.9 to 4.3% of the strip decode profiles) and is in none of
   OpenJPH's; `Codeblock::pull_line` is a copy that OpenJPH
   avoids.

For 32-bit reversible samples neither side has SIMD block coders,
because they take the 64-bit paths, so the feature changes only the
wavelet, colour and line conversion kernels there (0.88 to 0.90 times
the scalar build's instructions to decode). htj2k simd encodes them
in 0.56 to 0.90 times OpenJPH SIMD's cycles and decodes them in 0.75
to 1.14 times.
