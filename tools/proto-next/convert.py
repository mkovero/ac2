#!/usr/bin/env python3
"""One-shot converter of a host's stored files to the PROTO 105 shapes.

Run once per host with the old daemon stopped, before the first start of a 105 daemon:

    python3 tools/proto-next/convert.py --rig ~/.config/ac2/rig.json \
        --cal ~/.config/ac2/calibrations.json --plans ~/.local/share/ac2/plans \
        --auth ~/.config/ac2/authorized_clients

A 105 daemon started before that sets an old authorized-clients file aside as
`authorized_clients.damaged-<time>`: convert that file and put it back as
`authorized_clients`, the daemon stopped.

Each file is rewritten atomically; the original is kept beside it as `<name>.pre105`.
A file already in the new shape is left alone. Covers, per file:

- rig.json (rig settings) 11 -> 12: `monitor_session` -> `monitor_stream`; a band
  transfer's bands tagged by `type`, not `status`; a paired rig without `data` (its data
  endpoint is the port after `ctrl`); a pane layout's `levels` and `plates` written out
  where missing (none set, every plate at its default place and size); every rig id (a
  paired rig's `rig`, a remote input's `rig`, a pane's pinned rig `id`) a decimal string;
  every cue state's DMX level with `snap: false` (it fades, as every level did).
- calibration store 4 -> 5: an input's `InputKind` tagged by `type`, not `kind`; an
  electrical calibration's `loading` written out (`null`, unloaded) where missing; every
  `calibrated_at` and `imported_at` a decimal string.
- plan files (`plans/*.json`), unversioned -> `version: 1`: a surface tagged by `type`,
  not `kind`.
- authorized clients (text, `<name> <key>` a line) -> `<name> <key> <role>`: every app's key
  becomes `operator` (what every key could do before roles); a rig's `rig=<id>` stays. No
  key is printed.

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
AUTH_HEADER = "# ac2 authorized clients: <name> <Z85 CURVE public key> <operator | viewer | rig=<rig id>>"
AUTH_ROLES = ("operator", "viewer")


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


LAYOUT_LEVELS = ("transfer", "spectrum_dbfs", "spectrum_spl", "distortion")
LAYOUT_PLATES = ("legend", "readout", "mark", "peaks", "harmonics", "loudness")


def layout_fields(node):
    """Every pane layout under `node` (a map with `chrome` and `content`) with its `levels`
    and `plates`, as a layout that never set them reads."""
    if isinstance(node, dict):
        if "chrome" in node and "content" in node:
            node.setdefault("levels", {k: None for k in LAYOUT_LEVELS})
            node.setdefault("plates", {k: {"at": None, "scale": 1.0} for k in LAYOUT_PLATES})
        for v in node.values():
            layout_fields(v)
    elif isinstance(node, list):
        for v in node:
            layout_fields(v)


def dmx_snaps(node):
    """Every cue state's DMX levels under `node` (a `CueAction`: `osc` and `dmx`) with
    `snap: false`."""
    if isinstance(node, dict):
        if "osc" in node and isinstance(node.get("dmx"), list):
            for d in node["dmx"]:
                if isinstance(d, dict):
                    d.setdefault("snap", False)
        for v in node.values():
            dmx_snaps(v)
    elif isinstance(node, list):
        for v in node:
            dmx_snaps(v)


def dec(v):
    """A u64 as the files now hold it: its decimal string (a JSON number read by Python is
    exact, so nothing is lost here)."""
    return str(v) if isinstance(v, int) and not isinstance(v, bool) else v


def rig_ids(node):
    """Every rig id under `node` as a decimal string: a pinned rig of a pane
    (`{"type": "rig", "id": ...}`) and the `rig` of a paired rig or a remote input."""
    if isinstance(node, dict):
        if node.get("type") == "rig" and "id" in node:
            node["id"] = dec(node["id"])
        if "rig" in node:
            node["rig"] = dec(node["rig"])
        for v in node.values():
            rig_ids(v)
    elif isinstance(node, list):
        for v in node:
            rig_ids(v)


def wall_times(node, keys):
    """Every value under `node` keyed by one of `keys` as a decimal string."""
    if isinstance(node, dict):
        for k, v in node.items():
            if k in keys:
                node[k] = dec(v)
            else:
                wall_times(v, keys)
    elif isinstance(node, list):
        for v in node:
            wall_times(v, keys)


def electrical_loading(node):
    """Every electrical calibration under `node` with its `loading`, `null` where missing."""
    if isinstance(node, dict):
        if node.get("type") == "electrical" and "mic_sensitivity_from" in node:
            node.setdefault("loading", None)
        for v in node.values():
            electrical_loading(v)
    elif isinstance(node, list):
        for v in node:
            electrical_loading(v)


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
    layout_fields(out.get("meters"))
    out["peers"] = [
        {k: v for k, v in p.items() if k != "data"} if isinstance(p, dict) else p
        for p in out.get("peers", [])
    ]
    rig_ids(out.get("peers"))
    rig_ids(out.get("meters"))
    rig_ids(out.get("monitor_stream"))
    dmx_snaps(out.get("meters"))
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
    electrical_loading(out.get("sensitivities"))
    wall_times(out, {"calibrated_at", "imported_at"})
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


def convert_auth(text):
    """The authorized-clients text with a role on every key line; `None` when every line
    has one. A line that is neither shape is an error naming its line number only: the
    file holds keys."""
    out = []
    changed = False
    for n, line in enumerate(text.splitlines(), 1):
        t = line.strip()
        if t.startswith("# ac2 authorized clients"):
            changed |= t != AUTH_HEADER
            out.append(AUTH_HEADER)
            continue
        if not t or t.startswith("#"):
            out.append(line)
            continue
        f = t.split()
        if len(f) == 2:
            out.append(f"{f[0]} {f[1]} operator")
            changed = True
        elif len(f) == 3 and (f[2] in AUTH_ROLES or f[2].startswith("rig=")):
            out.append(line)
        else:
            raise ValueError(f"line {n}: neither `<name> <key>` nor `<name> <key> <role>`")
    return "\n".join(out) + "\n" if changed else None


def write_beside(path, text, mode=None):
    """Writes `text` over `path` atomically, with `mode` (default: its own); the original
    kept as `.pre105`, with the same mode."""
    keep = path + ".pre105"
    shutil.copy2(path, keep)
    if mode is not None:
        os.chmod(keep, mode)
    d = os.path.dirname(os.path.abspath(path))
    fd, tmp = tempfile.mkstemp(dir=d, prefix=".convert-")
    os.chmod(tmp, os.stat(path).st_mode & 0o777 if mode is None else mode)
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        f.write(text)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)
    print(f"{path}: converted (original kept as {path}.pre105)")


def rewrite(path, convert):
    with open(path, encoding="utf-8") as f:
        doc = json.load(f)
    new = convert(doc)
    if new is None:
        print(f"{path}: already converted")
        return
    write_beside(path, json.dumps(new, indent=2, ensure_ascii=False) + "\n")


def rewrite_auth(path, _convert):
    with open(path, encoding="utf-8") as f:
        new = convert_auth(f.read())
    if new is None:
        print(f"{path}: already converted")
        return
    # Whoever may write the list may make any key an operator: the daemon refuses one that
    # another user or the group could write.
    write_beside(path, new, mode=0o600)


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--rig", action="append", default=[], help="a rig.json to convert")
    ap.add_argument("--cal", action="append", default=[], help="a calibration store to convert")
    ap.add_argument(
        "--plans", action="append", default=[], help="a plan directory: every *.json in it"
    )
    ap.add_argument(
        "--auth", action="append", default=[], help="an authorized-clients file to convert"
    )
    args = ap.parse_args(argv)
    jobs = [(p, rewrite, convert_rig) for p in args.rig]
    jobs += [(p, rewrite, convert_cal) for p in args.cal]
    jobs += [(p, rewrite_auth, None) for p in args.auth]
    for d in args.plans:
        try:
            names = sorted(n for n in os.listdir(d) if n.endswith(".json"))
        except OSError as e:
            print(f"{d}: {e}", file=sys.stderr)
            return 1
        jobs += [(os.path.join(d, n), rewrite, convert_plan) for n in names]
    if not jobs:
        ap.error("nothing to convert: give --rig, --cal, --plans or --auth PATH")
    failed = False
    for p, run, convert in jobs:
        try:
            run(p, convert)
        except (OSError, ValueError) as e:
            print(f"{p}: {e}", file=sys.stderr)
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
