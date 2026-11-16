"""Shared helpers for the oracle regeneration scripts."""

import os
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)
BUILD = os.path.join(HERE, "build")
DATA = os.path.join(CRATE, "tests", "data")
ENC = os.path.join(BUILD, "ojph_enc")
DEC = os.path.join(BUILD, "ojph_dec")


def fnv1a64(data: bytes) -> int:
    """FNV-1a 64-bit (offset basis 0xcbf29ce484222325, prime 0x100000001b3)."""
    h = 0xCBF29CE484222325
    for b in data:
        h ^= b
        h = (h * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def fnv1a64_file(path: str) -> str:
    with open(path, "rb") as f:
        return "%016x" % fnv1a64(f.read())


def run(args, env=None):
    """Runs a command, returns (returncode, stdout, stderr) as text."""
    e = dict(os.environ)
    e.pop("OJPH_BLOCK_DUMP", None)
    if env:
        e.update(env)
    p = subprocess.run(args, capture_output=True, text=True, env=e)
    return p.returncode, p.stdout, p.stderr


def clean_dir(path: str, exts):
    os.makedirs(path, exist_ok=True)
    for n in os.listdir(path):
        if os.path.splitext(n)[1] in exts:
            os.remove(os.path.join(path, n))
