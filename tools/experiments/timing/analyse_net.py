"""Timing study, phase C: what the receiving side of `ac2 selftest net` logged.

    python3 analyse_net.py <run-dir> [--window 60] [--bin-us 50] [--worst 5]

Reads `arrivals.bin` and `meta.json` (formats in crates/ac2-cli/src/selftest/net.rs) and,
as the command's own summary does (docs/design/timing-study.md §5, Q6):
- one-way delay `arrived − sent` above the run's smallest (the two hosts' clock offset
  drops out with it), p50 / p99 / p99.9 / max;
- `arrived − scheduled` above its smallest (adds the sender's own lateness);
- arrival-interval jitter: |interval − the blocks' scheduled spacing|;
- missing (never read) and reordered blocks;
- the suggested link offset, one-way p99.9 plus one block period.

The same per `--window` seconds of the sender's schedule into `per_minute.csv`, a histogram
of the one-way delay above the floor into `histogram.csv` (both beside the run's files), and
the worst windows by one-way p99.9 printed.

stdlib + numpy.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

import numpy as np

MAGIC = b"AC2NETA1"
FORMAT_VERSION = 1
RECORD_BYTES = 32
HEADER_BYTES = 16

RECORD = np.dtype(
    [
        ("seq", "<u8"),
        ("scheduled_ns", "<u8"),
        ("sent_ns", "<u8"),
        ("arrived_ns", "<u8"),
    ]
)


def load(d: pathlib.Path):
    meta = json.loads((d / "meta.json").read_text())
    raw = (d / "arrivals.bin").read_bytes()
    if raw[:8] != MAGIC:
        sys.exit(f"{d}/arrivals.bin: not a selftest net log")
    version, rec = np.frombuffer(raw[8:16], "<u4")
    if version != FORMAT_VERSION or rec != RECORD_BYTES:
        sys.exit(f"{d}/arrivals.bin: format {version}/{rec}, expected {FORMAT_VERSION}/{RECORD_BYTES}")
    body = raw[HEADER_BYTES:]
    n = len(body) // RECORD_BYTES
    a = np.frombuffer(body[: n * RECORD_BYTES], RECORD)
    return meta, a


def q(v: np.ndarray) -> tuple[float, float, float, float]:
    """Nearest-rank p50, p99, p99.9 and max, µs, of ns values."""
    if v.size == 0:
        return (float("nan"),) * 4
    s = np.sort(v)
    n = s.size

    def at(p: float) -> float:
        rank = int(np.ceil(p * n))
        return s[min(max(rank, 1), n) - 1] / 1e3

    return at(0.5), at(0.99), at(0.999), s[-1] / 1e3


def fmt(label: str, t) -> str:
    return f"  {label:<17}p50 {t[0]:.0f} µs, p99 {t[1]:.0f} µs, p99.9 {t[2]:.0f} µs, max {t[3]:.0f} µs"


def interval_jitter(a: np.ndarray, period_ns: float) -> np.ndarray:
    """|interval − scheduled spacing| between consecutive arrivals with rising seq, ns."""
    seq = a["seq"].astype(np.int64)
    arr = a["arrived_ns"].astype(np.int64)
    keep = np.concatenate(([True], seq[1:] > np.maximum.accumulate(seq)[:-1]))
    seq, arr = seq[keep], arr[keep]
    if seq.size < 2:
        return np.zeros(0, np.int64)
    return np.abs(np.diff(arr) - np.diff(seq) * period_ns)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("run", type=pathlib.Path, help="the receiver's --out directory")
    ap.add_argument("--window", type=float, default=60.0, help="seconds per row of per_minute.csv")
    ap.add_argument("--bin-us", type=float, default=50.0, help="histogram bin width, µs")
    ap.add_argument("--worst", type=int, default=5, help="worst windows to print")
    args = ap.parse_args()

    meta, a = load(args.run)
    if a.size == 0:
        sys.exit("no blocks were received")
    rate, frames = meta["sample_rate"], meta["frames"]
    period_ns = frames * 1e9 / rate
    one_way = a["arrived_ns"].astype(np.int64) - a["sent_ns"].astype(np.int64)
    vs_sched = a["arrived_ns"].astype(np.int64) - a["scheduled_ns"].astype(np.int64)
    floor = one_way.min()
    above = one_way - floor
    above_sched = vs_sched - vs_sched.min()

    seq = a["seq"].astype(np.int64)
    ended = (meta.get("received") or {}).get("sender") or {}
    total = max(int(ended.get("blocks", 0)), int(seq.max()) + 1)
    reordered = int(np.sum(seq[1:] < np.maximum.accumulate(seq)[:-1]))
    missing = total - a.size

    q_one = q(above)
    print(f"{args.run}: {meta.get('host', '?')} receiving from {meta.get('peer_host', '?')}, "
          f"{rate} Hz × {frames} frames × {meta['channels']} ch, block {period_ns / 1e3:.0f} µs")
    print(f"  blocks           {a.size} read of {total}, {missing} missing, {reordered} reordered")
    print(f"  one-way floor    {floor / 1e3:.0f} µs (holds the hosts' clock offset)")
    print(fmt("one-way jitter", q_one))
    print(fmt("vs schedule", q(above_sched)))
    print(fmt("interval jitter", q(interval_jitter(a, period_ns))))
    print(f"  link offset      {(q_one[2] * 1e3 + period_ns) / 1e6:.2f} ms (one-way p99.9 + one block)")

    # Windows on the sender's schedule, so a window's expected count is exact.
    w_blocks = max(1, int(round(args.window * rate / frames)))
    win = seq // w_blocks
    rows = []
    for w in range(int(total - 1) // w_blocks + 1):
        m = win == w
        sub = a[m]
        expected = min(total, (w + 1) * w_blocks) - w * w_blocks
        p50, p99, p999, mx = q(above[m])
        ij = q(interval_jitter(sub, period_ns))
        rows.append((w * args.window, expected, sub.size, expected - sub.size, p50, p99, p999, mx, ij[2], ij[3]))
    arr = np.array(rows, dtype=float)
    out = args.run / "per_minute.csv"
    np.savetxt(out, arr, delimiter=",", comments="", fmt="%.6g",
               header="t_start_s,expected,received,missing,one_way_p50_us,one_way_p99_us,"
                      "one_way_p999_us,one_way_max_us,interval_jitter_p999_us,interval_jitter_max_us")
    print(f"  per window       {len(rows)} × {args.window:.0f} s → {out}")

    edges = np.arange(0.0, above.max() / 1e3 + args.bin_us, args.bin_us)
    if edges.size < 2:
        edges = np.array([0.0, args.bin_us])
    counts, edges = np.histogram(above / 1e3, bins=edges)
    hist = np.column_stack((edges[:-1], edges[1:], counts))
    hout = args.run / "histogram.csv"
    np.savetxt(hout, hist, delimiter=",", comments="", fmt=["%.6g", "%.6g", "%d"],
               header="from_us,to_us,blocks")
    print(f"  histogram        {counts.size} bins of {args.bin_us:.0f} µs → {hout}")

    order = np.argsort(-np.nan_to_num(arr[:, 6], nan=-1.0))[: args.worst]
    print(f"  worst windows by one-way p99.9:")
    for i in order:
        r = arr[i]
        print(f"    t={r[0]:>7.0f} s  p99.9 {r[6]:.0f} µs, max {r[7]:.0f} µs, "
              f"{int(r[3])} missing, interval jitter max {r[9]:.0f} µs")


if __name__ == "__main__":
    main()
