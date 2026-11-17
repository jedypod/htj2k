#!/usr/bin/env python3
"""Generates tests/data/profile/MANIFEST.txt: encoder references at large
sizes and over quality sweeps, kept as sizes and hashes.

Every case is an ojph_enc command line, encoded with OpenJPH and decoded
with ojph_dec. The codestreams are too large to commit (up to a few MB
each), or too many, so the manifest keeps the size and FNV-1a-64 hash of
the codestream, of its main header (up to the first SOT, so a test can
tell a parameter difference from one in the coded data) and of
ojph_dec's decode. tests/encode_profile.rs encodes each case with the
crate's encoder and requires the same bytes, then decodes them with the
crate's decoder and requires ojph_dec's samples.

The cases:
* wide_*: one tile, RPCL, 128x32 code blocks, 5 decompositions, TLM and
  tile parts at resolutions, at sizes from 8192x256 down to 8192x1 and
  1x256, with 1 to 4 components of 16-bit signed (NLT type 3), 32-bit
  signed (NLT type 3) and 32-bit unsigned samples, reversible with the
  RCT on three or four components, planar when subsampled;
* ict_*: the same layout, irreversible with the ICT and NLT type 4 on
  the first three components, at the same sizes and at sizes of 1 mod 32
  (8193x257 to 1x257), with a COM segment, and with reversible extra
  components (NLT type 3 when signed);
* sweep_*: one 64x32 image of three components, irreversible with the
  ICT and NLT type 4, at every Qfactor from 1 to 100 and at 54
  explicit per-component steps, for 16-bit and 32-bit signed samples.

The codestreams, their decodes and the encoder inputs are left in
oracle/build/profile/.
"""

import os
import sys

from common import BUILD, DATA, DEC, ENC, clean_dir, fnv1a64, fnv1a64_file, run

# One tile, RPCL, 128x32 blocks, TLM and tile parts at resolutions.
LAYOUT = ["--block", "128x32", "--prog", "RPCL", "--tlm", "--tileparts", "R"]

S16 = ["--depth", "16", "--signed", "1", "--nlt3"]
S32 = ["--depth", "32", "--signed", "1", "--nlt3"]
U32 = ["--depth", "32", "--signed", "0"]
RCT = ["--rct", "1", "--planar", "0"]


def nlt4(comps, points):
    out = []
    for c in comps:
        out += ["--nlt4", "%d:%d" % (c, points)]
    return out


# Irreversible with the ICT and NLT type 4 on components 0 to 2.
ICT16 = ["--depth", "16", "--signed", "1", "--irrev", "--rct", "1"] + \
    nlt4(range(3), 513)
ICT32 = ["--depth", "32", "--signed", "1", "--irrev", "--rct", "1"] + \
    nlt4(range(3), 257)


def steps(k):
    """The k-th step of the explicit sweep, for components 0, 1 and 2."""
    base = 0.006 * 0.85 ** k
    return ["--qstep", "%.6g" % base, "--qstep-comp", "1:%.6g" % (base * 0.96),
            "--qstep-comp", "2:%.6g" % (base * 1.1)]


def cases():
    c = []

    def add(name, size, comps, kind, *extra, levels=5, seed):
        args = ["--size", size, "--comps", str(comps), "--levels",
                str(levels)] + kind + LAYOUT + list(extra) + \
            ["--seed", str(seed)]
        c.append((name, args))

    # -- reversible, short and wide --
    add("wide_s16_rct_4c_8192x32", "8192x32", 4, S16, *RCT, seed=5001)
    add("wide_s16_1c_8192x32", "8192x32", 1, S16, seed=5002)
    add("wide_s32_rct_3c_8192x32", "8192x32", 3, S32, *RCT, seed=5003)
    add("wide_u32_1c_8192x32", "8192x32", 1, U32, seed=5004)
    add("wide_s32_rct_4c_noise_8192x32", "8192x32", 4, S32, *RCT,
        "--pattern", "noise", seed=5005)
    add("wide_s16_rct_4c_2048x256", "2048x256", 4, S16, *RCT, seed=5011)
    add("wide_s16_rct_3c_noise_2048x256", "2048x256", 3, S16, *RCT,
        "--pattern", "noise", seed=5012)
    add("wide_s32_rct_4c_2048x256", "2048x256", 4, S32, *RCT, seed=5013)
    add("wide_u32_rct_3c_2048x256", "2048x256", 3, U32, *RCT, seed=5014)
    add("wide_u32_4c_planar_2048x256", "2048x256", 4, U32, "--planar", "1",
        seed=5015)
    # luminance and two subsampled chroma components: planar, no RCT
    add("wide_s16_sub_3c_2048x256", "2048x256", 3, S16, "--sub",
        "1x1,2x2,2x2", "--planar", "1", seed=5021)
    add("wide_s16_sub_3c_8192x32", "8192x32", 3, S16, "--sub",
        "1x1,2x2,2x2", "--planar", "1", seed=5022)
    # short images, and small ones with fewer levels
    add("wide_s16_rct_4c_8192x8", "8192x8", 4, S16, *RCT, seed=5031)
    add("wide_s32_rct_3c_300x8", "300x8", 3, S32, *RCT, seed=5032)
    add("wide_s16_rct_4c_64x8_lv3", "64x8", 4, S16, *RCT, levels=3,
        seed=5033)
    add("wide_u32_1c_16x16_lv2", "16x16", 1, U32, levels=2, seed=5034)
    # from 8192x256 down to 8192x1, and 1x256
    for i, size in enumerate(["8192x256", "8192x64", "8192x16", "8192x3",
                              "8192x1", "1x256"]):
        add("wide_s16_rct_3c_%s" % size, size, 3, S16, *RCT, seed=5040 + i)
    add("wide_s32_1c_8192x1", "8192x1", 1, S32, seed=5046)
    add("wide_u32_rct_3c_1x256", "1x256", 3, U32, *RCT, seed=5047)

    # -- irreversible with the ICT and NLT type 4 --
    for i, size in enumerate(["8192x256", "8192x32", "8192x8", "8192x3",
                              "8192x1", "1x256"]):
        add("ict_s16_q90_%s" % size, size, 3, ICT16, "--qfactor", "90",
            seed=5100 + i)
    add("ict_s16_noise_q60_2048x256", "2048x256", 3, ICT16, "--qfactor",
        "60", "--pattern", "noise", seed=5106)
    add("ict_s32_steps_2048x256", "2048x256", 3, ICT32, *steps(10),
        seed=5107)
    add("ict_s32_q75_8192x32", "8192x32", 3, ICT32, "--qfactor", "75",
        seed=5108)
    # sizes of 1 mod 32, with a COM segment recording the padding
    for i, (size, pad) in enumerate([
            ("8193x257", "rows=1,cols=1"), ("8193x33", "rows=1,cols=1"),
            ("8193x1", "rows=0,cols=1"), ("4097x33", "rows=30,cols=1"),
            ("513x33", "rows=16,cols=0"), ("33x33", "rows=0,cols=0"),
            ("1x257", "rows=1,cols=0")]):
        add("ict_s16_pad1_%s" % size, size, 3, ICT16, "--qfactor", "90",
            "--comment", "padding:" + pad, seed=5110 + i)
    add("ict_s32_pad1_steps_97x33", "97x33", 3, ICT32, *steps(3),
        "--comment", "padding:rows=28,cols=0", seed=5117)
    # reversible extra components: 32-bit signed with NLT type 3, and
    # 32-bit unsigned
    mixed = ["--comp-fmt", "16:1,16:1,16:1,32:1,32:0", "--irrev", "--rct",
             "1", "--rev-comps", "3,4", "--nlt3-comps", "3"] + \
        nlt4(range(3), 513)
    add("ict_mixed_5c_q85_2048x256", "2048x256", 5, mixed, "--qfactor",
        "85", seed=5120)
    add("ict_mixed_5c_steps_1000x64", "1000x64", 5, mixed, *steps(20),
        seed=5121)
    add("ict_s32_rev_s16_4c_q50_512x256", "512x256", 4,
        ["--comp-fmt", "32:1,32:1,32:1,16:1", "--irrev", "--rct", "1",
         "--rev-comps", "3", "--nlt3-comps", "3"] + nlt4(range(3), 257),
        "--qfactor", "50", seed=5122)
    # a subsampled fourth component: planar, so no ICT
    add("irv_s16_sub_4c_q90_512x64", "512x64", 4,
        ["--depth", "16", "--signed", "1", "--irrev", "--sub",
         "1x1,1x1,1x1,2x2", "--planar", "1", "--rev-comps", "3",
         "--nlt3-comps", "3"] + nlt4(range(3), 513), "--qfactor", "90",
        seed=5123)

    # -- sweeps on one small image --
    for kind, ict in [("s16", ICT16), ("s32", ICT32)]:
        for q in range(1, 101):
            add("sweep_%s_q%d" % (kind, q), "64x32", 3, ict, "--qfactor",
                str(q), seed=5200)
        for k in range(54):
            add("sweep_%s_step%d" % (kind, k), "64x32", 3, ict, *steps(k),
                seed=5200)
    return c


def main_header_len(data):
    """The length of the main header: up to the first SOT marker."""
    i = 2
    while i + 4 <= len(data):
        if data[i] != 0xFF:
            raise ValueError("bad marker at %d" % i)
        if data[i + 1] == 0x90:
            return i
        i += 2 + ((data[i + 2] << 8) | data[i + 3])
    raise ValueError("no SOT")


def main():
    out = os.path.join(DATA, "profile")
    build = os.path.join(BUILD, "profile")
    clean_dir(out, {".txt"})
    clean_dir(build, {".j2c", ".raw"})
    manifest = [
        "# Encoder references at large sizes and over quality sweeps,",
        "# made with oracle/ojph_enc and decoded with oracle/ojph_dec by",
        "# oracle/gen_profile.py.  The codestreams are not committed;",
        "# they are in oracle/build/profile/ after a regeneration.",
        "# Columns:",
        "#   name | status | j2c bytes | j2c_fnv1a64 | header bytes |",
        "#   header_fnv1a64 | raw bytes | raw_fnv1a64 | command",
        "# status: lossless = the decode equals the encoder input exactly;",
        "# lossy = irreversible.  header is the main header, up to the",
        "# first SOT; raw is ojph_dec's decode of the codestream.",
    ]
    bad = 0
    names = set()
    counts = {}
    for name, args in cases():
        assert name not in names, name
        names.add(name)
        j2c = os.path.join(build, name + ".j2c")
        rin = os.path.join(build, name + ".in.raw")
        raw = os.path.join(build, name + ".raw")
        cmd = "ojph_enc -o %s.j2c %s" % (name, " ".join(args))
        rc, _so, se = run([ENC, "-o", j2c, "--raw-out", rin] + args)
        if rc != 0:
            print("REFUSED", name, se.strip())
            bad += 1
            continue
        rc, _so, se = run([DEC, j2c, raw])
        if rc != 0:
            print("DECODE-FAILED", name, se.strip())
            bad += 1
            continue
        lossy = "--irrev" in args or "--irrev-comps" in args
        if open(rin, "rb").read() == open(raw, "rb").read():
            status = "lossless"
        elif lossy:
            status = "lossy"
        else:
            status = "MISMATCH"
            bad += 1
            print("MISMATCH", name)
        counts[status] = counts.get(status, 0) + 1
        data = open(j2c, "rb").read()
        hl = main_header_len(data)
        manifest.append("%s | %s | %d | %s | %d | %016x | %d | %s | %s" % (
            name, status, len(data), fnv1a64_file(j2c), hl,
            fnv1a64(data[:hl]), os.path.getsize(raw), fnv1a64_file(raw),
            cmd))
    with open(os.path.join(out, "MANIFEST.txt"), "w") as f:
        f.write("\n".join(manifest) + "\n")
    print("profile: %d cases %s, %d problems" % (len(names), counts, bad))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
