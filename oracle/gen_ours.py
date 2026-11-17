#!/usr/bin/env python3
"""Generates tests/data/ours/MANIFEST.txt: codestreams the crate's encoder
writes on purpose differently from OpenJPH, or that OpenJPH cannot write,
checked by decoding them with OpenJPH.

Each case is an ojph_enc command line. It is encoded by the crate's
encoder through examples/encode_args.rs (built here with cargo), which
also dumps the encoder input, and the codestream is decoded with ojph_dec
and compared with that input. The manifest keeps the size and FNV-1a-64
hash of the crate's codestream with ojph_dec's verdict; tests/encode_ours.rs
checks that the encoder still writes exactly that codestream, so the
verdict still applies, and that the crate's decoder agrees.

The cases:
* levels0_*: reversible coding with no decompositions. OpenJPH's QCD gives
  the only band one magnitude bit-plane too few, so it loses the minimum
  sample (and, for 32-bit unsigned samples, the top bit of about half of
  them); the crate's encoder writes one more (see ParamQcd::set_rev_quant),
  so its codestream differs from OpenJPH's in QCD even where OpenJPH's
  happens to round-trip. Every such matrix case comes first (three are
  MISMATCH there), then random configurations.

The codestreams are left in oracle/build/ours/.
"""

import os
import random
import re
import sys

from common import BUILD, CRATE, DATA, DEC, clean_dir, fnv1a64_file, run

EXAMPLE = os.path.join(CRATE, "target", "release", "examples", "encode_args")


def levels0_reversible(args):
    """Reversible, with some component at 0 decompositions (--levels 5 and
    --coc C:L:WxH as ojph_enc parses them)."""
    if "--irrev" in args:
        return False
    levels = ["5"] + [args[i + 1] for i, a in enumerate(args) if a == "--levels"]
    cocs = [args[i + 1].split(":")[1] for i, a in enumerate(args) if a == "--coc"]
    return levels[-1] == "0" or "0" in cocs


def levels0_cases():
    c = []
    man = open(os.path.join(DATA, "matrix", "MANIFEST.txt")).read()
    for line in man.splitlines():
        if line.startswith("#") or " | REFUSED" in line:
            continue
        f = [x.strip() for x in line.split("|")]
        args = re.sub(r"^ojph_enc -o \S+ ", "", f[5]).split()
        if levels0_reversible(args):
            c.append(("levels0_matrix_" + f[0], args))
    r = random.Random(20261116)
    for i in range(40):
        depth = r.choice([2, 7, 8, 10, 12, 16, 24, 28, 30, 31, 32])
        signed = r.choice([0, 1])
        comps = r.choice([1, 1, 2, 3, 4])
        a = ["--size", "%dx%d" % (r.randint(1, 70), r.randint(1, 40)),
             "--levels", "0", "--depth", str(depth), "--signed", str(signed),
             "--comps", str(comps),
             "--block", r.choice(["4x4", "8x8", "16x16", "64x64", "32x8"]),
             "--pattern", r.choice(["noise", "noise", "smooth", "const"]),
             "--prog", r.choice(["LRCP", "RLCP", "RPCL", "PCRL", "CPRL"]),
             "--seed", str(r.randint(1, 10**6))]
        if comps >= 3 and r.random() < 0.5:
            a += ["--rct", "1", "--planar", "0"]
        if signed and r.random() < 0.4:
            a += ["--nlt3"]
        if r.random() < 0.3:
            a += ["--img-off", "%d,%d" % (r.randint(0, 3), r.randint(0, 3))]
        if r.random() < 0.3:
            a += ["--tile", "%dx%d" % (r.randint(4, 40), r.randint(4, 30))]
        c.append(("levels0_random%02d" % i, a))
    return c


def cases():
    return levels0_cases()


def main():
    rc, _so, se = run(["cargo", "build", "--release", "--quiet",
                       "--manifest-path", os.path.join(CRATE, "Cargo.toml"),
                       "--example", "encode_args"])
    if rc != 0:
        print(se)
        return 1
    out = os.path.join(DATA, "ours")
    build = os.path.join(BUILD, "ours")
    clean_dir(out, {".txt"})
    clean_dir(build, {".j2c", ".raw"})
    manifest = [
        "# Codestreams the crate's encoder writes differently from OpenJPH,",
        "# or that OpenJPH cannot write, encoded by examples/encode_args.rs and",
        "# decoded with oracle/ojph_dec by oracle/gen_ours.py.  Columns:",
        "#   name | status | j2c bytes | j2c_fnv1a64 | command",
        "# status: lossless = ojph_dec's decode equals the encoder input.",
    ]
    bad = 0
    names = set()
    for name, args in cases():
        assert name not in names, name
        names.add(name)
        j2c = os.path.join(build, name + ".j2c")
        rin = os.path.join(build, name + ".in.raw")
        raw = os.path.join(build, name + ".raw")
        cmd = "ojph_enc -o %s.j2c %s" % (name, " ".join(args))
        rc, _so, se = run([EXAMPLE, "-o", j2c, "--raw-out", rin] + args)
        if rc != 0:
            print("REFUSED", name, se.strip())
            bad += 1
            continue
        rc, _so, se = run([DEC, j2c, raw])
        if rc != 0:
            status = "DECODE-FAILED"
        elif open(rin, "rb").read() == open(raw, "rb").read():
            status = "lossless"
        else:
            status = "MISMATCH"
        if status != "lossless":
            bad += 1
            print(status, name, se.strip())
        manifest.append("%s | %s | %d | %s | %s" % (
            name, status, os.path.getsize(j2c), fnv1a64_file(j2c), cmd))
    with open(os.path.join(out, "MANIFEST.txt"), "w") as f:
        f.write("\n".join(manifest) + "\n")
    print("ours: %d cases, %d problems" % (len(names), bad))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
