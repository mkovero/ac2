"""Monte-Carlo evaluation of the Q1 finder prototype over randomised scenario classes.

    python3 mc.py [--n N] [--only CLASS,...] [--set key=value ...] [--out results/x.json]

Scores each trial as accepted-correct, accepted-wrong (|error| > band tolerance; split into
wrong-arrival = nearer another path, and imprecise), ambiguous (with/without the true
arrival among the <= 3 listed candidates), or refused. Prints one table row per class.
"""

from __future__ import annotations

import os

os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")  # one BLAS thread per worker process
os.environ.setdefault("OMP_NUM_THREADS", "1")

import argparse
import json
import zlib
import math
import multiprocessing as mp
import pathlib
import sys
import time
from dataclasses import asdict, replace

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "refgen" / "sets"))
sys.path.insert(0, str(HERE))

import delay_finder as df  # noqa: E402
import finder as F  # noqa: E402

P = df.Path
FS = df.FS

# observation / search per band for the experiments (search kept small for speed; the
# +-1 s default is exercised by its own class)
OBS = {"full": 12000, "mid": 24000, "sub": 192000}
SEARCH = {"full": 2400, "mid": 4800, "sub": 9600}
PULSE_W = {"full": 3.8, "mid": 20.0, "sub": 536.0}  # nominal -6 dB envelope width, samples


def tol_samples(band):
    return F.BANDS[band].tol_s * FS


def _direct_delay(rng, band, extra=0.0):
    s = SEARCH[band]
    lo, hi = -0.8 * s, 0.8 * s - extra
    return float(rng.uniform(lo, hi))


def two_path(rng, band, sep_ms=(0.2, 20.0), ld=(-20.0, 10.0), exc=("pink", "white"),
             snr=(20.0, 40.0)):
    sep = math.exp(rng.uniform(math.log(sep_ms[0]), math.log(sep_ms[1]))) * FS / 1000.0
    d = _direct_delay(rng, band, sep)
    l_d = float(rng.uniform(*ld))
    pol = int(rng.choice([-1, 1]))
    paths = [P(d, l_d), P(d + sep, 0.0, pol)]
    return dict(paths=paths, excitation=str(rng.choice(exc)), snr_db=float(rng.uniform(*snr)),
                meta=dict(sep=sep, sep_w=sep / PULSE_W[band], l_d=l_d, pol=pol))


def single(rng, band, exc=("pink", "white"), snr=(20.0, 40.0)):
    d = _direct_delay(rng, band)
    return dict(paths=[P(d, 0.0)], excitation=str(rng.choice(exc)),
                snr_db=float(rng.uniform(*snr)), meta={})


def room(rng, band, exc=("pink", "white"), snr=(20.0, 40.0)):
    """Direct + 3-6 discrete reflections + (implicitly) noise: a room-like early response."""
    d = _direct_delay(rng, band, 25.0 * FS / 1000)
    l_d = float(rng.uniform(-8.0, 3.0))
    paths = [P(d, l_d)]
    for _ in range(int(rng.integers(3, 7))):
        sep = float(rng.uniform(0.5, 25.0)) * FS / 1000
        paths.append(P(d + sep, float(rng.uniform(-15.0, 0.0)), int(rng.choice([-1, 1]))))
    # the strongest reflection is at most 0 dB; the direct is the reference
    return dict(paths=paths, excitation=str(rng.choice(exc)), snr_db=float(rng.uniform(*snr)),
                meta=dict(l_d=l_d))


def periodic(rng, band, **kw):
    d = _direct_delay(rng, band)
    per = 8192
    # a late reflection whose image (delay - period) lands inside the search range, early
    late = float(rng.uniform(per - 0.8 * SEARCH[band] - d, per - 10 - d)) if d < per else 0.0
    return dict(paths=[P(d, 0.0), P(d + late, float(rng.uniform(-10, 0)))], excitation="pink",
                snr_db=30.0, period=per, meta=dict(late=late))


def large(rng, band, **kw):
    d = float(rng.uniform(-0.9, 0.9)) * FS
    return dict(paths=[P(d, -3.0), P(d + float(rng.uniform(0.5, 20)) * FS / 1000, 0.0)],
                excitation="pink", snr_db=float(rng.uniform(20, 40)), search=48000, meta={})


def nosignal(rng, band, **kw):
    d = _direct_delay(rng, band)
    return dict(paths=[P(d, 0.0)], excitation="pink", snr_db=-60.0, meta={})


CLASSES = {}


def cls(name, band, gen, n_scale=1.0, **kw):
    CLASSES[name] = (band, gen, kw, n_scale)


for b in ("full", "mid", "sub"):
    ns = 0.3 if b == "sub" else 1.0
    cls(f"{b}/single", b, single, ns)
    cls(f"{b}/two_path", b, two_path, 3 * ns)
    # resolved pairs: separation 2.5-8 nominal pulse widths
    cls(f"{b}/two_path_resolved", b, two_path, ns,
        sep_ms=(2.5 * PULSE_W[b] / 48.0, 8 * PULSE_W[b] / 48.0))
    cls(f"{b}/room", b, room, ns)
cls("full/periodic_wrap", "full", periodic, 0.5)
# direct sound beyond the first refinement window (+-21 ms) before a louder reflection
cls("full/two_path_far", "full", two_path, 0.5, sep_ms=(25.0, 45.0), ld=(-10.0, 0.0))
cls("full/large_delay", "full", large, 0.3)
cls("full/no_signal", "full", nosignal, 0.3)
cls("sub/no_signal", "sub", nosignal, 0.2)
for e in ("white", "pink", "bandlimited", "music", "music_hf"):
    cls(f"full/exc_{e}", "full", two_path, 1.0, exc=(e,), ld=(-10.0, 0.0), sep_ms=(0.5, 20.0))
for e in ("pink", "music", "subonly"):
    cls(f"sub/exc_{e}", "sub", single, 0.3, exc=(e,))
cls("full/exc_narrow", "full", single, 0.3, exc=("narrow",))
for s in (-10, 0, 10, 20, 30, 40):
    cls(f"full/snr_{s:+d}", "full", two_path, 0.5, snr=(s, s), ld=(-10.0, 0.0), sep_ms=(0.5, 20.0))
    cls(f"sub/snr_{s:+d}", "sub", single, 0.3, snr=(s, s))


def make(name, i, seed0=12345):
    band, gen, kw, _ = CLASSES[name]
    rng = np.random.default_rng([seed0, zlib.crc32(name.encode()), i])
    spec = gen(rng, band, **kw)
    S = spec.get("search", SEARCH[band])
    sc = df.Scenario(name=f"{name}#{i}", cls=name, band=band, paths=spec["paths"],
                     excitation=spec["excitation"], snr_db=spec["snr_db"], lm=OBS[band],
                     search=(-S, S), seed=int(rng.integers(1, 2**31)),
                     period=spec.get("period"))
    return sc, spec["meta"]


def score(sc, res, tol):
    tr = sc.truth()
    first = tr["first"]
    g = max(p.gain_db for p in sc.paths)
    # paths within +-2 dB of the threshold make both answers acceptable
    acceptable = [first] + [p.delay for p in sc.paths
                            if abs(p.gain_db - g - df.THRESHOLD_DB) <= 2.0 and p.delay < first]
    if res.status == "no_estimate":
        return "refused", None
    if res.status == "ambiguous":
        ok = any(abs(c.delay - a) <= tol for c in res.listed for a in acceptable)
        return ("amb_listed" if ok else "amb_missing"), None
    errs = [res.delay - a for a in acceptable]
    err = min(errs, key=abs)
    if abs(err) <= tol:
        return "correct", err
    # nearer another path than the target?
    others = [p.delay for p in sc.paths if p.delay not in acceptable]
    if others and min(abs(res.delay - o) for o in others) < abs(err):
        return "wrong_arrival", err
    return "imprecise", err


def run_one(args):
    name, i, pdict, obs = args
    OBS.update(obs)
    sc, meta = make(name, i)
    p = F.Params(**pdict)
    ref, rs, m, ms = df.synth(sc)
    res = F.find_delay(ref, rs, m, ms, sc.fs, sc.band, sc.search, p)
    out, err = score(sc, res, tol_samples(sc.band))
    tol = tol_samples(sc.band)
    lo = min(q.delay for q in sc.paths) - tol
    hi = max(q.delay for q in sc.paths) + tol
    span_ok = res.status == "accepted" and lo <= res.delay <= hi
    tr = sc.truth()
    mm = res.pick.mismatch if res.pick else float("nan")
    unc = res.pick.uncertainty if res.pick else float("nan")
    err_raw = (res.delay - sc.truth()["first"]) if res.pick else float("nan")
    return dict(cls=name, i=i, outcome=out, err=err, span_ok=span_ok, reasons=res.reasons, psr=res.psr_db,
                psr_acq=res.psr_acq_db, mismatch=mm, unc=unc, err_raw=err_raw, status=res.status,
                pick_level=res.pick.level_db if res.pick else float("nan"),
                snr=res.band_snr_db, exc=res.excited_frac, snr_true=sc.snr_db,
                first_level=tr["first_level_db"], n_paths=len(sc.paths), **meta)


def summarise(rows, by="cls"):
    groups = {}
    for r in rows:
        groups.setdefault(r[by], []).append(r)
    table = []
    for k in sorted(groups):
        g = groups[k]
        n = len(g)
        c = lambda o: sum(1 for r in g if r["outcome"] == o)  # noqa: E731
        acc = c("correct") + c("wrong_arrival") + c("imprecise")
        errs = np.array([abs(r["err"]) for r in g if r["outcome"] == "correct"])
        span = sum(1 for r in g if r.get("span_ok"))
        table.append(dict(key=k, n=n, accepted=acc / n, correct=c("correct") / n,
                          wrong=(c("wrong_arrival") + c("imprecise")) / n,
                          wrong_arrival=c("wrong_arrival") / n, imprecise=c("imprecise") / n,
                          wrong_of_accepted=(acc - c("correct")) / acc if acc else 0.0,
                          ambiguous=(c("amb_listed") + c("amb_missing")) / n,
                          amb_missing=c("amb_missing") / n, refused=c("refused") / n,
                          span_of_accepted=span / acc if acc else float("nan"),
                          err_p50=float(np.median(errs)) if len(errs) else float("nan"),
                          err_p95=float(np.percentile(errs, 95)) if len(errs) else float("nan"),
                          err_max=float(errs.max()) if len(errs) else float("nan")))
    return table


def print_table(table, title=""):
    if title:
        print(f"\n## {title}")
    print(f"{'class':28s} {'n':>5s} {'acc':>6s} {'ok':>6s} {'wrong':>6s} {'w/acc':>6s} "
          f"{'amb':>6s} {'ambX':>5s} {'ref':>6s} {'e50':>6s} {'e95':>6s} {'emax':>6s} {'span':>6s}")
    for t in table:
        print(f"{t['key']:28s} {t['n']:5d} {t['accepted']:6.1%} {t['correct']:6.1%} "
              f"{t['wrong']:6.1%} {t['wrong_of_accepted']:6.1%} {t['ambiguous']:6.1%} "
              f"{t['amb_missing']:5.1%} {t['refused']:6.1%} {t['err_p50']:6.2f} "
              f"{t['err_p95']:6.2f} {t['err_max']:6.2f} {t['span_of_accepted']:6.1%}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=200)
    ap.add_argument("--only", default="")
    ap.add_argument("--set", nargs="*", default=[])
    ap.add_argument("--out", default="")
    ap.add_argument("--obs", nargs="*", default=[], help="band=samples")
    ap.add_argument("--jobs", type=int, default=max(1, mp.cpu_count() - 1))
    a = ap.parse_args()
    pdict = {}
    for kv in a.set:
        k, v = kv.split("=")
        cur = getattr(F.Params(), k)
        pdict[k] = (v.lower() in ("1", "true", "yes")) if isinstance(cur, bool) else type(cur)(v)
    for kv in a.obs:
        k, v = kv.split("=")
        OBS[k] = int(v)
    names = [n for n in CLASSES if not a.only or any(n.startswith(o) for o in a.only.split(","))]
    jobs = []
    for name in names:
        n = max(10, int(a.n * CLASSES[name][3]))
        jobs += [(name, i, pdict, dict(OBS)) for i in range(n)]
    t0 = time.time()
    with mp.Pool(a.jobs) as pool:
        rows = pool.map(run_one, jobs, chunksize=4)
    print(f"{len(rows)} trials in {time.time() - t0:.0f} s; params {pdict}")
    print_table(summarise(rows), "per class")
    if a.out:
        pathlib.Path(a.out).parent.mkdir(parents=True, exist_ok=True)
        pathlib.Path(a.out).write_text(json.dumps(dict(params=pdict, rows=rows), indent=0,
                                                  default=float))
    return rows


if __name__ == "__main__":
    main()
