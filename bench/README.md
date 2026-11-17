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
  to 7% (on most rows the worst of the four is 3 to 5%), because other jobs use the core's other
  hardware thread and the shared L3. So a cycle ratio is good to about
  ±5 to 10%, and the instruction counts are the steadier guide.

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
* **System:** Linux 6.18, shared, with a load average from 8.6 to 17.2
  during the run (`uptime` before and after each of the 368 perf runs
  is in `log.txt`). The benchmark ran on CPU 21, whose core (with CPU
  5) was 18% busy when it was chosen.
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

Run of 2026-11-17 (`bench/results/`). Ratios above 1 mean htj2k takes
more. "± max" is the largest variation of the four harnesses on the
row. "native" is htj2k built with `-C target-cpu=native`; OpenJPH
generic was not, so the native / generic columns compare different
instruction sets (BMI, LZCNT, SSE4.1 rounding and AVX2 for the
compiler's own vectorisation, against the x86-64 baseline).

Geometric means of the cycle ratios, by group:

| group | op | htj2k / generic | htj2k / SIMD | native / generic | native / SIMD | instructions, htj2k / generic |
|---|---|--:|--:|--:|--:|--:|
| strips, reversible, 16-bit (6) | decode | 1.07 | 1.89 | 1.11 | 1.97 | 1.18 |
| | encode | 1.08 | 3.48 | 0.97 | 3.12 | 1.03 |
| strips, reversible, 32-bit (6) | decode | 1.12 | 1.32 | 0.95 | 1.12 | 1.19 |
| | encode | 1.10 | 1.15 | 0.96 | 1.01 | 1.06 |
| strips, irreversible (6) | decode | 1.29 | 3.81 | 0.99 | 2.91 | 1.30 |
| | encode | 1.10 | 3.21 | 0.94 | 2.74 | 1.26 |
| strips, noise (2) | decode | 1.11 | 1.70 | 1.06 | 1.61 | 1.18 |
| | encode | 1.16 | 1.88 | 0.97 | 1.58 | 1.05 |
| 3840x2160, reversible (1) | decode | 1.09 | 2.35 | 1.00 | 2.14 | 1.16 |
| | encode | 1.01 | 3.07 | 0.90 | 2.74 | 0.99 |
| 3840x2160, irreversible (2) | decode | 1.12 | 2.59 | 0.94 | 2.16 | 0.96 |
| | encode | 1.19 | 2.58 | 1.07 | 2.32 | 1.08 |

### Decode, all configurations

Cycles per sample (cycles:u), and htj2k's ratios:

| configuration | bits/sample | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k / generic | htj2k / SIMD | native / generic | native / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 13.43 | 29.1 | 18.5 | 35.7 | 34.3 | 1.23 | 1.93 | 1.18 | 1.86 | 4.5% |
| s16_3c_rev_8192x32 | 13.79 | 34.1 | 20.7 | 33.7 | 35.5 | 0.99 | 1.63 | 1.04 | 1.72 | 6.3% |
| s16_4c_rev_8192x32 | 13.69 | 36.6 | 18.2 | 36.4 | 37.7 | 1.00 | 2.00 | 1.03 | 2.07 | 3.4% |
| s32_1c_rev_8192x32 | 29.62 | 56.0 | 46.2 | 63.3 | 54.0 | 1.13 | 1.37 | 0.97 | 1.17 | 3.4% |
| s32_3c_rev_8192x32 | 29.78 | 57.0 | 50.4 | 69.4 | 55.3 | 1.22 | 1.38 | 0.97 | 1.10 | 3.3% |
| s32_4c_rev_8192x32 | 29.72 | 60.5 | 53.4 | 66.1 | 58.7 | 1.09 | 1.24 | 0.97 | 1.10 | 3.1% |
| s16_1c_rev_2048x256 | 12.53 | 29.9 | 16.5 | 34.6 | 37.0 | 1.16 | 2.10 | 1.23 | 2.24 | 5.7% |
| s16_3c_rev_2048x256 | 12.75 | 34.3 | 18.8 | 34.9 | 35.6 | 1.02 | 1.86 | 1.04 | 1.89 | 4.6% |
| s16_4c_rev_2048x256 | 12.65 | 32.5 | 18.1 | 33.6 | 37.7 | 1.03 | 1.86 | 1.16 | 2.08 | 4.2% |
| s32_1c_rev_2048x256 | 28.51 | 53.6 | 44.0 | 59.0 | 49.3 | 1.10 | 1.34 | 0.92 | 1.12 | 4.3% |
| s32_3c_rev_2048x256 | 28.73 | 56.2 | 45.4 | 60.0 | 52.3 | 1.07 | 1.32 | 0.93 | 1.15 | 3.9% |
| s32_4c_rev_2048x256 | 28.69 | 52.8 | 45.2 | 58.7 | 49.7 | 1.11 | 1.30 | 0.94 | 1.10 | 2.2% |
| s16_3c_ict_q90_8192x32 | 1.31 | 52.5 | 19.9 | 71.2 | 54.5 | 1.36 | 3.59 | 1.04 | 2.74 | 3.9% |
| s32_3c_ict_q90_8192x32 | 1.25 | 53.7 | 21.2 | 68.3 | 53.8 | 1.27 | 3.23 | 1.00 | 2.54 | 4.3% |
| s16_3c_ict_q90_2048x256 | 0.60 | 48.4 | 14.3 | 65.5 | 46.7 | 1.35 | 4.56 | 0.96 | 3.25 | 4.8% |
| s32_3c_ict_q90_2048x256 | 0.59 | 46.9 | 14.4 | 61.4 | 45.4 | 1.31 | 4.27 | 0.97 | 3.15 | 4.1% |
| s16_3c_ict_qs002_8192x32 | 5.29 | 65.4 | 20.5 | 81.6 | 64.4 | 1.25 | 3.97 | 0.99 | 3.14 | 2.8% |
| s32_3c_ict_qs002_8192x32 | 5.32 | 65.9 | 23.5 | 80.6 | 64.0 | 1.22 | 3.43 | 0.97 | 2.72 | 5.0% |
| s16_3c_rev_noise_8192x32 | 17.46 | 34.9 | 18.3 | 38.7 | 39.8 | 1.11 | 2.11 | 1.14 | 2.17 | 4.2% |
| s32_3c_rev_noise_8192x32 | 33.47 | 57.9 | 47.5 | 64.6 | 56.9 | 1.12 | 1.36 | 0.98 | 1.20 | 2.8% |
| u12_3c_rev_3840x2160 | 8.14 | 32.9 | 15.3 | 36.0 | 32.8 | 1.09 | 2.35 | 1.00 | 2.14 | 6.7% |
| u12_3c_ict_q90_3840x2160 | 0.04 | 14.1 | 7.0 | 16.5 | 13.6 | 1.17 | 2.36 | 0.96 | 1.94 | 5.1% |
| u12_3c_ict_qs002_3840x2160 | 3.82 | 39.5 | 15.0 | 42.6 | 36.1 | 1.08 | 2.85 | 0.91 | 2.41 | 5.4% |

Instructions per sample (instructions:u):

| configuration | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k / generic | htj2k / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 101.8 | 46.4 | 122.9 | 102.2 | 1.21 | 2.65 | 0.00% |
| s16_3c_rev_8192x32 | 107.2 | 47.0 | 125.0 | 103.7 | 1.17 | 2.66 | 0.00% |
| s16_4c_rev_8192x32 | 105.8 | 46.9 | 124.5 | 103.3 | 1.18 | 2.66 | 0.00% |
| s32_1c_rev_8192x32 | 184.0 | 155.3 | 217.5 | 174.7 | 1.18 | 1.40 | 0.00% |
| s32_3c_rev_8192x32 | 182.7 | 155.8 | 217.7 | 175.9 | 1.19 | 1.40 | 0.00% |
| s32_4c_rev_8192x32 | 183.0 | 155.6 | 217.6 | 175.6 | 1.19 | 1.40 | 0.00% |
| s16_1c_rev_2048x256 | 99.1 | 44.2 | 119.4 | 99.1 | 1.21 | 2.70 | 0.00% |
| s16_3c_rev_2048x256 | 104.2 | 44.9 | 121.2 | 100.2 | 1.16 | 2.70 | 0.00% |
| s16_4c_rev_2048x256 | 102.8 | 44.7 | 120.7 | 99.8 | 1.17 | 2.70 | 0.00% |
| s32_1c_rev_2048x256 | 179.9 | 151.1 | 212.1 | 170.2 | 1.18 | 1.40 | 0.00% |
| s32_3c_rev_2048x256 | 178.8 | 151.8 | 212.4 | 171.5 | 1.19 | 1.40 | 0.00% |
| s32_4c_rev_2048x256 | 179.1 | 151.6 | 212.4 | 171.2 | 1.19 | 1.40 | 0.00% |
| s16_3c_ict_q90_8192x32 | 121.7 | 38.3 | 160.5 | 108.4 | 1.32 | 4.20 | 0.00% |
| s32_3c_ict_q90_8192x32 | 121.1 | 37.8 | 160.0 | 108.0 | 1.32 | 4.23 | 0.00% |
| s16_3c_ict_q90_2048x256 | 104.8 | 27.9 | 137.7 | 88.1 | 1.31 | 4.94 | 0.00% |
| s32_3c_ict_q90_2048x256 | 104.8 | 27.9 | 137.8 | 88.1 | 1.31 | 4.94 | 0.00% |
| s16_3c_ict_qs002_8192x32 | 148.9 | 44.7 | 186.5 | 131.1 | 1.25 | 4.17 | 0.00% |
| s32_3c_ict_qs002_8192x32 | 149.0 | 44.6 | 186.6 | 131.2 | 1.25 | 4.18 | 0.00% |
| s16_3c_rev_noise_8192x32 | 114.7 | 48.2 | 133.5 | 111.9 | 1.16 | 2.77 | 0.00% |
| s32_3c_rev_noise_8192x32 | 193.1 | 166.2 | 230.5 | 186.3 | 1.19 | 1.39 | 0.00% |
| u12_3c_rev_3840x2160 | 93.0 | 37.4 | 108.2 | 89.1 | 1.16 | 2.89 | 0.00% |
| u12_3c_ict_q90_3840x2160 | 52.3 | 16.4 | 46.4 | 28.8 | 0.89 | 2.83 | 0.00% |
| u12_3c_ict_qs002_3840x2160 | 108.1 | 40.8 | 112.7 | 87.4 | 1.04 | 2.76 | 0.00% |

### Encode, all configurations

Cycles per sample (cycles:u), and htj2k's ratios:

| configuration | bits/sample | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k / generic | htj2k / SIMD | native / generic | native / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 13.43 | 76.4 | 22.4 | 83.7 | 71.1 | 1.10 | 3.74 | 0.93 | 3.18 | 3.2% |
| s16_3c_rev_8192x32 | 13.79 | 78.2 | 22.8 | 83.0 | 74.7 | 1.06 | 3.64 | 0.95 | 3.28 | 4.0% |
| s16_4c_rev_8192x32 | 13.69 | 76.7 | 26.4 | 81.6 | 77.8 | 1.06 | 3.09 | 1.02 | 2.95 | 3.7% |
| s32_1c_rev_8192x32 | 29.62 | 110.7 | 99.0 | 122.8 | 98.3 | 1.11 | 1.24 | 0.89 | 0.99 | 6.0% |
| s32_3c_rev_8192x32 | 29.78 | 109.3 | 104.5 | 121.2 | 107.8 | 1.11 | 1.16 | 0.99 | 1.03 | 2.8% |
| s32_4c_rev_8192x32 | 29.72 | 112.6 | 105.7 | 119.5 | 110.6 | 1.06 | 1.13 | 0.98 | 1.05 | 2.1% |
| s16_1c_rev_2048x256 | 12.53 | 69.4 | 19.9 | 77.9 | 65.7 | 1.12 | 3.92 | 0.95 | 3.31 | 3.6% |
| s16_3c_rev_2048x256 | 12.75 | 71.3 | 23.3 | 75.4 | 70.5 | 1.06 | 3.24 | 0.99 | 3.03 | 2.7% |
| s16_4c_rev_2048x256 | 12.65 | 70.2 | 23.0 | 76.6 | 68.4 | 1.09 | 3.34 | 0.97 | 2.98 | 4.3% |
| s32_1c_rev_2048x256 | 28.51 | 100.8 | 97.2 | 108.3 | 95.8 | 1.08 | 1.12 | 0.95 | 0.99 | 2.2% |
| s32_3c_rev_2048x256 | 28.73 | 101.3 | 100.9 | 116.9 | 101.0 | 1.15 | 1.16 | 1.00 | 1.00 | 3.5% |
| s32_4c_rev_2048x256 | 28.69 | 105.8 | 100.3 | 113.3 | 98.9 | 1.07 | 1.13 | 0.93 | 0.99 | 3.0% |
| s16_3c_ict_q90_8192x32 | 1.31 | 82.1 | 32.4 | 91.1 | 80.6 | 1.11 | 2.82 | 0.98 | 2.49 | 4.0% |
| s32_3c_ict_q90_8192x32 | 1.25 | 91.5 | 32.9 | 102.3 | 88.5 | 1.12 | 3.11 | 0.97 | 2.69 | 2.9% |
| s16_3c_ict_q90_2048x256 | 0.60 | 66.3 | 27.1 | 71.4 | 62.1 | 1.08 | 2.63 | 0.94 | 2.29 | 3.4% |
| s32_3c_ict_q90_2048x256 | 0.59 | 79.7 | 25.3 | 88.9 | 74.5 | 1.12 | 3.52 | 0.93 | 2.95 | 3.9% |
| s16_3c_ict_qs002_8192x32 | 5.29 | 118.5 | 37.7 | 130.6 | 111.1 | 1.10 | 3.46 | 0.94 | 2.95 | 2.4% |
| s32_3c_ict_qs002_8192x32 | 5.32 | 129.4 | 37.0 | 142.6 | 116.6 | 1.10 | 3.86 | 0.90 | 3.16 | 2.0% |
| s16_3c_rev_noise_8192x32 | 17.46 | 71.9 | 28.0 | 82.4 | 69.8 | 1.15 | 2.95 | 0.97 | 2.50 | 2.6% |
| s32_3c_rev_noise_8192x32 | 33.47 | 106.1 | 103.5 | 124.2 | 103.2 | 1.17 | 1.20 | 0.97 | 1.00 | 3.4% |
| u12_3c_rev_3840x2160 | 8.14 | 64.6 | 21.2 | 65.3 | 58.3 | 1.01 | 3.07 | 0.90 | 2.74 | 4.4% |
| u12_3c_ict_q90_3840x2160 | 0.04 | 12.7 | 9.0 | 17.1 | 14.3 | 1.35 | 1.90 | 1.13 | 1.59 | 4.4% |
| u12_3c_ict_qs002_3840x2160 | 3.82 | 76.8 | 23.1 | 80.4 | 77.8 | 1.05 | 3.48 | 1.01 | 3.37 | 2.3% |

Instructions per sample (instructions:u):

| configuration | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native | htj2k / generic | htj2k / SIMD | ± max |
|---|--:|--:|--:|--:|--:|--:|--:|
| s16_1c_rev_8192x32 | 190.7 | 61.3 | 198.8 | 169.1 | 1.04 | 3.24 | 0.00% |
| s16_3c_rev_8192x32 | 196.6 | 62.0 | 202.0 | 171.2 | 1.03 | 3.25 | 0.00% |
| s16_4c_rev_8192x32 | 195.0 | 61.8 | 201.0 | 170.6 | 1.03 | 3.25 | 0.00% |
| s32_1c_rev_8192x32 | 278.6 | 254.0 | 294.2 | 243.8 | 1.06 | 1.16 | 0.00% |
| s32_3c_rev_8192x32 | 279.2 | 254.6 | 294.5 | 244.8 | 1.05 | 1.16 | 0.00% |
| s32_4c_rev_8192x32 | 279.0 | 254.4 | 294.3 | 244.5 | 1.05 | 1.16 | 0.00% |
| s16_1c_rev_2048x256 | 184.9 | 58.2 | 191.5 | 163.4 | 1.04 | 3.29 | 0.00% |
| s16_3c_rev_2048x256 | 190.3 | 58.7 | 193.8 | 164.8 | 1.02 | 3.30 | 0.00% |
| s16_4c_rev_2048x256 | 188.7 | 58.5 | 193.0 | 164.3 | 1.02 | 3.30 | 0.00% |
| s32_1c_rev_2048x256 | 270.6 | 245.9 | 286.7 | 237.4 | 1.06 | 1.17 | 0.00% |
| s32_3c_rev_2048x256 | 271.5 | 246.7 | 287.4 | 238.9 | 1.06 | 1.16 | 0.00% |
| s32_4c_rev_2048x256 | 271.3 | 246.6 | 287.4 | 238.7 | 1.06 | 1.17 | 0.00% |
| s16_3c_ict_q90_8192x32 | 154.1 | 57.3 | 200.0 | 144.1 | 1.30 | 3.49 | 0.00% |
| s32_3c_ict_q90_8192x32 | 152.9 | 57.0 | 199.2 | 143.4 | 1.30 | 3.50 | 0.00% |
| s16_3c_ict_q90_2048x256 | 124.2 | 40.5 | 164.1 | 110.7 | 1.32 | 4.05 | 0.00% |
| s32_3c_ict_q90_2048x256 | 124.2 | 40.5 | 164.5 | 111.1 | 1.32 | 4.07 | 0.00% |
| s16_3c_ict_qs002_8192x32 | 218.8 | 62.3 | 256.1 | 191.3 | 1.17 | 4.11 | 0.00% |
| s32_3c_ict_qs002_8192x32 | 219.0 | 62.0 | 256.5 | 191.8 | 1.17 | 4.13 | 0.00% |
| s16_3c_rev_noise_8192x32 | 210.5 | 66.6 | 218.9 | 185.4 | 1.04 | 3.29 | 0.00% |
| s32_3c_rev_noise_8192x32 | 293.1 | 268.4 | 311.1 | 259.3 | 1.06 | 1.16 | 0.00% |
| u12_3c_rev_3840x2160 | 170.0 | 53.3 | 169.0 | 145.3 | 0.99 | 3.17 | 0.00% |
| u12_3c_ict_q90_3840x2160 | 41.1 | 19.3 | 45.9 | 29.3 | 1.12 | 2.38 | 0.00% |
| u12_3c_ict_qs002_3840x2160 | 154.0 | 53.9 | 159.2 | 131.8 | 1.03 | 2.95 | 0.00% |

## Profiles

`perf record -e cycles:u` of htj2k and OpenJPH generic on five
configurations; `bench/results/profile.txt` has all of them. Self
time, top 5. The 3840x2160 profiles run only 4 iterations, so the
set-up (`synthesise`, `main` and, for decoding, the encode before the
loop) takes a large share of them; it is left out here.

**Decode, `s16_3c_rev_8192x32`** (htj2k 33.7, generic 34.1 cycles/sample)

| htj2k | % | OpenJPH generic | % |
|---|--:|---|--:|
| `coding::decoder32::decode_codeblock32` | 54.8 | `ojph_decode_codeblock32` | 73.4 |
| `coding::readers::Frwd::read` | 17.5 | `gen_rct_backward` | 3.8 |
| `transform::wavelet::rev_horz_syn` | 6.9 | `gen_rev_tx_from_cb32` | 3.8 |
| `codestream::subband::Subband::pull_line` | 3.6 | `gen_rev_horz_syn` | 3.5 |
| `transform::wavelet::rev_vert_step` | 2.3 | `memmove` | 3.3 |

**Decode, `s32_3c_rev_8192x32`** (69.4 against 57.0)

| htj2k | % | OpenJPH generic | % |
|---|--:|---|--:|
| `coding::decoder64::decode_codeblock64` | 74.7 | `ojph_decode_codeblock64` | 68.1 |
| `transform::wavelet::rev_horz_syn` | 6.9 | `gen_rev_tx_from_cb64` | 7.4 |
| `codestream::subband::Subband::pull_line` | 3.6 | `gen_rev_horz_syn` | 6.1 |
| `transform::wavelet::rev_vert_step` | 3.6 | `memmove` | 4.5 |
| `memset` | 2.0 | `gen_rev_vert_step` | 4.3 |

**Decode, `s16_3c_ict_q90_8192x32`** (71.2 against 52.5)

| htj2k | % | OpenJPH generic | % |
|---|--:|---|--:|
| `transform::colour::irv_convert_to_integer_nlt` | 51.1 | `gen_irv_convert_to_integer_nlt` | 54.3 |
| `coding::decoder32::decode_codeblock32` | 22.3 | `ojph_decode_codeblock32` | 24.5 |
| `floorf` (libm) | 5.7 | `gen_irv_horz_syn` | 4.2 |
| `transform::wavelet::irv_horz_syn` | 5.4 | `gen_ict_backward` | 3.2 |
| `transform::wavelet::irv_vert_step` | 2.5 | `gen_irv_vert_step` | 2.8 |

**Decode, `u12_3c_rev_3840x2160`**: `decode_codeblock32` and
`Frwd::read` again lead htj2k's profile (32.4% and 5.9%, with the
set-up encode in 23.4%), as `ojph_decode_codeblock32` (35.9%) leads
OpenJPH's.

**Encode, `s16_3c_rev_8192x32`** (83.0 against 78.2) and
**`s32_3c_rev_8192x32`** (121.2 against 109.3)

| htj2k, 16-bit | % | htj2k, 32-bit | % |
|---|--:|---|--:|
| `coding::encoder::encode` (32-bit) | 87.6 | `coding::encoder::encode` (64-bit) | 83.9 |
| `memmove` | 2.1 | `transform::wavelet::rev_horz_ana` | 3.1 |
| `transform::wavelet::rev_horz_ana` | 1.8 | `memmove` | 2.9 |
| `memset` | 1.8 | `memset` | 2.6 |
| | | `codestream::write::codeblock::Codeblock::push` | 2.1 |

OpenJPH generic spends 88.9% in `ojph_encode_codeblock32` and 84.8% in
`ojph_encode_codeblock64`, then the same transforms.

**Encode, `s16_3c_ict_q90_8192x32`** (91.1 against 82.1)

| htj2k | % | OpenJPH generic | % |
|---|--:|---|--:|
| `transform::colour::irv_convert_to_float_nlt` | 40.0 | `gen_irv_convert_to_float_nlt` | 48.5 |
| `coding::encoder::encode` | 36.8 | `ojph_encode_codeblock32` | 38.5 |
| `floorf` (libm) | 6.5 | `gen_irv_horz_ana` | 3.1 |
| `codestream::write::codeblock::Codeblock::push` | 5.4 | `gen_ict_forward` | 2.1 |
| `transform::wavelet::irv_horz_ana` | 3.3 | `gen_irv_vert_step` | 1.8 |

**Encode, `u12_3c_ict_q90_3840x2160`** (17.1 against 12.7), without
the set-up

| htj2k | % | OpenJPH generic | % |
|---|--:|---|--:|
| `codestream::write::codeblock::Codeblock::push` | 21.4 | `gen_irv_horz_ana` | 10.9 |
| `coding::encoder::encode` | 11.2 | `ojph_encode_codeblock32` | 10.6 |
| `transform::wavelet::irv_horz_ana` | 10.1 | `gen_ict_forward` | 6.8 |
| `transform::wavelet::irv_vert_step` | 6.5 | `gen_irv_vert_step` | 6.5 |
| `memmove` | 3.6 | `gen_irv_tx_to_cb32` | 5.5 |

## Where the gaps are

Against the C++ it is ported from, htj2k is close: 1.0 to 1.36 times
OpenJPH generic's cycles, about 1.1 on most rows, and the native build
is at or below OpenJPH generic on most rows (not on the 16-bit
reversible decode). The large gaps are to OpenJPH's SIMD build, where
OpenJPH has SIMD kernels.

1. **16-bit and 12-bit samples (32-bit code blocks) against OpenJPH
   SIMD: about 1.9x to decode and 3.5x to encode.** OpenJPH's AVX2
   block encoder and its SIMD block decoder, wavelet and colour kernels
   take the reversible images to 15 to 26 cycles per sample in either
   direction;
   htj2k needs 34 to 36 to decode and 65 to 84 to encode. The block
   encoder is 87% of htj2k's encode, so a vectorised block encoder is
   the largest single item.
2. **Irreversible coding: 1.9 to 4.6x against SIMD, and 1.3x against
   generic on the strips' decode.** The conversions between integers
   and floats with NLT type 4 are half of both sides' time on the
   Qfactor 90 strips, and OpenJPH SIMD vectorises them. On top of that,
   htj2k calls libm's `floorf` once per sample (6% of cycles), as the
   x86-64 baseline has no rounding instruction. With
   `target-cpu=native` (SSE4.1) it becomes `roundss`, and the native
   build matches OpenJPH generic (0.99 on the irreversible strips). A
   rounding that does not need SSE4.1 would give most of that to the
   default build.
3. **The block decoders' bit readers.** `Frwd::read` alone is 17.5% of
   the 16-bit decode, and it is not inlined into `decode_codeblock32`
   as OpenJPH's `frwd_read` is. htj2k executes 16 to 21% more
   instructions than OpenJPH generic on every reversible decode. On the
   32-bit (64-bit block) decode htj2k is 1.12x OpenJPH generic and 1.32x
   OpenJPH SIMD. OpenJPH has no SIMD 64-bit block decoder, so for
   32-bit samples this is the gap that matters.
4. **Empty code blocks in the encoder.** `Codeblock::push` is 21% of
   the 3840x2160 Qfactor 90 encode, where nearly every block is
   empty, against 5.5% for OpenJPH's `gen_irv_tx_to_cb32`. This row is
   htj2k's worst against generic (1.35x). It also shows on the strips
   (5% of the irreversible encode).
5. **Zeroing and copying.** `memset` (2 to 2.6%) appears in every htj2k
   profile and in none of OpenJPH's, and `Subband::pull_line` (3.6%)
   is a copy that OpenJPH avoids. They are small, but they are on
   every path.

For 32-bit samples OpenJPH's SIMD build gains little over its generic
build (1.0 to 1.4x), because its 64-bit block coders have no SIMD
version. There the default htj2k build is within 1.12 to 1.38x of
OpenJPH SIMD, and the native build within 1.0 to 1.2x. For 16-bit and
12-bit samples, matching OpenJPH SIMD needs SIMD block coders,
wavelets, and colour and sample conversions: about 2x for decoding and
3x for encoding.
