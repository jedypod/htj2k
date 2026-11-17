#!/usr/bin/env python3
"""Generates tests/data/profile/MANIFEST.txt: encoder references at large
sizes, kept as sizes and hashes.

Every case is an ojph_enc command line, encoded with OpenJPH and decoded
with ojph_dec. The codestreams are too large to commit (up to a few MB
each), so the manifest keeps the size and FNV-1a-64 hash of
the codestream, of its main header (up to the first SOT, so a test can
tell a parameter difference from one in the coded data) and of
ojph_dec's decode. tests/encode_profile.rs encodes each case with the
crate's encoder and requires the same bytes, then decodes them with the
crate's decoder and requires ojph_dec's samples.

The cases, wide_*: one tile, RPCL, 128x32 code blocks, 5
decompositions, TLM and tile parts at resolutions, at sizes from
8192x256 down to 8192x1 and 1x256, with 1 to 4 components of 16-bit
signed (NLT type 3), 32-bit signed (NLT type 3) and 32-bit unsigned
samples, reversible with the RCT on three or four components, planar
when subsampled.

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
        "# Encoder references at large sizes,",
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
        lossy = "--irrev" in args
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
