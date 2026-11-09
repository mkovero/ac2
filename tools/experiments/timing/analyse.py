"""Timing study, phase A: what `ac2 selftest stamps` logged, per run and between two rigs.

    python3 analyse.py <run-dir> [<run-dir-2>] [--window 60] [--decimate 1] [--plot]

Per run (docs/design/timing-study.md §5, Q1):
- the line of `capture_ns` against the block index on the raw stamp clock, one shared slope
  with an intercept per contiguous stretch: device rate (ppm against the stamp clock) and
  the residual's sigma, p99 and max; `residual.csv` beside the run's files;
- wander: the same fit in `--window` second windows, ppm per window (`wander.csv`);
- `pairs.csv`: the stamp clock against CLOCK_MONOTONIC and CLOCK_REALTIME (ppm and the map's
  scatter), and which host clock the stamp clock reads equal to.

With two runs taken at the same time on two hosts (Q2): each block's stamp is mapped to
REALTIME through its own run's pairs (piecewise linear: REALTIME is slewed by chrony, so one
line over an hour would not hold), each device's rate against REALTIME is fitted per window
on a common REALTIME grid, and the rate ratio between the rigs is written to `ratio.csv` in
the first run's directory. Two rigs on one PTP/NTP reference share REALTIME to the
discipline's accuracy, so the ratio is the rigs' audio clocks against each other.

stdlib + numpy; matplotlib only for `--plot`, when it is installed.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

import numpy as np

MAGIC = b"AC2STMP1"
FORMAT_VERSION = 1
RECORD_BYTES = 32
HEADER_BYTES = 16
NO_CAPTURE = np.uint64(0xFFFF_FFFF_FFFF_FFFF)
# BlockFlags: XRUN | DISCONTINUITY | OVERFLOW | CONFIG_CHANGE (ac2-audio block.rs).
BREAKS = 0x02 | 0x04 | 0x08 | 0x10
XRUN = 0x02

RECORD = np.dtype(
    [
        ("start", "<u8"),
        ("frames", "<u4"),
        ("flags", "<u4"),
        ("capture_ns", "<u8"),
        ("callback_ns", "<u8"),
    ]
)


class Run:
    def __init__(self, d: pathlib.Path):
        self.dir = d
        self.meta = json.loads((d / "meta.json").read_text())
        raw = (d / "blocks.bin").read_bytes()
        if raw[:8] != MAGIC:
            sys.exit(f"{d}/blocks.bin: not a stamps log")
        version, rec = np.frombuffer(raw[8:16], "<u4")
        if version != FORMAT_VERSION or rec != RECORD_BYTES:
            sys.exit(f"{d}/blocks.bin: format {version}/{rec}, expected {FORMAT_VERSION}/{RECORD_BYTES}")
        whole = (len(raw) - HEADER_BYTES) // RECORD_BYTES
        self.blocks = np.frombuffer(raw, RECORD, count=whole, offset=HEADER_BYTES)
        self.rate = float(self.meta["sample_rate"])
        pairs = np.loadtxt(d / "pairs.csv", delimiter=",", skiprows=1, dtype=np.int64, ndmin=2)
        self.pairs = pairs  # stamp, mono, mono_bracket, real, real_bracket, raw

    @property
    def name(self) -> str:
        return f"{self.meta.get('host', '?')}:{self.dir.name}"


def segments(b: np.ndarray) -> np.ndarray:
    """Stretch id per block: a new stretch after a flagged break or an index jump."""
    end = b["start"][:-1] + b["frames"][:-1].astype(np.uint64)
    jump = np.concatenate([[False], b["start"][1:] != end])
    brk = (b["flags"] & BREAKS) != 0
    return np.cumsum(jump | brk)


def pooled_fit(x: np.ndarray, y: np.ndarray, seg: np.ndarray):
    """Shared slope, intercept per stretch; x, y float offsets. Returns slope, residuals."""
    _, inv = np.unique(seg, return_inverse=True)
    n = np.bincount(inv).astype(float)
    xm = np.bincount(inv, x) / n
    ym = np.bincount(inv, y) / n
    xc, yc = x - xm[inv], y - ym[inv]
    sxx = float(np.dot(xc, xc))
    if sxx <= 0:
        return None, None
    slope = float(np.dot(xc, yc)) / sxx
    return slope, yc - slope * xc


def stamp_points(r: Run):
    b = r.blocks
    ok = b["capture_ns"] != NO_CAPTURE
    seg = segments(b)[ok]
    s0, c0 = int(b["start"][ok][0]), int(b["capture_ns"][ok][0])
    x = (b["start"][ok].astype(np.int64) - s0).astype(float)
    y = (b["capture_ns"][ok].astype(np.int64) - c0).astype(float)
    return ok, seg, x, y, c0


def ppm_of(slope_ns_per_sample: float, rate: float) -> float:
    return (1e9 / slope_ns_per_sample / rate - 1.0) * 1e6


def describe(r: Run, window_s: float, decimate: int, plot: bool) -> None:
    b = r.blocks
    print(f"== {r.name}: {r.meta['backend']} {r.meta['device']} {r.rate:.0f} Hz, "
          f"{r.meta.get('buffer_frames')} frames, capture latency {r.meta.get('capture_latency_frames')}")
    print(f"   stamp clock {r.meta.get('stamp_clock')} reads as {r.meta.get('stamp_clock_reads_as')}")
    if len(b) == 0:
        print("   no blocks")
        return
    seg_all = segments(b)
    xr = int(np.count_nonzero(b["flags"] & XRUN))
    print(f"   blocks {len(b)}, stretches {int(seg_all[-1]) + 1}, xrun blocks {xr}, "
          f"{(int(b['start'][-1]) - int(b['start'][0])) / r.rate:.1f} s of audio")
    ok, seg, x, y, _ = stamp_points(r)
    if len(x) < 3:
        print("   too few capture stamps")
        return
    slope, res = pooled_fit(x, y, seg)
    a = np.abs(res)
    print(f"   capture_ns line: {ppm_of(slope, r.rate):+.4f} ppm against the stamp clock; "
          f"residual sigma {res.std():.0f} ns, p99 |r| {np.percentile(a, 99):.0f} ns, "
          f"max |r| {a.max():.0f} ns")
    t = y / 1e9
    sel = slice(None, None, max(decimate, 1))
    np.savetxt(r.dir / "residual.csv",
               np.column_stack([b["start"][ok][sel].astype(float), t[sel], res[sel]]),
               delimiter=",", header="start_sample,t_s,residual_ns", comments="", fmt=["%.0f", "%.6f", "%.1f"])

    # Wander: the fit per window of stamp-clock time.
    win = np.floor(t / window_s).astype(np.int64)
    rows = []
    for w in np.unique(win):
        m = win == w
        if m.sum() < 10:
            continue
        s, rr = pooled_fit(x[m], y[m], seg[m])
        if s is None:
            continue
        rows.append(((w + 0.5) * window_s, ppm_of(s, r.rate), rr.std(), int(m.sum())))
    if rows:
        wd = np.array(rows)
        np.savetxt(r.dir / "wander.csv", wd, delimiter=",", header="t_mid_s,ppm,residual_sigma_ns,blocks",
                   comments="", fmt=["%.1f", "%.4f", "%.1f", "%d"])
        p = wd[:, 1]
        print(f"   wander over {len(p)} x {window_s:.0f} s windows: ppm {p.mean():+.4f} mean, "
              f"{p.std():.4f} sd, {p.max() - p.min():.4f} peak-to-peak")

    if len(r.pairs) >= 2:
        pr = r.pairs
        st = (pr[:, 0] - pr[0, 0]).astype(float)
        for name, col in (("CLOCK_MONOTONIC", 1), ("CLOCK_REALTIME", 3)):
            hy = (pr[:, col] - pr[0, col]).astype(float)
            s, rr = pooled_fit(st, hy, np.zeros(len(st), dtype=np.int64))
            if s is not None:
                print(f"   pairs: {name} runs {(s - 1) * 1e6:+.4f} ppm against the stamp clock, "
                      f"map scatter {rr.std():.0f} ns ({len(pr)} pairs)")
        print(f"   stamp - MONOTONIC_RAW median {np.median(pr[:, 0] - pr[:, 5]):.0f} ns, "
              f"stamp - MONOTONIC median {np.median(pr[:, 0] - pr[:, 1]):.0f} ns")
    end = r.meta.get("end") or {}
    if "pairs" in end:
        print(f"   pairs kept {end['pairs']['kept']}, dropped {end['pairs']['dropped']}")

    if plot:
        try:
            import matplotlib

            matplotlib.use("Agg")
            import matplotlib.pyplot as plt
        except ImportError:
            print("   (matplotlib not installed: no plot)")
            return
        fig, ax = plt.subplots(2, 1, figsize=(10, 6), sharex=True)
        ax[0].plot(t[sel], res[sel] / 1e3, ",")
        ax[0].set_ylabel("residual, µs")
        if rows:
            ax[1].plot(wd[:, 0], wd[:, 1], ".-")
        ax[1].set_ylabel("ppm per window")
        ax[1].set_xlabel("stamp clock, s")
        fig.suptitle(r.name)
        fig.savefig(r.dir / "residual.png", dpi=120)


def to_realtime(r: Run, epoch_ns: int):
    """Capture stamps mapped to REALTIME (s after `epoch_ns`) through the run's pairs."""
    ok, seg, x, _, _ = stamp_points(r)
    pr = r.pairs
    p0 = int(pr[0, 0])
    st = (pr[:, 0] - p0).astype(float)
    rt = (pr[:, 3] - epoch_ns).astype(float)
    cap = (r.blocks["capture_ns"][ok].astype(np.int64) - p0).astype(float)
    inside = (cap >= st[0]) & (cap <= st[-1])
    return np.interp(cap[inside], st, rt) / 1e9, x[inside], seg[inside]


def ratio(a: Run, b: Run, window_s: float) -> None:
    if len(a.pairs) < 2 or len(b.pairs) < 2:
        print("ratio: a run has no pairs to map its stamps to REALTIME")
        return
    epoch = int(min(a.pairs[0, 3], b.pairs[0, 3]))
    per = []
    for r in (a, b):
        t, x, seg = to_realtime(r, epoch)
        win = np.floor(t / window_s).astype(np.int64)
        d = {}
        for w in np.unique(win):
            m = win == w
            if m.sum() < 10:
                continue
            # Samples against REALTIME seconds: the slope is the device rate in Hz.
            s, _ = pooled_fit(t[m] * 1e9, x[m], seg[m])
            if s is not None:
                d[int(w)] = (s * 1e9 / r.rate - 1.0) * 1e6
        per.append(d)
    common = sorted(set(per[0]) & set(per[1]))
    if not common:
        print("ratio: the runs share no REALTIME window (were they concurrent?)")
        return
    rows = [
        ((w + 0.5) * window_s, per[0][w], per[1][w], ((1 + per[0][w] * 1e-6) / (1 + per[1][w] * 1e-6) - 1) * 1e6)
        for w in common
    ]
    arr = np.array(rows)
    np.savetxt(a.dir / "ratio.csv", arr, delimiter=",", comments="",
               header=f"t_mid_s_after_{epoch},ppm_{a.name},ppm_{b.name},ratio_ppm",
               fmt=["%.1f", "%.4f", "%.4f", "%.4f"])
    q = arr[:, 3]
    trend = np.polyfit(arr[:, 0] / 3600.0, q, 1)[0] if len(q) > 1 else float("nan")
    print(f"== ratio {a.name} / {b.name} against REALTIME over {len(q)} x {window_s:.0f} s windows: "
          f"{q.mean():+.4f} ppm mean, {q.std():.4f} sd, trend {trend:+.4f} ppm/h "
          f"(written to {a.dir / 'ratio.csv'})")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("runs", nargs="+", type=pathlib.Path)
    ap.add_argument("--window", type=float, default=60.0, help="wander/ratio window, s")
    ap.add_argument("--decimate", type=int, default=1, help="keep every Nth point in residual.csv")
    ap.add_argument("--plot", action="store_true", help="residual.png per run (needs matplotlib)")
    a = ap.parse_args()
    if len(a.runs) > 2:
        ap.error("one or two run directories")
    runs = [Run(d) for d in a.runs]
    for r in runs:
        describe(r, a.window, a.decimate, a.plot)
    if len(runs) == 2:
        ratio(runs[0], runs[1], a.window)


if __name__ == "__main__":
    main()
