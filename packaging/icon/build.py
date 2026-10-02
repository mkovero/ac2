#!/usr/bin/env python3
"""Render the app icon from ac2.svg into the formats the packages need.

    python3 packaging/icon/build.py          # writes packaging/icon/generated/
    python3 packaging/icon/build.py --check  # fails if generated/ is stale

Needs `rsvg-convert` (librsvg). ICO and ICNS are packed here from PNG entries (both formats
accept embedded PNG), so no ImageMagick or iconutil is required and the output is the same
on every OS.
"""

import argparse
import io
import struct
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SVG = HERE / "ac2.svg"
OUT = HERE / "generated"

PNG_SIZES = [16, 24, 32, 48, 64, 128, 256, 512, 1024]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
# ICNS element types holding PNG data, by pixel size (Retina @2x variants included).
ICNS_TYPES = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"icp6", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic13", 256),
    (b"ic14", 512),
]


def render(size: int) -> bytes:
    return subprocess.run(
        ["rsvg-convert", "-w", str(size), "-h", str(size), str(SVG)],
        check=True,
        capture_output=True,
    ).stdout


def ico(pngs: dict) -> bytes:
    entries = [(s, pngs[s]) for s in ICO_SIZES]
    head = struct.pack("<HHH", 0, 1, len(entries))
    offset = len(head) + 16 * len(entries)
    dirs, data = b"", b""
    for size, png in entries:
        dim = 0 if size >= 256 else size  # 0 means 256 in an ICONDIRENTRY
        dirs += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset + len(data))
        data += png
    return head + dirs + data


def icns(pngs: dict) -> bytes:
    body = b""
    for kind, size in ICNS_TYPES:
        png = pngs[size]
        body += kind + struct.pack(">I", 8 + len(png)) + png
    return b"icns" + struct.pack(">I", 8 + len(body)) + body


def outputs() -> dict:
    pngs = {s: render(s) for s in PNG_SIZES}
    files = {f"ac2-{s}.png": pngs[s] for s in (16, 32, 48, 64, 128, 256, 512)}
    files["ac2.ico"] = ico(pngs)
    files["ac2.icns"] = icns(pngs)
    return files


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    files = outputs()
    if args.check:
        stale = [n for n, b in files.items() if not (OUT / n).exists() or (OUT / n).read_bytes() != b]
        if stale:
            print("stale icon outputs:", ", ".join(stale), file=sys.stderr)
            return 1
        return 0
    OUT.mkdir(exist_ok=True)
    for name, b in files.items():
        (OUT / name).write_bytes(b)
    print(f"wrote {len(files)} files to {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
