#!/usr/bin/env python3
"""Generates the strip codestreams in tests/data/strips/.

These are decoder vectors at sizes the matrix avoids: images 300 samples
wide and 8 to 200 lines high (64 for 32-bit samples) with four
components, and 64 samples wide with one, coded with one tile, RPCL, 128x32 code blocks, 5
decompositions, a TLM marker and tile parts at resolutions. All are
reversible, with 16-bit or 32-bit signed samples under NLT type 3 and
the RCT on four components. Each is encoded with ojph_enc and decoded
with ojph_dec, and must round-trip.

Every codestream is committed, but only the decodes in COMMIT_RAW are
(the rest would add about 2 MB); the manifest keeps the size and
FNV-1a-64 hash of every decode, so a test can check any of them. All
decodes are left in oracle/build/strips/. The manifest has the layout
of tests/data/matrix/MANIFEST.txt.
"""

import os
import shutil
import sys

from common import BUILD, DATA, DEC, ENC, clean_dir, fnv1a64_file, run

LAYOUT = ["--levels", "5", "--block", "128x32", "--prog", "RPCL", "--tlm",
          "--tileparts", "R"]
S16 = ["--depth", "16", "--signed", "1", "--nlt3"]
S32 = ["--depth", "32", "--signed", "1", "--nlt3"]
RCT4 = ["--comps", "4", "--rct", "1", "--planar", "0"]

# The decodes small enough to commit.
COMMIT_RAW = {"s16_rct_4c_300x8", "s32_rct_4c_300x8", "s16_1c_64x32",
              "s16_1c_64x8", "s16_1c_const_64x32", "s16_1c_const_64x8"}


def cases():
    c = []

    def add(name, size, kind, *extra, seed):
        c.append((name, ["--size", size] + kind + LAYOUT + list(extra) +
                  ["--seed", str(seed)]))

    add("s16_rct_4c_300x200", "300x200", S16 + RCT4, seed=6041)
    add("s16_rct_4c_300x32", "300x32", S16 + RCT4, seed=6002)
    add("s16_rct_4c_noise_300x32", "300x32", S16 + RCT4, "--pattern",
        "noise", seed=6043)
    add("s16_rct_4c_300x8", "300x8", S16 + RCT4, seed=6004)
    # 32-bit samples code to about twice the bytes; 64 lines still give
    # two rows of 32-line code blocks at the full resolution
    add("s32_rct_4c_300x64", "300x64", S32 + RCT4, seed=6011)
    add("s32_rct_4c_300x32", "300x32", S32 + RCT4, seed=6012)
    add("s32_rct_4c_300x8", "300x8", S32 + RCT4, seed=6014)
    add("s16_1c_64x32", "64x32", S16, seed=6021)
    add("s16_1c_64x8", "64x8", S16, seed=6022)
    add("s16_1c_const_64x32", "64x32", S16, "--pattern", "const",
        seed=6023)
    add("s16_1c_const_64x8", "64x8", S16, "--pattern", "const", seed=6024)
    return c


def main():
    out = os.path.join(DATA, "strips")
    build = os.path.join(BUILD, "strips")
    clean_dir(out, {".j2c", ".raw", ".txt"})
    clean_dir(build, {".raw"})
    manifest = [
        "# Strip codestreams made with oracle/ojph_enc and decoded with",
        "# oracle/ojph_dec by oracle/gen_strips.py.  Columns:",
        "#   name | status | j2c bytes | raw bytes | raw_fnv1a64 | command",
        "# status: lossless = decoded .raw equals the encoder input exactly;",
        "#         lossy = irreversible.",
        "# Every .j2c is here; only some .raw files are (COMMIT_RAW in",
        "# gen_strips.py), all of them are in oracle/build/strips/.",
    ]
    bad = 0
    names = set()
    for name, args in cases():
        assert name not in names, name
        names.add(name)
        j2c = os.path.join(out, name + ".j2c")
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
        if open(rin, "rb").read() == open(raw, "rb").read():
            status = "lossless"
        elif "--irrev" in args:
            status = "lossy"
        else:
            status = "MISMATCH"
            bad += 1
            print("MISMATCH", name)
        if name in COMMIT_RAW:
            shutil.copyfile(raw, os.path.join(out, name + ".raw"))
        manifest.append("%s | %s | %d | %d | %s | %s" % (
            name, status, os.path.getsize(j2c), os.path.getsize(raw),
            fnv1a64_file(raw), cmd))
    assert COMMIT_RAW <= names, COMMIT_RAW - names
    with open(os.path.join(out, "MANIFEST.txt"), "w") as f:
        f.write("\n".join(manifest) + "\n")
    print("strips: %d cases, %d problems" % (len(names), bad))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
