#!/usr/bin/env python3
"""One-shot converter of a host's stored files to the PROTO 105 shapes.

Run once per host with the old daemon stopped, before the first start of a 105 daemon:

    python3 tools/proto-next/convert.py --rig ~/.config/ac2/rig.json

Each file is rewritten atomically; the original is kept beside it as `<name>.pre105`.
A file already in the new shape is left alone. Covers, per file:

- rig.json (rig settings) 11 -> 12: `monitor_session` -> `monitor_stream`.

Deleted once every host has been converted.
"""

import argparse
import json
import os
import shutil
import sys
import tempfile

RIG_FORMAT = "ac2-rig-settings"


def convert_rig(doc):
    """The rig settings document in format 12; raises ValueError on another version."""
    if doc.get("format") != RIG_FORMAT:
        raise ValueError(f"not {RIG_FORMAT}")
    version = doc.get("version")
    if version == 12:
        return None
    if version != 11:
        raise ValueError(f"format version {version!r}; this converter reads 11")
    out = {}
    for key, value in doc.items():
        out["monitor_stream" if key == "monitor_session" else key] = value
    out["version"] = 12
    return out


def rewrite(path, convert):
    with open(path, encoding="utf-8") as f:
        doc = json.load(f)
    new = convert(doc)
    if new is None:
        print(f"{path}: already converted")
        return
    shutil.copy2(path, path + ".pre105")
    d = os.path.dirname(os.path.abspath(path))
    fd, tmp = tempfile.mkstemp(dir=d, prefix=".convert-")
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        json.dump(new, f, indent=2, ensure_ascii=False)
        f.write("\n")
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)
    print(f"{path}: converted (original kept as {path}.pre105)")


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--rig", action="append", default=[], help="a rig.json to convert")
    args = ap.parse_args(argv)
    if not args.rig:
        ap.error("nothing to convert: give --rig PATH")
    failed = False
    for p in args.rig:
        try:
            rewrite(p, convert_rig)
        except (OSError, ValueError) as e:
            print(f"{p}: {e}", file=sys.stderr)
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
