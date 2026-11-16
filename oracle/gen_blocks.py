#!/usr/bin/env python3
"""Generates the code-block decoder vectors in tests/data/blocks/.

Decodes a selection of the codestreams in tests/data/matrix and
tests/data/strips with ojph_dec while $OJPH_BLOCK_DUMP is set, so every call to
OpenJPH's generic ojph_decode_codeblock32/64 appends one record (layout in
tests/data/blocks/README.md).  The records are then deduplicated and capped
per output file, and written as <group>.ojbk plus INDEX.txt.

Run gen_matrix.py and gen_strips.py first (the inputs come from there).
"""

import os
import struct
import sys

from common import BUILD, DATA, DEC, clean_dir, fnv1a64, run

HDR = struct.Struct("<4sBBBB8I")  # 40 bytes

# group -> (record cap, byte cap, must-include sources, other sources)
GROUPS = [
    ("dec32_small", 600, 300_000,
     # levels-0 streams whose minimum sample OpenJPH's encoder drops (see
     # MANIFEST); their blocks are valid and decode to the stored value.
     ["levels0_noise_33x31", "blk4x1024_37x61"],
     ["size_1x1", "levels32_1x1", "size_1x1_l5_s12", "size_1x17",
      "size_37x1", "levels6_3x2_off", "levels10_5x3", "tiles_1x17",
      "tiles_37x1", "pat_zero_u8", "pat_const_u8", "pat_const_s16",
      "depth1u_noise", "depth1s_noise", "depth1u_smooth", "blk4x4_37x29",
      "blk4x4_noise16", "blk8x16_noise_29x40", "prec_1x1_res0",
      "prec_nonsquare", "tiles7x5_37x29", "sub_3x2_tiles",
      "sub_4x4_signed16", "imgoff_odd_tiles_3comps", "coc_levels_rpcl",
      "s16_1c_64x8", "s16_1c_const_64x32"]),
    ("dec32_depths", 300, 260_000, [],
     ["depth8u_noise", "depth8s_smooth", "depth12u_noise", "depth12s_smooth",
      "depth16s_noise", "depth16u_smooth", "depth24u_noise",
      "depth24s_smooth", "depth26s_noise_l5", "depth27s_noise_l5",
      "depth28s_noise", "depth28u_smooth", "rct1_depth24u", "rct0_depth28s",
      "nlt1_depth24s", "levels5_noise_33x31", "levels2_noise_33x31",
      "irv_u8", "irv_s16", "irv_rct_u12", "irv_u32", "pat_noise_s16"]),
    ("dec32_large", 200, 560_000, [],
     ["size_300x200_s16", "s16_rct_4c_300x200", "blk64x64_130x35",
      "blk128x32_130x35", "blk32x128_35x130", "blk1024x4_300x200",
      "s16_1c_64x32", "s16_rct_4c_300x8", "s16_rct_4c_300x32"]),
    ("dec64", 500, 520_000, [],
     ["depth29s_noise_l5", "depth30s_noise_l5", "depth31u_noise",
      "depth31s_smooth", "depth32s_noise", "depth32u_noise",
      "depth32s_smooth", "rct1_depth28s", "rct1_depth31u", "rct1_depth32s",
      "nlt1_depth32s", "nlt3_depth32s_l5", "pat_noise_u32", "pat_const_u32",
      "pat_zero_u32", "pat_smooth_u32", "s32_rct_4c_300x8",
      "s32_rct_4c_300x32", "s32_rct_4c_300x64"]),
]


def find_stream(name):
    for sub in ("matrix", "strips"):
        p = os.path.join(DATA, sub, name + ".j2c")
        if os.path.exists(p):
            return p
    raise SystemExit("missing input stream %s (run gen_matrix.py and "
                     "gen_strips.py first)" % name)


def parse(buf):
    """Splits a dump into (header tuple, record bytes) pairs."""
    out, p = [], 0
    while p < len(buf):
        h = HDR.unpack_from(buf, p)
        magic, bits, _sc, _res, _pad, mm, npass, l1, l2, w, ht, st, clen = h
        assert magic == b"OJBK" and clen == l1 + l2, (p, h)
        n = HDR.size + clen + w * ht * bits // 8
        out.append((h, buf[p:p + n]))
        p += n
    return out


def dump(name, tmp):
    if os.path.exists(tmp):
        os.remove(tmp)
    rc, _so, se = run([DEC, find_stream(name), os.devnull],
                      env={"OJPH_BLOCK_DUMP": tmp})
    if rc != 0 or se.strip():
        raise SystemExit("ojph_dec %s: rc=%d %s" % (name, rc, se.strip()))
    with open(tmp, "rb") as f:
        return parse(f.read())


def select(cands, must, rec_cap, byte_cap):
    """Deterministic, diversity-first selection of candidate records.

    cands: list of (source, index_in_source, header, bytes), deduplicated.
    Pass 0 takes every record of the must-include sources; pass 1 the first
    record of each distinct (bits, width, height, missing_msbs); pass 2 the
    first record of each source; pass 3 fills round-robin across sources.
    Records that would exceed either cap are skipped.
    """
    chosen, used = set(), [0]

    def take(i):
        if i in chosen or len(chosen) >= rec_cap:
            return
        if used[0] + len(cands[i][3]) > byte_cap:
            return
        chosen.add(i)
        used[0] += len(cands[i][3])

    for i, c in enumerate(cands):
        if c[0] in must:
            take(i)
    seen = set()
    for i, c in enumerate(cands):
        h = c[2]
        k = (h[1], h[9], h[10], h[5])
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
    out = os.path.join(DATA, "blocks")
    clean_dir(out, {".ojbk", ".txt"})
    tmp = os.path.join(BUILD, "block_dump.tmp")
    index = [
        "# Code-block decoder records (layout: README.md).  One line per",
        "# record:",
        "# file | record | byte offset | record bytes | source stream |"
        " call# in source | decoder bits | missing_msbs | num_passes |"
        " lengths1 | lengths2 | width | height | stride | stripe_causal |"
        " result | fnv1a64 of decoded samples",
    ]
    totals = {}
    for group, rec_cap, byte_cap, must, srcs in GROUPS:
        cands, seen = [], set()
        for src in must + srcs:
            for j, (h, rec) in enumerate(dump(src, tmp)):
                if rec in seen:
                    continue
                seen.add(rec)
                cands.append((src, j, h, rec))
        sel = select(cands, set(must), rec_cap, byte_cap)
        path = os.path.join(out, group + ".ojbk")
        off = 0
        with open(path, "wb") as f:
            for n, i in enumerate(sel):
                src, j, h, rec = cands[i]
                f.write(rec)
                _m, bits, sc, res, _p, mm, npass, l1, l2, w, ht, st, cl = h
                samples = rec[HDR.size + cl:]
                index.append(
                    "%s | %d | %d | %d | %s | %d | %d | %d | %d | %d | %d |"
                    " %d | %d | %d | %d | %d | %016x" % (
                        group, n, off, len(rec), src, j, bits, mm, npass, l1,
                        l2, w, ht, st, sc, res, fnv1a64(samples)))
                off += len(rec)
        totals[group] = (len(sel), len(cands), off)
        print("%s: %d of %d distinct records, %d bytes" % (
            group, len(sel), len(cands), off))
    if os.path.exists(tmp):
        os.remove(tmp)
    with open(os.path.join(out, "INDEX.txt"), "w") as f:
        f.write("\n".join(index) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
