#!/usr/bin/env python3
"""Generates the parameter-matrix codestreams in tests/data/matrix/.

Every case is encoded with ojph_enc (which also dumps its input samples to
oracle/build/matrix_in/<name>.raw) and decoded with ojph_dec into
tests/data/matrix/<name>.raw.  For reversible cases the decoded output must
equal the encoder input byte for byte; any difference is reported (and the
script exits non-zero) rather than hidden.  Cases OpenJPH refuses to encode
are listed in MANIFEST.txt with the error text, and no files are kept.
"""

import os
import sys

from common import HERE, BUILD, DATA, DEC, ENC, clean_dir, fnv1a64_file, run


def cases():
    c = []

    def add(name, *args):
        c.append((name, list(args)))

    # -- progression orders (3 comps, several resolutions and precincts) --
    for p in ["LRCP", "RLCP", "RPCL", "PCRL", "CPRL"]:
        add("prog_%s" % p.lower(), "--size", "40x24", "--comps", "3",
            "--levels", "2", "--block", "8x8", "--precincts", "8x8,16x16",
            "--prog", p, "--planar", "0", "--seed", "11")
    add("prog_rpcl_tiles", "--size", "45x30", "--comps", "3", "--levels", "3",
        "--block", "8x8", "--precincts", "16x16", "--prog", "RPCL",
        "--tile", "24x16", "--seed", "12")
    add("prog_pcrl_sub", "--size", "40x24", "--comps", "3", "--levels", "2",
        "--block", "8x8", "--precincts", "8x8,16x16", "--prog", "PCRL",
        "--sub", "1x1,2x2,2x2", "--seed", "13")

    # -- decomposition levels --
    for lv in [0, 1, 2, 5]:
        add("levels%d_32x32" % lv, "--size", "32x32", "--levels", str(lv),
            "--depth", "8", "--seed", "20")
        add("levels%d_noise_33x31" % lv, "--size", "33x31", "--levels",
            str(lv), "--depth", "10", "--pattern", "noise", "--seed", "21")
    add("levels8_17x9", "--size", "17x9", "--levels", "8", "--seed", "22")
    add("levels10_5x3", "--size", "5x3", "--levels", "10", "--seed", "23")
    add("levels32_1x1", "--size", "1x1", "--levels", "32", "--seed", "24")
    add("levels33_4x4", "--size", "4x4", "--levels", "33", "--seed", "24")
    add("levels6_3x2_off", "--size", "3x2", "--img-off", "5,3", "--levels",
        "6", "--seed", "25")

    # -- code-block sizes --
    add("blk4x4_37x29", "--size", "37x29", "--block", "4x4", "--levels", "2",
        "--seed", "30")
    add("blk4x4_noise16", "--size", "24x20", "--block", "4x4", "--levels",
        "1", "--depth", "16", "--pattern", "noise", "--seed", "31")
    add("blk64x64_130x35", "--size", "130x35", "--block", "64x64",
        "--levels", "2", "--seed", "32")
    add("blk128x32_130x35", "--size", "130x35", "--block", "128x32",
        "--levels", "1", "--depth", "12", "--seed", "33")
    add("blk32x128_35x130", "--size", "35x130", "--block", "32x128",
        "--levels", "1", "--depth", "12", "--seed", "34")
    add("blk1024x4_300x200", "--size", "300x200", "--block", "1024x4",
        "--levels", "1", "--depth", "8", "--seed", "35")
    add("blk4x1024_37x61", "--size", "37x61", "--block", "4x1024",
        "--levels", "0", "--depth", "8", "--seed", "36")
    add("blk8x16_noise_29x40", "--size", "29x40", "--block", "8x16",
        "--levels", "1", "--depth", "12", "--pattern", "noise", "--seed",
        "37")
    add("blk2x2_illegal", "--size", "8x8", "--block", "2x2")
    add("blk128x64_illegal", "--size", "8x8", "--block", "128x64")

    # -- bit depths, signed and unsigned --
    for d in [1, 8, 12, 16, 24, 28, 31, 32]:
        for s in [0, 1]:
            sg = "s" if s else "u"
            add("depth%d%s_noise" % (d, sg), "--size", "24x20", "--depth",
                str(d), "--signed", str(s), "--levels", "3", "--pattern",
                "noise", "--seed", str(100 + d * 2 + s))
            add("depth%d%s_smooth" % (d, sg), "--size", "21x19", "--depth",
                str(d), "--signed", str(s), "--levels", "2", "--block",
                "16x16", "--seed", str(200 + d * 2 + s))
    for d in [26, 27, 29, 30]:
        add("depth%ds_noise_l5" % d, "--size", "20x20", "--depth", str(d),
            "--signed", "1", "--levels", "5", "--block", "8x8", "--pattern",
            "noise", "--seed", str(300 + d))

    # -- colour transform --
    for d, s in [(8, 0), (12, 0), (16, 1), (24, 0), (28, 1), (31, 0),
                 (32, 1), (32, 0)]:
        sg = "s" if s else "u"
        for r in [0, 1]:
            add("rct%d_depth%d%s" % (r, d, sg), "--size", "19x17", "--comps",
                "3", "--rct", str(r), "--depth", str(d), "--signed", str(s),
                "--levels", "2", "--block", "8x8", "--pattern",
                "noise" if d >= 24 else "smooth", "--seed", str(400 + d))
    add("rct1_4comps", "--size", "23x18", "--comps", "4", "--rct", "1",
        "--depth", "10", "--levels", "2", "--seed", "450")
    add("rct1_planar_illegal", "--size", "8x8", "--comps", "3", "--rct", "1",
        "--planar", "1")
    add("rct1_2comps", "--size", "8x8", "--comps", "2", "--rct", "1")
    add("planar0_3comps", "--size", "23x18", "--comps", "3", "--planar", "0",
        "--depth", "12", "--levels", "2", "--seed", "451")
    add("comps5_planar", "--size", "13x11", "--comps", "5", "--depth", "6",
        "--levels", "1", "--seed", "452")

    # -- NLT type 3 --
    for d in [8, 16, 24, 32]:
        for n in [0, 1]:
            add("nlt%d_depth%ds" % (n, d), "--size", "22x18", "--depth",
                str(d), "--signed", "1", "--levels", "2", "--block", "16x16",
                "--pattern", "noise" if d >= 24 else "smooth",
                *(["--nlt3"] if n else []), "--seed", str(500 + d))
    add("nlt3_comp1_of3", "--size", "22x18", "--comps", "3", "--depth", "16",
        "--signed", "1", "--nlt3-comps", "1", "--levels", "2", "--seed",
        "540")
    add("nlt3_rct", "--size", "22x18", "--comps", "3", "--depth", "16",
        "--signed", "1", "--nlt3", "--rct", "1", "--levels", "2", "--seed",
        "541")
    add("nlt3_unsigned", "--size", "8x8", "--depth", "16", "--signed", "0",
        "--nlt3-comps", "0", "--seed", "542")
    add("nlt3_depth32s_l5", "--size", "16x16", "--depth", "32", "--signed",
        "1", "--nlt3", "--levels", "5", "--block", "8x8", "--pattern",
        "noise", "--seed", "543")

    # -- tiles and offsets --
    add("tiles16_61x47", "--size", "61x47", "--tile", "16x16", "--levels",
        "2", "--block", "8x8", "--seed", "600")
    add("tiles_off_61x47", "--size", "61x47", "--img-off", "5,3",
        "--tile-off", "2,1", "--tile", "32x20", "--levels", "3", "--block",
        "16x8", "--seed", "601")
    add("tiles7x5_37x29", "--size", "37x29", "--tile", "7x5", "--levels",
        "2", "--block", "4x4", "--depth", "12", "--signed", "1", "--seed",
        "602")
    add("tile_larger_than_image", "--size", "30x20", "--tile", "64x64",
        "--levels", "3", "--seed", "603")
    add("imgoff_13x7_no_tiles", "--size", "37x29", "--img-off", "13,7",
        "--levels", "3", "--block", "8x8", "--seed", "604")
    add("imgoff_odd_tiles_3comps", "--size", "41x33", "--img-off", "7,9",
        "--tile-off", "3,4", "--tile", "16x24", "--comps", "3", "--rct", "1",
        "--levels", "2", "--block", "8x8", "--depth", "10", "--seed", "605")
    add("tiles_1x17", "--size", "1x17", "--tile", "1x5", "--levels", "2",
        "--seed", "606")
    add("tiles_37x1", "--size", "37x1", "--tile", "9x1", "--levels", "3",
        "--seed", "607")
    add("tile_off_illegal", "--size", "16x16", "--img-off", "2,2",
        "--tile-off", "5,5", "--tile", "8x8")

    # -- TLM and tile-part divisions --
    for tlm in [0, 1]:
        for tp in ["", "R", "C", "RC"]:
            nm = "tlm%d_tp%s" % (tlm, tp.lower() or "none")
            args = ["--size", "40x30", "--comps", "3", "--tile", "24x16",
                    "--levels", "2", "--block", "8x8", "--seed", "700"]
            if tlm:
                args.append("--tlm")
            if tp:
                args += ["--tileparts", tp]
            add(nm, *args)
    add("tlm_tpR_cprl", "--size", "40x30", "--comps", "3", "--tile", "24x16",
        "--levels", "2", "--block", "8x8", "--prog", "CPRL", "--tlm",
        "--tileparts", "R", "--seed", "701")
    add("tlm_tpRC_rpcl_rct", "--size", "40x30", "--comps", "3", "--rct", "1",
        "--tile", "24x16", "--levels", "2", "--block", "8x8", "--precincts",
        "8x8", "--prog", "RPCL", "--tlm", "--tileparts", "RC", "--seed",
        "702")

    # -- component subsampling --
    add("sub_2x1", "--size", "32x24", "--comps", "3", "--sub", "1x1,2x1,2x1",
        "--levels", "2", "--block", "8x8", "--seed", "800")
    add("sub_2x2", "--size", "32x24", "--comps", "3", "--sub", "1x1,2x2,2x2",
        "--levels", "2", "--block", "8x8", "--seed", "801")
    add("sub_2x2_odd_off", "--size", "37x29", "--img-off", "1,1", "--comps",
        "3", "--sub", "1x1,2x2,2x2", "--levels", "3", "--block", "8x8",
        "--seed", "802")
    add("sub_3x2_tiles", "--size", "37x29", "--comps", "2", "--sub",
        "1x1,3x2", "--tile", "16x16", "--levels", "2", "--block", "8x8",
        "--seed", "803")
    add("sub_2x2_planar0", "--size", "32x24", "--comps", "3", "--sub",
        "1x1,2x2,2x2", "--planar", "0", "--levels", "2", "--seed", "804")
    add("sub_4x4_signed16", "--size", "33x17", "--comps", "2", "--sub",
        "1x1,4x4", "--depth", "16", "--signed", "1", "--levels", "1",
        "--seed", "805")

    # -- precincts --
    add("prec8x8_64x64", "--size", "64x64", "--levels", "3", "--block",
        "16x16", "--precincts", "8x8", "--seed", "900")
    add("prec_mixed", "--size", "60x44", "--levels", "3", "--block", "8x8",
        "--precincts", "4x4,8x8,16x16,32x32", "--seed", "901")
    add("prec_lt_block", "--size", "48x40", "--levels", "2", "--block",
        "32x32", "--precincts", "16x16", "--seed", "902")
    add("prec_1x1_res0", "--size", "20x20", "--levels", "2", "--block",
        "4x4", "--precincts", "1x1,2x2,4x4", "--seed", "903")
    add("prec_nonsquare", "--size", "50x34", "--levels", "2", "--block",
        "8x4", "--precincts", "32x8,16x32", "--seed", "904")

    # -- COC (per-component coding style) --
    add("coc_levels_blocks", "--size", "33x27", "--comps", "3", "--levels",
        "3", "--block", "16x16", "--coc", "1:1:4x8", "--coc", "2:0:32x32",
        "--seed", "950")
    # Per-component decomposition counts that differ trip an assertion in
    # OpenJPH's encoder for the layer-first orders (LRCP, RLCP) -- kept above
    # and below as REFUSED entries -- but work for the position/component
    # orders.  coc_lv0_rpcl also hits the levels-0 minimum-sample issue.
    add("coc_lv0_rpcl", "--size", "33x27", "--comps", "3", "--levels", "3",
        "--block", "16x16", "--coc", "1:1:4x8", "--coc", "2:0:32x32",
        "--prog", "RPCL", "--seed", "950")
    for p in ["RPCL", "PCRL", "CPRL", "RLCP"]:
        add("coc_levels_%s" % p.lower(), "--size", "33x27", "--comps", "3",
            "--levels", "3", "--block", "16x16", "--coc", "1:1:4x8",
            "--coc", "2:2:32x32", "--prog", p, "--seed", "951")
    add("coc_blocks_only", "--size", "33x27", "--comps", "3", "--levels",
        "2", "--block", "16x16", "--coc", "1:2:4x8", "--coc", "2:2:32x32",
        "--seed", "952")

    # -- patterns --
    for pat in ["smooth", "noise", "const", "zero"]:
        add("pat_%s_u8" % pat, "--size", "32x32", "--pattern", pat,
            "--levels", "3", "--seed", "1000")
        add("pat_%s_s16" % pat, "--size", "27x21", "--pattern", pat,
            "--depth", "16", "--signed", "1", "--levels", "2", "--seed",
            "1001")
        add("pat_%s_u32" % pat, "--size", "16x16", "--pattern", pat,
            "--depth", "32", "--levels", "2", "--block", "8x8", "--seed",
            "1002")

    # -- odd sizes --
    add("size_1x1", "--size", "1x1", "--levels", "0", "--seed", "1100")
    add("size_1x1_l5_s12", "--size", "1x1", "--depth", "12", "--signed", "1",
        "--levels", "5", "--seed", "1101")
    add("size_1x17", "--size", "1x17", "--levels", "3", "--seed", "1102")
    add("size_37x1", "--size", "37x1", "--levels", "3", "--seed", "1103")
    add("size_61x47_3c", "--size", "61x47", "--comps", "3", "--rct", "1",
        "--levels", "4", "--block", "16x16", "--seed", "1104")
    add("size_130x35", "--size", "130x35", "--levels", "5", "--seed", "1105")
    add("size_300x200_s16", "--size", "300x200", "--depth", "16", "--signed",
        "1", "--nlt3", "--levels", "5", "--block", "64x64", "--seed", "1106")

    # -- irreversible 9/7 (lossy; .raw is OpenJPH's rounded integer output)
    add("irv_u8", "--size", "32x32", "--irrev", "--levels", "3",
        "--seed", "1200")
    add("irv_rct_u12", "--size", "32x24", "--comps", "3", "--rct", "1",
        "--irrev", "--depth", "12", "--levels", "3", "--seed", "1201")
    add("irv_s16", "--size", "27x21", "--irrev", "--depth", "16", "--signed",
        "1", "--levels", "2", "--qstep", "0.001", "--seed", "1202")
    add("irv_u32", "--size", "16x16", "--irrev", "--depth", "32",
        "--levels", "2", "--seed", "1203")
    return c


def tidy(stderr):
    """One-line error text with machine-specific source paths shortened."""
    src = os.path.realpath(os.environ.get("OJPH_SRC") or os.path.join(
        HERE, "..", "..", "reference", "OpenJPH"))
    return " / ".join(l.replace(src + "/", "OpenJPH/")
                      for l in stderr.strip().splitlines())


def main():
    out = os.path.join(DATA, "matrix")
    inp = os.path.join(BUILD, "matrix_in")
    clean_dir(out, {".j2c", ".raw", ".txt"})
    clean_dir(inp, {".raw"})
    names = set()
    manifest = [
        "# Parameter-matrix codestreams made with oracle/ojph_enc and decoded",
        "# with oracle/ojph_dec.  Columns:",
        "#   name | status | j2c bytes | raw bytes | raw_fnv1a64 | command",
        "# status: lossless = decoded .raw equals the encoder input exactly;",
        "#         lossy maxerr=N = irreversible, max |decoded - input|;",
        "#         MISMATCH = reversible but decoded != input (a bug);",
        "#         REFUSED = OpenJPH refused to encode (error text follows,",
        "#         no files kept); DECODE-FAILED = ojph_dec failed.",
        "# Commands are relative to tests/data/matrix; ojph_enc also gets",
        "# --raw-out <tmp> to dump its input for the comparison.",
    ]
    bad = 0
    for name, args in cases():
        assert name not in names, name
        names.add(name)
        j2c = os.path.join(out, name + ".j2c")
        raw = os.path.join(out, name + ".raw")
        rin = os.path.join(inp, name + ".raw")
        cmd = "ojph_enc -o %s.j2c %s" % (name, " ".join(args))
        rc, _so, se = run([ENC, "-o", j2c, "--raw-out", rin] + args)
        if rc != 0:
            for p in (j2c, rin):
                if os.path.exists(p):
                    os.remove(p)
            err = tidy(se)
            manifest.append("%s | REFUSED | | | | %s | %s" % (name, cmd, err))
            print("REFUSED", name, err)
            continue
        rc, _so, se = run([DEC, j2c, raw])
        if rc != 0:
            bad += 1
            err = tidy(se)
            manifest.append("%s | DECODE-FAILED | | | | %s | %s" %
                            (name, cmd, err))
            print("DECODE-FAILED", name, err)
            continue
        a = open(rin, "rb").read()
        b = open(raw, "rb").read()
        if "--irrev" in args:
            import struct
            n = len(a) // 4
            # signed interpretation unless --signed 0 (the default)
            signed = "--signed" in args and \
                args[args.index("--signed") + 1] == "1"
            fmt = "<%d%s" % (n, "i" if signed else "I")
            if len(a) != len(b):
                status = "MISMATCH(size)"
                bad += 1
            else:
                ea, eb = struct.unpack(fmt, a), struct.unpack(fmt, b)
                status = "lossy maxerr=%d" % max(
                    abs(x - y) for x, y in zip(ea, eb))
        elif a == b:
            status = "lossless"
        else:
            bad += 1
            ndiff = sum(1 for i in range(0, min(len(a), len(b)), 4)
                        if a[i:i + 4] != b[i:i + 4])
            status = "MISMATCH sizes %d/%d, %d samples differ" % (
                len(a), len(b), ndiff)
            print("MISMATCH", name, status)
        if se.strip():
            status += " (decoder stderr: %s)" % " / ".join(
                se.strip().splitlines())
        manifest.append("%s | %s | %d | %d | %s | %s" % (
            name, status, os.path.getsize(j2c), os.path.getsize(raw),
            fnv1a64_file(raw), cmd))
    with open(os.path.join(out, "MANIFEST.txt"), "w") as f:
        f.write("\n".join(manifest) + "\n")
    print("matrix: %d cases, %d problems" % (len(names), bad))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
