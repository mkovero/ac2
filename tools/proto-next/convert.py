#!/usr/bin/env python3
"""One-shot converter of a host's stored files to the PROTO 105 shapes.

Run once per host with the old daemon stopped, before the first start of a 105 daemon:

    python3 tools/proto-next/convert.py --rig ~/.config/ac2/rig.json \
        --cal ~/.config/ac2/calibrations.json --plans ~/.local/share/ac2/plans

Each file is rewritten atomically; the original is kept beside it as `<name>.pre105`.
A file already in the new shape is left alone. Covers, per file:

- rig.json (rig settings) 11 -> 12: `monitor_session` -> `monitor_stream`; a band
  transfer's bands tagged by `type`, not `status`.
- calibration store 4 -> 5: an input's `InputKind` tagged by `type`, not `kind`.
- plan files (`plans/*.json`), unversioned -> `version: 1`: a surface tagged by `type`,
  not `kind`.

Deleted once every host has been converted.
"""

import argparse
import json
import os
import shutil
import sys
import tempfile

RIG_FORMAT = "ac2-rig-settings"
CAL_FORMAT = "ac2-calibrations"
PLAN_VERSION = 1


def retag(obj, old):
    """`obj`, a dict tagged by `old`, tagged by `type` instead (key order kept)."""
    if not isinstance(obj, dict) or old not in obj or "type" in obj:
        return obj
    return {("type" if k == old else k): v for k, v in obj.items()}


def band_transfer_types(node):
    """Every band transfer's bands under `node` tagged by `type`."""
    if isinstance(node, dict):
        for k, v in list(node.items()):
            if k == "transfer" and isinstance(v, dict) and isinstance(v.get("bands"), list):
                v["bands"] = [retag(b, "status") for b in v["bands"]]
            band_transfer_types(v)
    elif isinstance(node, list):
        for v in node:
            band_transfer_types(v)


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
    band_transfer_types(out.get("meters"))
    out["version"] = 12
    return out


def convert_cal(doc):
    """The calibration store in format 5; raises ValueError on another version."""
    if doc.get("format") != CAL_FORMAT:
        raise ValueError(f"not {CAL_FORMAT}")
    version = doc.get("version")
    if version == 5:
        return None
    if version != 4:
        raise ValueError(f"format version {version!r}; this converter reads 4")
    out = dict(doc)
    out["inputs"] = [
        {**i, "kind": retag(i.get("kind"), "kind")} if isinstance(i, dict) else i
        for i in doc.get("inputs", [])
    ]
    out["version"] = 5
    return out


def convert_plan(doc):
    """A plan file in version 1; raises ValueError when it is no plan."""
    if not isinstance(doc, dict) or "name" not in doc or "outline" not in doc:
        raise ValueError("not a plan")
    if doc.get("version") == PLAN_VERSION:
        return None
    if "version" in doc:
        raise ValueError(f"plan file version {doc['version']!r}; this converter reads none")
    out = {"version": PLAN_VERSION}
    for key, value in doc.items():
        if key == "surfaces" and isinstance(value, list):
            value = [
                {**s, "surface": retag(s.get("surface"), "kind")} if isinstance(s, dict) else s
                for s in value
            ]
        out[key] = value
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
    ap.add_argument("--cal", action="append", default=[], help="a calibration store to convert")
    ap.add_argument(
        "--plans", action="append", default=[], help="a plan directory: every *.json in it"
    )
    args = ap.parse_args(argv)
    jobs = [(p, convert_rig) for p in args.rig] + [(p, convert_cal) for p in args.cal]
    for d in args.plans:
        try:
            names = sorted(n for n in os.listdir(d) if n.endswith(".json"))
        except OSError as e:
            print(f"{d}: {e}", file=sys.stderr)
            return 1
        jobs += [(os.path.join(d, n), convert_plan) for n in names]
    if not jobs:
        ap.error("nothing to convert: give --rig, --cal or --plans PATH")
    failed = False
    for p, convert in jobs:
        try:
            rewrite(p, convert)
        except (OSError, ValueError) as e:
            print(f"{p}: {e}", file=sys.stderr)
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
