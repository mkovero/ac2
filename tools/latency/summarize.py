#!/usr/bin/env python3
"""Summarise an `ac2-ui --latency-log FILE` CSV: per-stage p50/p95/max in ms.

    python3 -I tools/latency/summarize.py run.csv [--view spectrum_3d]

Stages of the newest data frame each pane showed (format: docs/design/latency-study.md §2):

  capture->read    read_wall_ns - capture_wall_ns   (daemon clock vs this host's: chrony offset)
  read->apply      apply_ns - read_ns               (link drain hold + reducer wait)
  apply->build     scene_ns - apply_ns              (first pass that drew the frame)
  build->present   submit_ns - scene_ns             (rest of the pass, tessellation, acquire)
  glide lag        glide_lag_ms                     (what is drawn trails the newest frame)
  capture->present submit_wall_ns - capture_wall_ns (first presented frame showing it)
  ... as seen      the same plus the glide lag, every presented frame

capture->read and read->apply count each data frame once; the others count each frame on
its first presented pass, except the glide lag and "as seen", which count every pass. The
3D spectrograph's slot advance per presented frame is printed as a histogram.
Standard library only.
"""

import argparse
import csv
import sys
from collections import Counter


def rows(path):
    with open(path, newline="") as f:
        lines = (l for l in f if not l.startswith("#"))
        yield from csv.DictReader(lines)


def num(r, c):
    v = r.get(c, "")
    return float(v) if v not in ("", None) else None


def pct(xs, p):
    s = sorted(xs)
    if not s:
        return float("nan")
    k = (len(s) - 1) * p / 100.0
    lo = int(k)
    hi = min(lo + 1, len(s) - 1)
    return s[lo] + (s[hi] - s[lo]) * (k - lo)


def line(name, xs):
    if not xs:
        print(f"  {name:<18} (no samples)")
        return
    print(
        f"  {name:<18} n={len(xs):>6}  p50 {pct(xs, 50):8.2f}  p95 {pct(xs, 95):8.2f}"
        f"  max {max(xs):8.2f} ms"
    )


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("csv")
    ap.add_argument("--view", help="only panes drawing this view (spectrum, spectrum_3d, ...)")
    a = ap.parse_args()

    per_frame = {}  # (topic, incarnation, seq) -> first row showing it
    seen = []  # every row with a data frame and a probe
    passes = {}  # pass -> (pass_start_ns, ui_end_ns, frame_cpu_ms)
    build = {}  # view -> build ms of built scenes
    slots = Counter()
    for r in rows(a.csv):
        p = int(r["pass"])
        passes.setdefault(p, (num(r, "pass_start_ns"), num(r, "ui_end_ns"), num(r, "frame_cpu_ms")))
        view = r["view"]
        if a.view and view != a.view:
            continue
        if r["scene"] == "built" and view:
            build.setdefault(view, []).append(num(r, "build_ms"))
        if not r["seq"]:
            continue
        if view == "spectrum_3d" and r["slot_advance"] and num(r, "submit_ns") is not None:
            slots[int(r["slot_advance"])] += 1
        key = (r["topic"], r["incarnation"], r["seq"])
        if key not in per_frame and num(r, "submit_ns") is not None:
            per_frame[key] = r
        if num(r, "submit_ns") is not None:
            seen.append(r)

    ms = 1e-6
    def diffs(rs, a_col, b_col):
        out = []
        for r in rs:
            x, y = num(r, a_col), num(r, b_col)
            if x is not None and y is not None:
                out.append((y - x) * ms)
        return out

    first = list(per_frame.values())
    print(f"{a.csv}: {len(passes)} passes, {len(per_frame)} data frames shown")
    print("stages (ms):")
    line("capture->read", diffs(first, "capture_wall_ns", "read_wall_ns"))
    line("read->apply", diffs(first, "read_ns", "apply_ns"))
    line("apply->build", diffs(first, "apply_ns", "scene_ns"))
    line("build->present", diffs(first, "scene_ns", "submit_ns"))
    line("glide lag", [num(r, "glide_lag_ms") for r in seen if num(r, "glide_lag_ms") is not None])
    line("capture->present", diffs(first, "capture_wall_ns", "submit_wall_ns"))
    line(
        "... as seen",
        [
            (num(r, "submit_wall_ns") - num(r, "capture_wall_ns")) * ms + num(r, "glide_lag_ms")
            for r in seen
            if num(r, "submit_wall_ns") is not None and num(r, "glide_lag_ms") is not None
        ],
    )

    starts = sorted(v[0] for v in passes.values() if v[0] is not None)
    print("passes (ms):")
    line("pass interval", [(b - a) * ms for a, b in zip(starts, starts[1:])])
    line("ui pass", [(e - s) * ms for s, e, _ in passes.values() if s is not None and e is not None])
    line("frame cpu", [c for _, _, c in passes.values() if c is not None])
    for v, xs in sorted(build.items()):
        line(f"build {v}", xs)

    if slots:
        print("3D slot advance per presented frame:")
        n = sum(slots.values())
        for k in sorted(slots):
            print(f"  {k:>3} slots  {slots[k]:>6}  {100.0 * slots[k] / n:5.1f} %  " + "#" * round(40 * slots[k] / n))
    return 0


if __name__ == "__main__":
    sys.exit(main())
