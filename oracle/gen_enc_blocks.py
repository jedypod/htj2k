#!/usr/bin/env python3
"""Generates the code-block encoder vectors in tests/data/encoder/.

Runs ojph_enc on a set of images while $OJPH_ENC_BLOCK_DUMP is set, so every
call to OpenJPH's generic ojph_encode_codeblock32/64 appends one OJBE record
(layout in tests/data/encoder/README.md).  The records are deduplicated and
capped per output file (enc32.ojbe, enc64.ojbe).  enc_blocks_synth, which
calls the encoders directly with synthetic blocks, writes synth.ojbe.
INDEX.txt describes every record.

Needs only oracle/build.sh to have been run; the images are synthesised by
ojph_enc, so this does not depend on the other generators.
"""

import os
import struct
import sys

from common import BUILD, DATA, ENC, clean_dir, fnv1a64, run

SYNTH = os.path.join(BUILD, "enc_blocks_synth")
HDR = struct.Struct("<4sBBBB8I")  # 40 bytes


def cases():
    """(name, ojph_enc arguments, 32 or 64: the encoder the case is for)."""
    c = []

    def add(bits, name, *args):
        c.append((name, list(args), bits))

    # -- 32-bit encoder (K_max <= 30) --
    # 128x32 blocks, 5 levels, RCT and NLT type 3 on 16-bit samples.
    add(32, "s16_rct_128x32", "--size", "256x64", "--comps", "3", "--depth",
        "16", "--signed", "1", "--rct", "1", "--levels", "5", "--block",
        "128x32", "--seed", "1")
    add(32, "s16_nlt3_rct_128x32", "--size", "256x64", "--comps", "3", "--depth",
        "16", "--signed", "1", "--nlt3", "--rct", "1", "--levels", "5",
        "--block", "128x32", "--pattern", "noise", "--seed", "2")
    add(32, "u16_noise_128x32", "--size", "160x40", "--depth", "16",
        "--levels", "5", "--block", "128x32", "--pattern", "noise", "--seed",
        "3")
    add(32, "u8_smooth_64", "--size", "130x70", "--depth", "8", "--levels",
        "2", "--block", "64x64", "--seed", "4")
    add(32, "s12_noise_32x128", "--size", "40x140", "--depth", "12",
        "--signed", "1", "--levels", "1", "--block", "32x128", "--pattern",
        "noise", "--seed", "5")
    add(32, "u8_noise_4x4", "--size", "21x19", "--depth", "8", "--levels",
        "2", "--block", "4x4", "--pattern", "noise", "--seed", "6")
    add(32, "u1_noise", "--size", "37x29", "--depth", "1", "--levels", "3",
        "--block", "8x8", "--pattern", "noise", "--seed", "7")
    add(32, "u8_const", "--size", "33x31", "--depth", "8", "--levels", "3",
        "--block", "16x16", "--pattern", "const", "--seed", "8")
    add(32, "s24_smooth_odd", "--size", "45x27", "--depth", "24", "--signed",
        "1", "--levels", "3", "--block", "16x8", "--seed", "9")
    add(32, "u28_noise", "--size", "30x22", "--depth", "28", "--levels",
        "2", "--block", "8x16", "--pattern", "noise", "--seed", "10")
    add(32, "irv_u12", "--size", "48x40", "--comps", "3", "--depth", "12",
        "--irrev", "--rct", "1", "--levels", "3", "--block", "32x32",
        "--seed", "11")
    add(32, "lv0_noise_1024x4", "--size", "1024x8", "--depth", "8",
        "--levels", "0", "--block", "1024x4", "--pattern", "noise", "--seed",
        "12")
    add(32, "lv0_smooth_4x1024", "--size", "8x1024", "--depth", "10",
        "--levels", "0", "--block", "4x1024", "--seed", "13")
    add(32, "tiles_sub", "--size", "37x29", "--comps", "3", "--depth", "8",
        "--levels", "2", "--block", "8x8", "--tile", "16x16", "--sub",
        "1x1,2x2,2x1", "--seed", "14")
    # -- 64-bit encoder (K_max > 30) --
    # 128x32 blocks, 5 levels, RCT and NLT type 3 on 32-bit samples.
    add(64, "s32_rct_128x32", "--size", "256x64", "--comps", "3", "--depth",
        "32", "--signed", "1", "--rct", "1", "--levels", "5", "--block",
        "128x32", "--seed", "21")
    add(64, "s32_nlt3_128x32", "--size", "160x40", "--depth", "32", "--signed",
        "1", "--nlt3", "--levels", "5", "--block", "128x32", "--pattern",
        "noise", "--seed", "22")
    add(64, "u32_noise", "--size", "40x36", "--depth", "32", "--levels", "3",
        "--block", "16x16", "--pattern", "noise", "--seed", "23")
    add(64, "u32_smooth", "--size", "37x29", "--depth", "32", "--levels",
        "2", "--block", "8x8", "--seed", "24")
    add(64, "s31_noise", "--size", "33x31", "--depth", "31", "--signed", "1",
        "--levels", "5", "--block", "32x32", "--pattern", "noise", "--seed",
        "25")
    add(64, "s30_noise_l5", "--size", "24x20", "--depth", "30", "--signed",
        "1", "--levels", "5", "--block", "8x8", "--pattern", "noise",
        "--seed", "26")
    add(64, "u32_const", "--size", "19x17", "--depth", "32", "--levels",
        "2", "--block", "8x8", "--pattern", "const", "--seed", "27")
    add(64, "lv0_u32_noise_1024x4", "--size", "1024x4", "--depth", "32",
        "--levels", "0", "--block", "1024x4", "--pattern", "noise", "--seed",
        "28")
    add(64, "s32_4x4", "--size", "13x11", "--depth", "32", "--signed", "1",
        "--levels", "2", "--block", "4x4", "--pattern", "noise", "--seed",
        "29")
    return c


# file -> (record cap, byte cap)
CAPS = {"enc32": (500, 700_000), "enc64": (300, 900_000)}


def parse(buf):
    """Splits a dump into (header tuple, record bytes) pairs."""
    out, p = [], 0
    while p < len(buf):
        h = HDR.unpack_from(buf, p)
        magic, bits, failed, _r0, _r1, mm, npass, w, ht, st, l0, clen, err = h
        assert magic == b"OJBE" and (failed or clen == l0), (p, h)
        n = HDR.size + w * ht * bits // 8 + clen
        out.append((h, buf[p:p + n]))
        p += n
    return out


def dump(args, tmp):
    if os.path.exists(tmp):
        os.remove(tmp)
    rc, _so, se = run(args, env={"OJPH_ENC_BLOCK_DUMP": tmp})
    return rc, se, (parse(open(tmp, "rb").read())
                    if os.path.exists(tmp) else [])


def select(cands, rec_cap, byte_cap):
    """Deterministic, diversity-first selection, as in gen_blocks.py: the
    first record of each distinct (width, height, missing_msbs), then the
    first of each source, then round-robin across sources, within caps."""
    chosen, used = set(), [0]

    def take(i):
        if i in chosen or len(chosen) >= rec_cap:
            return
        if used[0] + len(cands[i][3]) > byte_cap:
            return
        chosen.add(i)
        used[0] += len(cands[i][3])

    seen = set()
    for i, c in enumerate(cands):
        h = c[2]
        k = (h[7], h[8], h[5])
        if k not in seen:
            seen.add(k)
            take(i)
    seen = set()
    for i, c in enumerate(cands):
        if c[0] not in seen:
            seen.add(c[0])
            take(i)
    by_src = {}
    for i, c in enumerate(cands):
        by_src.setdefault(c[0], []).append(i)
    queues = list(by_src.values())
    while len(chosen) < rec_cap and any(queues):
        for q in queues:
            if q:
                take(q.pop(0))
    return sorted(chosen)


def main():
    out = os.path.join(DATA, "encoder")
    clean_dir(out, {".ojbe", ".txt"})
    tmp = os.path.join(BUILD, "enc_block_dump.tmp")
    j2c = os.path.join(BUILD, "enc_block_dump.j2c")
    groups = {"enc32": [], "enc64": []}
    seen = set()
    for name, args, bits in cases():
        rc, se, recs = dump([ENC, "-o", j2c] + args, tmp)
        if rc != 0 or se.strip():
            raise SystemExit("ojph_enc %s: rc=%d %s" % (name, rc, se.strip()))
        if not recs:
            raise SystemExit("ojph_enc %s: no encoder calls" % name)
        for j, (h, rec) in enumerate(recs):
            if h[1] != bits:
                raise SystemExit("%s: expected only %d-bit calls" % (name, bits))
            if rec in seen:
                continue
            seen.add(rec)
            groups["enc%d" % bits].append((name, j, h, rec))
    rc, se, synth = dump([SYNTH], tmp)
    if rc != 0:
        raise SystemExit("enc_blocks_synth: rc=%d %s" % (rc, se.strip()))
    sys.stderr.write(se)
    files = [(g, [groups[g][i] for i in select(groups[g], *CAPS[g])])
             for g in ("enc32", "enc64")]
    files.append(("synth", [("synth", j, h, rec)
                            for j, (h, rec) in enumerate(synth)]))
    index = [
        "# Code-block encoder records (layout: README.md).  One line per",
        "# record:",
        "# file | record | byte offset | record bytes | source |"
        " call# in source | encoder bits | failed | error code |"
        " missing_msbs | num_passes | width | height | stride | length |"
        " fnv1a64 of coded bytes",
    ]
    for group, recs in files:
        off = 0
        with open(os.path.join(out, group + ".ojbe"), "wb") as f:
            for n, (src, j, h, rec) in enumerate(recs):
                f.write(rec)
                (_m, bits, failed, _r0, _r1, mm, npass, w, ht, st, l0, clen,
                 err) = h
                index.append(
                    "%s | %d | %d | %d | %s | %d | %d | %d | 0x%08x | %d | %d |"
                    " %d | %d | %d | %d | %016x" % (
                        group, n, off, len(rec), src, j, bits, failed, err,
                        mm, npass, w, ht, st, l0,
                        fnv1a64(rec[len(rec) - clen:] if clen else b"")))
                off += len(rec)
        print("%s: %d records, %d bytes" % (group, len(recs), off))
    print("enc32/enc64 candidates: %d / %d distinct records" % (
        len(groups["enc32"]), len(groups["enc64"])))
    for p in (tmp, j2c):
        if os.path.exists(p):
            os.remove(p)
    with open(os.path.join(out, "INDEX.txt"), "w") as f:
        f.write("\n".join(index) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
