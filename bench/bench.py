#!/usr/bin/env python3
"""Runs the codec benchmark: correctness checks, then perf counters.

Called by bench/run.sh after it has built the four harnesses; see
bench/README.md for the method. Usage:

    python3 bench/bench.py [--cpu N] [--repeat R] [--target S] [--only SUBSTR]
                           [--profile] [--out DIR]

For every configuration it first runs `check` on each harness and
requires:
  * htj2k's codestream (both builds) == OpenJPH generic's, byte for byte;
  * every decode (htj2k, htj2k native, OpenJPH SIMD) == OpenJPH generic's.
OpenJPH SIMD's codestream is compared with generic's too, and reported.

Then, for each configuration, operation (enc, dec) and harness, it runs

    taskset -c CPU perf stat -x, -e cycles:u,instructions:u,task-clock -r R \
        HARNESS OP N ...      and again with 2N

and takes (count(2N) - count(N)) / (N * samples), a sample being one
pixel of one component, so that the set-up (synthesis, the encode before
decoding, process start) cancels. N is chosen so that N iterations code
about S samples (--target, default 16M). The run-to-run variation is
perf's standard deviation of the mean over the R runs, propagated through
the difference, as a percentage of it.

Results go to DIR (default bench/results): results.csv, results.md and
log.txt (the commands, perf output and `uptime` before and after each
perf run). With --profile it also records `perf record` profiles of a
few configurations and writes the top symbols to profile.txt
(--profile-only: the checks and the profiles, without perf stat).
The set-up (synthesis, and the encode before decoding) is in the
profiles; N is four times the perf stat N to keep it small.
"""

import argparse
import math
import os
import shlex
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

HARNESSES = [
    ("ojph_gen", "OpenJPH generic", "oracle/build/bench/bench_codec_generic"),
    ("ojph_simd", "OpenJPH SIMD", "oracle/build/bench/bench_codec_simd"),
    ("htj2k", "htj2k", "target/release/examples/bench_codec"),
    ("htj2k_nat", "htj2k native", "target/native/release/examples/bench_codec"),
]

STRIP = "--levels 5 --block 128x32 --prog RPCL --tlm --tileparts R"


def configs():
    """(name, samples, ojph_enc options) for every configuration."""
    out = []
    seed = 7000

    def add(name, w, h, comps, opts):
        nonlocal seed
        seed += 1
        out.append((name, w * h * comps,
                    f"--size {w}x{h} --comps {comps} {opts} --seed {seed}"))

    # Short, wide strips, reversible, NLT type 3 on signed samples.
    for (w, h) in ((8192, 32), (2048, 256)):
        for depth in (16, 32):
            for comps in (1, 3, 4):
                rct = " --rct 1 --planar 0" if comps >= 3 else ""
                add(f"s{depth}_{comps}c_rev_{w}x{h}", w, h, comps,
                    f"--depth {depth} --signed 1 --nlt3{rct} {STRIP}")
    # Irreversible 3-component strips: the ICT, NLT type 4, Qfactor 90.
    for (w, h) in ((8192, 32), (2048, 256)):
        for depth in (16, 32):
            pts = 513 if depth == 16 else 257
            nlt4 = " ".join(f"--nlt4 {c}:{pts}" for c in range(3))
            add(f"s{depth}_3c_ict_q90_{w}x{h}", w, h, 3,
                f"--depth {depth} --signed 1 --irrev --rct 1 {nlt4} "
                f"--qfactor 90 {STRIP}")
    # The same at a fixed step, 0.002, for a rate typical of high-quality
    # irreversible coding (about 5.5 bits per sample here). Qfactor 90
    # quantises the synthetic images' white noise, which is all high
    # frequency, almost entirely away (well under 1 bit per sample).
    for depth in (16, 32):
        pts = 513 if depth == 16 else 257
        nlt4 = " ".join(f"--nlt4 {c}:{pts}" for c in range(3))
        add(f"s{depth}_3c_ict_qs002_8192x32", 8192, 32, 3,
            f"--depth {depth} --signed 1 --irrev --rct 1 {nlt4} "
            f"--qstep 0.002 {STRIP}")
    # Worst case: noise over the full range.
    for depth in (16, 32):
        add(f"s{depth}_3c_rev_noise_8192x32", 8192, 32, 3,
            f"--depth {depth} --signed 1 --nlt3 --rct 1 --planar 0 {STRIP} "
            f"--pattern noise")
    # A typical whole image: OpenJPH's defaults (64x64 blocks, 5 levels,
    # LRCP), 12-bit unsigned RGB with the colour transform.
    add("u12_3c_rev_3840x2160", 3840, 2160, 3, "--depth 12 --rct 1")
    add("u12_3c_ict_q90_3840x2160", 3840, 2160, 3,
        "--depth 12 --rct 1 --irrev --qfactor 90")
    add("u12_3c_ict_qs002_3840x2160", 3840, 2160, 3,
        "--depth 12 --rct 1 --irrev --qstep 0.002")
    return out


class Log:
    def __init__(self, path):
        self.f = open(path, "w")

    def __call__(self, *parts):
        text = " ".join(str(p) for p in parts)
        self.f.write(text + "\n")
        self.f.flush()

    def echo(self, *parts):
        self(*parts)
        print(*parts, flush=True)


def cpu_times():
    t = {}
    with open("/proc/stat") as f:
        for line in f:
            if line.startswith("cpu") and line[3].isdigit():
                p = line.split()
                v = [int(x) for x in p[1:]]
                idle = v[3] + (v[4] if len(v) > 4 else 0)
                t[int(p[0][3:])] = (sum(v), idle)
    return t


def siblings(cpu):
    path = f"/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list"
    try:
        text = open(path).read().strip()
    except OSError:
        return [cpu]
    out = []
    for part in text.split(","):
        if "-" in part:
            a, b = part.split("-")
            out.extend(range(int(a), int(b) + 1))
        else:
            out.append(int(part))
    return out


def pick_cpu(log, seconds=3.0):
    """The CPU whose physical core (it and its SMT siblings) was least busy."""
    a = cpu_times()
    time.sleep(seconds)
    b = cpu_times()
    busy = {}
    for c in a:
        dt = b[c][0] - a[c][0]
        di = b[c][1] - a[c][1]
        busy[c] = 1.0 - di / dt if dt > 0 else 1.0
    score = {c: max(busy[s] for s in siblings(c) if s in busy) + busy[c] * 1e-3
             for c in busy if c != 0}
    cpu = min(score, key=score.get)
    log("busy over %.0fs: " % seconds
        + " ".join(f"{c}:{busy[c]:.2f}" for c in sorted(busy)))
    log.echo(f"picked cpu {cpu} (siblings {siblings(cpu)}, "
             f"core busy {score[cpu]:.2f})")
    return cpu


def run(cmd, log):
    log("$", " ".join(shlex.quote(c) for c in cmd))
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if r.returncode != 0:
        log(r.stdout, r.stderr)
        raise RuntimeError(f"command failed ({r.returncode}): {' '.join(cmd)}\n"
                           f"{r.stderr}")
    return r.stdout


def parse_kv(text):
    return dict(p.split("=", 1) for p in text.split())


def uptime():
    return subprocess.run(["uptime"], capture_output=True, text=True).stdout.strip()


def perf_stat(cpu, repeat, cmd, log):
    """Mean and relative stddev of the mean of each event, and the stdout."""
    # Relative to ROOT, where every command runs, so that log.txt holds
    # no absolute paths.
    out_file = os.path.join("target", "bench_perf.csv")
    full = ["taskset", "-c", str(cpu), "perf", "stat", "-x,", "-o", out_file,
            "-e", "cycles:u,instructions:u,task-clock", "-r", str(repeat),
            "--"] + cmd
    log("uptime before:", uptime())
    stdout = run(full, log)
    log("uptime after: ", uptime())
    text = open(os.path.join(ROOT, out_file)).read()
    log(text.strip())
    res = {}
    for line in text.splitlines():
        if not line or line.startswith("#"):
            continue
        f = line.split(",")
        if len(f) < 4:
            continue
        ev = f[2].split(":")[0]
        try:
            val = float(f[0])
        except ValueError:
            raise RuntimeError(f"perf could not count {f[2]}: {line}")
        var = float(f[3].rstrip("%")) / 100.0 if f[3].endswith("%") else 0.0
        res[ev] = (val, var)
    return res, stdout.strip()


def diff(a, b):
    """(b - a) and the relative stddev of the difference."""
    (va, ra), (vb, rb) = a, b
    d = vb - va
    sd = math.hypot(va * ra, vb * rb)
    return d, (sd / d if d > 0 else float("inf"))


def check(cfgs, log):
    """Runs `check` everywhere. Returns {name: problems}."""
    problems = {}
    for name, _, opts in cfgs:
        res = {}
        for key, _, path in HARNESSES:
            res[key] = parse_kv(run([path, "check"] + shlex.split(opts), log))
        ref = res["ojph_gen"]
        bad = []
        for key in ("htj2k", "htj2k_nat"):
            if (res[key]["j2c_bytes"], res[key]["j2c_fnv"]) != (ref["j2c_bytes"], ref["j2c_fnv"]):
                bad.append(f"{key} codestream differs from OpenJPH generic")
        if (res["ojph_simd"]["j2c_bytes"], res["ojph_simd"]["j2c_fnv"]) != (ref["j2c_bytes"], ref["j2c_fnv"]):
            bad.append("OpenJPH SIMD codestream differs from OpenJPH generic")
        for key in ("htj2k", "htj2k_nat", "ojph_simd"):
            if (res[key]["raw_bytes"], res[key]["raw_fnv"]) != (ref["raw_bytes"], ref["raw_fnv"]):
                bad.append(f"{key} decode differs from OpenJPH generic "
                           f"({raw_diff(key, opts, log)})")
        problems[name] = (bad, ref)
        log.echo(f"check {name}: j2c {ref['j2c_bytes']} bytes, "
                 + ("OK" if not bad else "; ".join(bad)))
    return problems


def raw_diff(key, opts, log):
    """How `key`'s decode differs from OpenJPH generic's (by `dump`)."""
    path = dict((k, p) for k, _, p in HARNESSES)
    dumps = []
    for k in ("ojph_gen", key):
        cmd = [path[k], "dump"] + shlex.split(opts)
        log("$", " ".join(shlex.quote(c) for c in cmd), "> (memory)")
        r = subprocess.run(cmd, cwd=ROOT, capture_output=True)
        if r.returncode != 0:
            return f"dump failed: {r.stderr.decode(errors='replace').strip()}"
        dumps.append(r.stdout)
    a, b = dumps
    if len(a) != len(b):
        return f"{len(a)} vs {len(b)} bytes"
    import array
    x = array.array("i", a)
    y = array.array("i", b)
    if sys.byteorder != "little":
        x.byteswap()
        y.byteswap()
    d = [abs(p - q) for p, q in zip(x, y) if p != q]
    return (f"{len(d)} of {len(x)} samples differ, by at most {max(d)}"
            if d else "same samples")


def fmt(v, digits=1):
    return f"{v:.{digits}f}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cpu", type=int)
    ap.add_argument("--repeat", type=int, default=5)
    ap.add_argument("--target", type=float, default=16e6)
    ap.add_argument("--only")
    ap.add_argument("--profile", action="store_true")
    ap.add_argument("--no-perf", action="store_true",
                    help="only run the correctness checks")
    ap.add_argument("--profile-only", action="store_true",
                    help="only the correctness checks and the profiles")
    ap.add_argument("--out", default=os.path.join(ROOT, "bench", "results"))
    a = ap.parse_args()

    os.makedirs(a.out, exist_ok=True)
    log = Log(os.path.join(a.out, "log.txt"))
    cfgs = [c for c in configs() if not a.only or a.only in c[0]]
    for key, _, path in HARNESSES:
        if not os.path.exists(os.path.join(ROOT, path)):
            sys.exit(f"bench.py: {path} is missing; run bench/run.sh")

    log.echo("== correctness")
    problems = check(cfgs, log)
    if a.no_perf:
        return
    cpu = a.cpu if a.cpu is not None else pick_cpu(log)
    if a.profile_only:
        profile(cpu, cfgs, a.out, log)
        return

    log.echo("== perf stat")
    rows = []
    csv = open(os.path.join(a.out, "results.csv"), "w")
    csv.write("config,samples,op,harness,iters,cycles_per_sample,"
              "instructions_per_sample,cycles_var_pct,instructions_var_pct,"
              "task_clock_ms_per_iter\n")
    for name, samples, opts in cfgs:
        n = max(1, round(a.target / samples))
        for op in ("dec", "enc"):
            row = {"config": name, "op": op, "samples": samples, "n": n,
                   "bps": 8.0 * int(problems[name][1]["j2c_bytes"]) / samples}
            for key, _, path in HARNESSES:
                base = [path, op]
                r1, o1 = perf_stat(cpu, a.repeat, base + [str(n)] + shlex.split(opts), log)
                r2, o2 = perf_stat(cpu, a.repeat, base + [str(2 * n)] + shlex.split(opts), log)
                if o1 != o2:
                    raise RuntimeError(f"{name} {op} {key}: outputs differ: {o1} / {o2}")
                cyc, cv = diff(r1["cycles"], r2["cycles"])
                ins, iv = diff(r1["instructions"], r2["instructions"])
                tc, _ = diff(r1["task-clock"], r2["task-clock"])
                v = {"cyc": cyc / n / samples, "ins": ins / n / samples,
                     "cv": cv * 100, "iv": iv * 100, "ms": tc / n}
                row[key] = v
                csv.write(f"{name},{samples},{op},{key},{n},{v['cyc']:.3f},"
                          f"{v['ins']:.3f},{v['cv']:.2f},{v['iv']:.2f},"
                          f"{v['ms']:.3f}\n")
                csv.flush()
                log.echo(f"{name:28} {op} {key:10} N={n:<4} "
                         f"{v['cyc']:8.2f} cyc/sample (±{v['cv']:.2f}%)  "
                         f"{v['ins']:8.2f} ins/sample (±{v['iv']:.2f}%)")
            rows.append(row)
    csv.close()

    md = open(os.path.join(a.out, "results.md"), "w")
    for op, title in (("dec", "Decode"), ("enc", "Encode")):
        md.write(f"### {title}\n\n")
        md.write("Cycles per sample (cycles:u), and htj2k's ratios:\n\n")
        md.write("| configuration | bits/sample | OpenJPH generic | OpenJPH SIMD | htj2k "
                 "| htj2k native | htj2k / generic | htj2k / SIMD | native / generic "
                 "| native / SIMD | ± max |\n")
        md.write("|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|\n")
        for r in rows:
            if r["op"] != op:
                continue
            g, s, h, nt = (r[k]["cyc"] for k in ("ojph_gen", "ojph_simd", "htj2k", "htj2k_nat"))
            var = max(r[k]["cv"] for k in ("ojph_gen", "ojph_simd", "htj2k", "htj2k_nat"))
            md.write(f"| {r['config']} | {fmt(r['bps'], 2)} | {fmt(g)} | {fmt(s)} "
                     f"| {fmt(h)} | {fmt(nt)} "
                     f"| {fmt(h / g, 2)} | {fmt(h / s, 2)} | {fmt(nt / g, 2)} "
                     f"| {fmt(nt / s, 2)} | {fmt(var, 1)}% |\n")
        md.write("\nInstructions per sample (instructions:u):\n\n")
        md.write("| configuration | OpenJPH generic | OpenJPH SIMD | htj2k | htj2k native "
                 "| htj2k / generic | htj2k / SIMD | ± max |\n")
        md.write("|---|--:|--:|--:|--:|--:|--:|--:|\n")
        for r in rows:
            if r["op"] != op:
                continue
            g, s, h, nt = (r[k]["ins"] for k in ("ojph_gen", "ojph_simd", "htj2k", "htj2k_nat"))
            var = max(r[k]["iv"] for k in ("ojph_gen", "ojph_simd", "htj2k", "htj2k_nat"))
            md.write(f"| {r['config']} | {fmt(g)} | {fmt(s)} | {fmt(h)} | {fmt(nt)} "
                     f"| {fmt(h / g, 2)} | {fmt(h / s, 2)} | {fmt(var, 2)}% |\n")
        md.write("\n")
    bad = {k: v[0] for k, v in problems.items() if v[0]}
    md.write("Correctness: " + ("all checks passed.\n" if not bad else
             "; ".join(f"{k}: {', '.join(v)}" for k, v in bad.items()) + "\n"))
    md.close()
    log.echo(f"wrote {a.out}/results.md")

    if a.profile:
        profile(cpu, cfgs, a.out, log)


PROFILE_CONFIGS = [
    "s16_3c_rev_8192x32",
    "s32_3c_rev_8192x32",
    "s16_3c_ict_q90_8192x32",
    "u12_3c_rev_3840x2160",
    "u12_3c_ict_q90_3840x2160",
]


def demangle_v0(sym):
    """The path of a Rust v0 symbol, `a::b::c`, without generic arguments.

    Enough for reading a profile: identifiers are length-prefixed, and the
    disambiguators (s..._), back-references (B..._) and constants (K..._)
    between them are skipped. Other symbols are returned unchanged.
    """
    if not sym.startswith("_R"):
        return sym
    body = sym[2:].split(".llvm.")[0]
    out, i = [], 0
    while i < len(body):
        c = body[i]
        if c in "sBK":
            j = body.find("_", i)
            i = len(body) if j < 0 else j + 1
        elif c.isdigit():
            j = i
            while j < len(body) and body[j].isdigit():
                j += 1
            n = int(body[i:j])
            if j < len(body) and body[j] == "_":
                j += 1
            out.append(body[j:j + n])
            i = j + n
        else:
            i += 1
    return "::".join(out) if out else sym


def profile(cpu, cfgs, out, log):
    """perf record of htj2k and OpenJPH generic on a few configurations."""
    log.echo("== perf record")
    txt = open(os.path.join(out, "profile.txt"), "w")
    data = os.path.join("target", "bench_perf.data")
    for name, samples, opts in cfgs:
        if name not in PROFILE_CONFIGS:
            continue
        n = max(4, round(4 * 16e6 / samples))
        for op in ("dec", "enc"):
            for key, label, path in HARNESSES:
                if key not in ("htj2k", "ojph_gen"):
                    continue
                run(["taskset", "-c", str(cpu), "perf", "record", "-q", "-e", "cycles:u",
                     "-o", data, "--", path, op, str(n)] + shlex.split(opts), log)
                rep = run(["perf", "report", "--stdio", "-i", data, "--no-children",
                           "--sort", "symbol", "--percent-limit", "1.5"], log)
                lines = []
                for l in rep.splitlines():
                    if not l.strip() or l.startswith("#"):
                        continue
                    f = l.split()
                    # "  12.34%  [.] symbol ..."
                    if len(f) >= 3 and f[1] in ("[.]", "[k]"):
                        while len(f) > 3 and f[-1] == "-":
                            f.pop()
                        sym = (demangle_v0(f[2]) if f[2].startswith("_R")
                               else " ".join(f[2:]))
                        l = f"{f[0]:>8}  {f[1]} {sym}"
                    lines.append(l.rstrip())
                txt.write(f"## {name} {op} {label} (N={n})\n\n")
                txt.write("\n".join(lines[:12]) + "\n\n")
                txt.flush()
                log.echo(f"profiled {name} {op} {label}")
    txt.close()


if __name__ == "__main__":
    main()
