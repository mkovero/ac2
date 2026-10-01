"""Tracking rule evaluation: consecutive non-overlapping windows through the finder.

    python3 tracking.py [--n N]

For each trial a long measurement is synthesised and cut into back-to-back windows of the
band's observation length; the tracker (finder.track_step) sees one result per window.
Reports: moves to the true first arrival, wrong moves (anything else), windows to first
lock, and how often an ambiguous / refused window blocked a move. A second phase changes
the delay mid-stream to measure re-lock latency.
"""

from __future__ import annotations

import os

os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")  # one BLAS thread per worker process
os.environ.setdefault("OMP_NUM_THREADS", "1")

import argparse
import zlib
import multiprocessing as mp
import pathlib
import sys

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "refgen" / "sets"))
sys.path.insert(0, str(HERE))

import delay_finder as df  # noqa: E402
import finder as F  # noqa: E402

P = df.Path
FS = df.FS
OBS = {"full": 12000, "sub": 192000}
SEARCH = {"full": 2400, "sub": 9600}
AGREE = {"full": 1, "sub": 5}  # samples: +-1 sample full range, +-0.1 ms sub


def windows(sc: df.Scenario, n_win: int):
    lm = OBS[sc.band]
    sc.lm = lm * n_win
    ref, rs, m, ms = df.synth(sc)
    S = sc.search[1]
    for i in range(n_win):
        a = i * lm
        yield ref[a : a + lm + 2 * S], rs + a, m[a : a + lm], ms + a


def trial(args):
    kind, band, i = args
    rng = np.random.default_rng([777, i, zlib.crc32(kind.encode()), len(band)])
    S = SEARCH[band]
    d1 = float(rng.uniform(-0.6 * S, 0.6 * S))
    d2 = d1 + float(rng.choice([-1, 1])) * float(rng.uniform(5, 0.3 * S))
    sep = float(rng.uniform(1, 15)) * FS / 1000
    snr = float(rng.uniform(15, 30))
    if kind == "single":
        paths = lambda d: [P(d, 0.0)]  # noqa: E731
    elif kind == "refl_louder":
        lvl = float(rng.uniform(-9, -3))
        paths = lambda d: [P(d, lvl), P(d + sep, 0.0, -1)]  # noqa: E731
    elif kind == "borderline":
        lvl = float(rng.uniform(-13, -11))
        paths = lambda d: [P(d, lvl), P(d + sep, 0.0)]  # noqa: E731
    else:
        raise ValueError(kind)
    n_win = 6 if band == "full" else 4
    tr = F.TrackState()
    log = []
    for phase, d in enumerate((d1, d2)):
        sc = df.Scenario("t", kind, band, paths(d), excitation="pink", snr_db=snr,
                         search=(-S, S), seed=int(rng.integers(1, 2**31)),
                         meas_start=1_000_000 + phase * 10_000_000)
        truth = sc.truth()["first"]
        for k, (r, rs, m, ms) in enumerate(windows(sc, n_win)):
            res = F.find_delay(r, rs, m, ms, FS, band, sc.search)
            mv = F.track_step(tr, res, ms, ms + len(m), agree=AGREE[band])
            log.append((phase, k, res.status, mv, truth))
    moves = [(ph, k, mv, t) for ph, k, st, mv, t in log if mv is not None]
    tol = F.BANDS[band].tol_s * FS
    good = [x for x in moves if abs(x[2] - x[3]) <= max(tol, 1)]
    wrong = [x for x in moves if abs(x[2] - x[3]) > max(tol, 1)]
    first_lock = {ph: min((k for p_, k, _, _ in good if p_ == ph), default=None) for ph in (0, 1)}
    statuses = [st for _, _, st, _, _ in log]
    return dict(kind=kind, band=band, good=len(good), wrong=len(wrong),
                lock0=first_lock[0], lock1=first_lock[1], windows=len(log),
                amb=statuses.count("ambiguous"), refused=statuses.count("no_estimate"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=60)
    a = ap.parse_args()
    jobs = []
    for band, scale in (("full", 1.0), ("sub", 0.25)):
        for kind in ("single", "refl_louder", "borderline"):
            jobs += [(kind, band, i) for i in range(max(8, int(a.n * scale)))]
    with mp.Pool(max(1, mp.cpu_count() - 1)) as pool:
        rows = pool.map(trial, jobs, chunksize=1)
    print(f"{'band/kind':22s} {'trials':>6s} {'moves':>6s} {'wrong':>6s} {'lock0':>8s} "
          f"{'relock':>8s} {'amb%':>6s} {'ref%':>6s}")
    for band in ("full", "sub"):
        for kind in ("single", "refl_louder", "borderline"):
            g = [r for r in rows if r["band"] == band and r["kind"] == kind]
            nw = sum(r["windows"] for r in g)
            l0 = [r["lock0"] for r in g if r["lock0"] is not None]
            l1 = [r["lock1"] for r in g if r["lock1"] is not None]
            print(f"{band + '/' + kind:22s} {len(g):6d} {sum(r['good'] for r in g):6d} "
                  f"{sum(r['wrong'] for r in g):6d} "
                  f"{(np.mean(l0) + 1 if l0 else float('nan')):8.2f} "
                  f"{(np.mean(l1) + 1 if l1 else float('nan')):8.2f} "
                  f"{100 * sum(r['amb'] for r in g) / nw:6.1f} "
                  f"{100 * sum(r['refused'] for r in g) / nw:6.1f}")
    print("lock0/relock: mean windows until the first correct move (1 = first window; the "
          "rule needs two, so 2 is the minimum); nan = never in this trial set")


if __name__ == "__main__":
    main()
