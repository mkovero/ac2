"""Coherence estimator study: repeated noise fixtures through ac2 and OSM against the
finite-average expectation of each estimator.

γ̂² from n averages reads above the true γ² by about (1 − γ²)²/n (Carter, Knapp & Nuttall),
so a mean over a band is judged against E[γ̂²] at each analyser's effective average count,
not against the truth. Two models are under test:
- ac2: the MTW ladder's own model effective averages per column (`ac2_column_model`, a port
  of ac2-core's `OverlapModel` with the ladder's stages, depth matching and column plan);
  the count varies with the bins per column and the stage serving it.
- OSM: 21 overlapped ticks as Welch's equivalent count (`osm.welch_neff`).
Each condition runs over several independent noise seeds, so a residual is read against the
seed-to-seed scatter, not one draw."""
from __future__ import annotations

import json
import math
import tempfile
from pathlib import Path

import numpy as np

from . import osm, osm_fixtures, wav

# ac2-core mtw::layout: the ladder's fixed shape
NFFT = 4096
DECIMATED_TARGETS_HZ = (12_000.0, 4_000.0)
DESIGN_PPO = 48
SERVED_FRACTION = 0.45
BLEND_OCT = 1.0 / 3.0
D_MAX = 16  # Hann bin correlations beyond this lag are < 1e-6


def _hann(n: int) -> np.ndarray:
    # periodic, as ac2's Window::Hann
    return 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(n) / n)


class OverlapModel:
    """Squared correlation ρ(m, d) of Hann DFT coefficients at block lag m and bin lag d for
    white input; a K-bin, uniformly weighted mean of `held` blocks then carries
    N_eff = (held·K)² / Σ_{i,j} Σ_{k,l} ρ(i − j, k − l) independent averages."""

    def __init__(self, n: int, hop: int):
        w = _hann(n)
        e2 = float(np.sum(w * w))
        lags = -(-n // hop)
        self.rho = np.zeros((lags, D_MAX + 1))
        for m in range(lags):
            s = m * hop
            p = np.zeros(n)
            p[: n - s] = w[: n - s] * w[s:]
            self.rho[m] = np.abs(np.fft.fft(p)[: D_MAX + 1]) ** 2 / (e2 * e2)

    def _bin_sum(self, m: int, k: int) -> float:
        s = k * self.rho[m, 0]
        for d in range(1, min(k, D_MAX + 1)):
            s += 2.0 * (k - d) * self.rho[m, d]
        return s

    def neff_fifo(self, held: int, bins: int) -> float:
        if held == 0 or bins == 0:
            return 0.0
        den = 0.0
        for m in range(min(held, len(self.rho))):
            c = held - m
            den += (c if m == 0 else 2.0 * c) * self._bin_sum(m, bins)
        return (held * bins) ** 2 / den

    def matched_blocks(self, target: float) -> int:
        n = 1
        while self.neff_fifo(n, 1) < target * (1 - 1e-9) and n < 1 << 20:
            n += 1
        return n


def ac2_ladder(fs: float, blocks: int) -> list[dict]:
    """Stages of ac2's MTW at `fs` with FIFO `blocks` on the full-rate stage and the default
    equal-confidence depth: each deeper stage holds the fewest blocks reaching the full-rate
    stage's single-bin effective count."""
    kappa = 1.0 / (2 ** (0.5 / DESIGN_PPO) - 2 ** (-0.5 / DESIGN_PPO))
    half_col = 2 ** (0.5 / DESIGN_PPO)
    blend = 2 ** BLEND_OCT
    factors = [1] + [max(1, round(fs / t)) for t in DECIMATED_TARGETS_HZ]
    stages: list[dict] = []
    for b, m in enumerate(factors):
        rate = fs / m
        hop = NFFT >> (b + 1)
        bin_hz = rate / NFFT
        model = OverlapModel(NFFT, hop)
        if b == 0:
            served, xo = SERVED_FRACTION * rate, None
            held = blocks
            target = model.neff_fifo(blocks, 1)
        else:
            lo = kappa * stages[-1]["bin_hz"]
            xo = (lo, lo * blend)
            served = lo * blend * half_col
            held = model.matched_blocks(target)
        stages.append({"rate": rate, "hop": hop, "bin_hz": bin_hz, "served": served, "xo": xo, "held": held,
                       "model": model})
    return stages


def _bins(st: dict, lo: float, hi: float) -> int:
    k_lo = max(math.ceil(lo / st["bin_hz"]), 1)
    top = math.floor(st["served"] / st["bin_hz"])
    k_hi = min(math.ceil(hi / st["bin_hz"]), top + 1)
    return max(k_hi - k_lo, 0)


def ac2_column_model(fs: float, blocks: int, fc: np.ndarray) -> list[list[tuple[float, float]]]:
    """Per column (48 ppo centres `fc`): the (weight, effective averages) of each stage whose
    γ² goes into it. Inside a crossover ac2 shows a weighted blend of two stages' γ², so its
    expectation is the same blend of the two expectations."""
    st = ac2_ladder(fs, blocks)
    half = 2 ** (0.5 / DESIGN_PPO)
    out = []
    for f in fc:
        lo, hi = f / half, f * half
        if f > st[0]["served"]:
            out.append([])
            continue
        b = 0
        parts = None
        while b + 1 < len(st):
            xlo, xhi = st[b + 1]["xo"]
            if f >= xhi:
                break
            if f > xlo:
                t = math.log(f / xlo) / math.log(xhi / xlo)
                w = 0.5 - 0.5 * math.cos(math.pi * t)
                kd, ks = _bins(st[b + 1], lo, hi), _bins(st[b], lo, hi)
                parts = [(p, st[s]["model"].neff_fifo(st[s]["held"], k))
                         for p, s, k in ((1 - w, b + 1, kd), (w, b, ks)) if k > 0]
                if len(parts) == 1:
                    parts = [(1.0, parts[0][1])]
                break
            b += 1
        if parts is None:
            k = _bins(st[b], lo, hi)
            parts = [(1.0, st[b]["model"].neff_fifo(st[b]["held"], k))] if k > 0 else []
        out.append(parts)
    return out


def ac2_expected_g2(g2: float, parts: list[list[tuple[float, float]]]) -> np.ndarray:
    return np.array([sum(w * osm.expected_g2(g2, n) for w, n in p) if p else np.nan for p in parts])


SNRS = (20.0, 10.0, 3.0, 0.0, -3.0)
# (label, ac2 --blocks, OSM --fft): OSM's coherence count changes with its FFT size (the tick
# hop is fixed), ac2's with its FIFO depth
SETTINGS = (("A", 8, 16), ("B", 32, 14))
SUBBANDS = ((1000.0, 2100.0), (2100.0, 20000.0))  # around ac2's 12 kHz / full-rate crossover at 96 kHz


def run(out: Path, seeds: int = 5, snrs=SNRS, settings=SETTINGS, seconds: float = 20.0) -> dict:
    """Runs every (setting, SNR, seed) through ac2 and the OSM harness; returns and writes
    the per-run means and the table."""
    import tomllib
    cfg = tomllib.loads(osm.CONFIG.read_text())
    harness, bin_dir = osm.find_tools(cfg)
    out.mkdir(parents=True, exist_ok=True)
    base = dict(cfg.get("settings", {}))
    lo, hi = osm.BIAS_BAND
    runs = []
    with tempfile.TemporaryDirectory(prefix="xc-coh-") as tmp, osm.private_daemon(bin_dir, Path(tmp)) as (ac2, recdir):
        for label, blocks, fft in settings:
            s = dict(base, ac2_tf_blocks=blocks, fft=fft)
            for snr in snrs:
                for k in range(seeds):
                    name = f"coh{label}_{snr:+g}_{k}".replace("+", "p").replace("-", "m").replace(".", "_")
                    case = osm_fixtures.Case(name, seconds, "tf", "", snr_db=snr, seed=1000 + 37 * k + int(snr * 3))
                    d = out / name
                    d.mkdir(parents=True, exist_ok=True)
                    osm_fixtures.generate(case, d / "pair.wav")
                    fs, x = wav.read(d / "pair.wav")
                    osm.run_ac2(ac2, recdir, name, d / "pair.wav", len(x), s, d, False)
                    oj = osm.run_harness(harness, d / "pair.wav", d / "osm.json", s)
                    L = osm.load_pair(d / "tf.csv", oj, x, s, 0.0)
                    fc, g2a = L["fc"], L["g2a"]
                    sel = (fc >= lo) & (fc <= hi) & np.isfinite(g2a)
                    sub = [float(np.mean(g2a[sel & (fc >= a) & (fc < b)])) for a, b in SUBBANDS]
                    okb = L["ok"] & (L["f"] >= lo) & (L["f"] <= hi)
                    runs.append({"setting": label, "blocks": blocks, "fft": fft, "snr": snr, "seed": case.seed,
                                 "ac2": float(np.mean(g2a[sel])), "ac2_sub": sub,
                                 "osm": float(np.mean(L["coh"][okb] ** 2)), "fc": fc[sel].tolist(), "fs": fs})
                    (d / "pair.wav").unlink()
    rows = summarise(runs, seeds)
    res = {"runs": [{k: v for k, v in r.items() if k != "fc"} for r in runs], "rows": rows}
    (out / "coherence-study.json").write_text(json.dumps(res, indent=1))
    (out / "coherence-study.md").write_text(table(rows))
    return res


def summarise(runs: list[dict], seeds: int) -> list[dict]:
    rows = []
    keys = sorted({(r["setting"], r["blocks"], r["fft"], r["snr"]) for r in runs}, key=lambda k: (k[0], -k[3]))
    for label, blocks, fft, snr in keys:
        rs = [r for r in runs if r["setting"] == label and r["snr"] == snr]
        g2 = 10 ** (snr / 10) / (1 + 10 ** (snr / 10))
        fc = np.array(rs[0]["fc"])
        parts = ac2_column_model(rs[0]["fs"], blocks, fc)
        e_col = ac2_expected_g2(g2, parts)
        n_col = np.array([min(n for _, n in p) for p in parts])
        e_sub = [float(np.mean(e_col[(fc >= a) & (fc < b)])) for a, b in SUBBANDS]
        neff_o = osm.welch_neff(2 ** fft, round(0.08 * rs[0]["fs"]), osm.OSM_COHERENCE_TICKS)
        a = np.array([r["ac2"] for r in rs])
        o = np.array([r["osm"] for r in rs])
        asub = np.array([r["ac2_sub"] for r in rs])
        rows.append({
            "setting": label, "blocks": blocks, "fft": fft, "snr": snr, "g2": g2, "seeds": len(rs),
            "ac2_neff_range": [float(np.min(n_col)), float(np.max(n_col))],
            "ac2_model": float(np.mean(e_col)), "ac2_mean": float(a.mean()), "ac2_sd": float(a.std(ddof=1)),
            "ac2_sub_model": e_sub, "ac2_sub_mean": asub.mean(axis=0).tolist(), "ac2_sub_sd": asub.std(axis=0, ddof=1).tolist(),
            "osm_neff": neff_o, "osm_model": osm.expected_g2(g2, neff_o), "osm_mean": float(o.mean()),
            "osm_sd": float(o.std(ddof=1))})
    return rows


def table(rows: list[dict]) -> str:
    """Residuals are mean − model; ±sem is the seed scatter of that mean (sd/√seeds)."""
    h = ["| SNR dB | setting | true γ² | ac2 N_eff (cols) | ac2 E[γ̂²] | ac2 mean ± sd | ac2 resid ± sem | "
         "ac2 resid 1–2.1k / 2.1–20k | OSM N_eff | OSM E[γ̂²] | OSM mean ± sd | OSM resid ± sem |",
         "|" + "---|" * 12]
    for r in rows:
        sq = math.sqrt(r["seeds"])
        sub = " / ".join(f"{m - e:+.4f}" for m, e in zip(r["ac2_sub_mean"], r["ac2_sub_model"]))
        h.append(f"| {r['snr']:+g} | {r['setting']} (ac2 --blocks {r['blocks']}, OSM FFT{r['fft']}) | {r['g2']:.4f} | "
                 f"{r['ac2_neff_range'][0]:.1f}–{r['ac2_neff_range'][1]:.0f} | {r['ac2_model']:.4f} | "
                 f"{r['ac2_mean']:.4f} ± {r['ac2_sd']:.4f} | {r['ac2_mean'] - r['ac2_model']:+.4f} ± {r['ac2_sd'] / sq:.4f} | "
                 f"{sub} | {r['osm_neff']:.1f} | {r['osm_model']:.4f} | {r['osm_mean']:.4f} ± {r['osm_sd']:.4f} | "
                 f"{r['osm_mean'] - r['osm_model']:+.4f} ± {r['osm_sd'] / sq:.4f} |")
    return "\n".join(h) + "\n"
