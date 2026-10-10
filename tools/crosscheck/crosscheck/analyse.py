"""Offline, re-runnable analysis of a run directory (or the 2026-10-07 fixtures).

Every comparison becomes a Check with a value, its tolerance and a sentence saying what the
number means. Sources:

- ac2 sweep (each variant), ac2 live transfer (TF) — ac2's own exports;
- REW offline import (meas and ref imported separately, meas/ref formed here), REW live
  (fixtures only: needs REW Pro to start over the API);
- direct: numpy cross-spectrum Σ M·R*/Σ|R|² of a raw recording (REW's stage, or ac2's own
  capture of its sweep) in the same bands, ac2's column width (`--resolution`, 1/48 octave by default);
- steady sines: least-squares phasors, Blackman harmonics, group delay from ±1/48-oct pairs.
"""
from __future__ import annotations

import math
import tomllib
from dataclasses import asdict, dataclass, field
from pathlib import Path

import numpy as np

from . import dsp, dut, dutcov, levels
from . import model
from .model import PathData, RunData, Sweep

TOLERANCES = Path(__file__).resolve().parent.parent / "tolerances.toml"
# A mains line leaks about ±1/gate (≈1.1 Hz for ac2's 0.89 s default gate) into the gated
# estimates; twice that keeps the masked columns clear of it.
MAINS_GUARD_HZ = 2.0


def dual_channel_harmonic_dbr(tones: list[dict], tone: dict, k: int, transfer=None) -> float | None:
    """Harmonic k of `tone` as a sweep divided by the measured reference reads it, dBr: the
    meas input's harmonic phasor less the reference input's carried through the path,
    D_m − T(k·f)·D_r, over the meas fundamental. T(k·f) is the meas÷ref ratio interpolated
    between the tones (dB and unwrapped phase, linear in log f); outside them, `transfer(k·f)`
    (a direct cross-spectrum of the same path, path delay in its phase) when given. None when
    neither covers k·f or the tone has no phasors."""
    hv = (tone.get("h_vec") or {}).get(str(k))
    pts = sorted((x["f"], x["ratio_db"], x["ratio_deg"]) for x in tones if "ratio_db" in x)
    fk = k * tone["f"]
    if hv is None:
        return None
    if len(pts) >= 2 and pts[0][0] <= fk <= pts[-1][0]:
        lf = np.log([a for a, _, _ in pts])
        mag = np.interp(np.log(fk), lf, [b for _, b, _ in pts])
        ph = np.interp(np.log(fk), lf, np.unwrap(np.radians([c for _, _, c in pts])))
        tk = 10 ** (mag / 20) * np.exp(1j * ph)
    else:
        tk = transfer(fk) if transfer is not None else None
        if tk is None or not np.isfinite(tk):
            return None
    d = complex(*hv["meas"]) - tk * complex(*hv["ref"])
    return float(20 * np.log10(abs(d))) if abs(d) > 0 else None


@dataclass
class Check:
    id: str
    group: str
    path: str
    title: str
    value: float | None
    unit: str
    tol: tuple[float, float] | None
    status: str  # PASS WARN FAIL INCONCLUSIVE INFO
    meaning: str
    detail: dict = field(default_factory=dict)


def judge(value, tol) -> str:
    if value is None or not np.isfinite(value):
        return "INCONCLUSIVE"
    if tol is None:
        return "INFO"
    a = abs(value)
    return "PASS" if a <= tol[0] else "WARN" if a <= tol[1] else "FAIL"


def judge_u(value, tol, u) -> str:
    """`judge` for a comparison with its own uncertainty `u` (2σ, the value's units): FAIL only
    when the value is beyond the warn limit by more than u; INCONCLUSIVE when u alone exceeds
    the pass limit (the data cannot tell a pass from a fail); otherwise the plain verdict."""
    if u is None or not np.isfinite(u) or tol is None or value is None or not np.isfinite(value):
        return judge(value, tol)
    if abs(value) - u > tol[1]:
        return "FAIL"
    if u > tol[0]:
        return "INCONCLUSIVE"
    return judge(value, tol)


def worst(*s: str) -> str:
    order = ["PASS", "INFO", "WARN", "INCONCLUSIVE", "FAIL"]
    return max(s, key=order.index)


def _tol(t: dict) -> tuple[float, float]:
    return (float(t["pass"]), float(t["warn"]))


def _f(x):
    if x is None:
        return None
    try:
        x = float(x)
    except (TypeError, ValueError):
        return None
    return x if math.isfinite(x) else None


class Analysis:
    def __init__(self, run: RunData, tolerances: dict | None = None):
        self.run = run
        self.tol = tolerances or tomllib.loads(TOLERANCES.read_text())
        # ac2's columns per octave as the run asked for them: every direct estimate, REW
        # average and mains guard is taken over the same band width, so a column's
        # averaging of the room's structure is the same on every source
        from .baseline import resolution_of
        self.ppo = resolution_of(getattr(run, "manifest", None))
        self.frac = 1 / self.ppo
        self.checks: list[Check] = []
        self.tables: dict[str, dict] = {}
        self.series: dict[str, dict] = {}  # for plots
        self.notes: list[str] = []
        self.dut_cells: list[dict] = []  # dutcov entries, every digital path

    # ------------------------------------------------------------ helpers
    def add(self, **kw) -> Check:
        c = Check(**kw)
        self.checks.append(c)
        return c

    @staticmethod
    def narrow_at(raw, fc: float, tau: float):
        """The direct estimate of one recording in a band a few bins wide centred on fc (the
        sine's own frequency, not a column centre), with its own noise from the band's
        coherence over its n bins: σφ = √((1 − γ²) / (2 γ² n)), 2σ in dB and in °."""
        T = len(raw.meas) / raw.fs
        fn = max(1 / 1000, np.log2(1 + 6 / (T * fc)))
        hn, cn = dsp.cross_spectrum_bands(raw.meas, raw.ref, raw.fs, np.array([fc]), fn, tau)
        hn, cn = hn[0], cn[0]
        if not (np.isfinite(hn) and np.isfinite(cn) and cn > 0):
            return None
        nb = max(1.0, fc * (2 ** (fn / 2) - 2 ** (-fn / 2)) * T)
        sph = np.sqrt(max(1 - cn, 0.0) / (2 * cn * nb))
        return hn, (2 * 20 / np.log(10) * sph, 2 * np.degrees(sph))

    def table(self, name: str, columns: list[str], rows: list[list], note: str = ""):
        self.tables[name] = {"columns": columns, "rows": rows, "note": note}

    def at(self, f_src, H_src, f_tgt, tau: float | None = None):
        """Complex response interpolated onto f_tgt: dB and unwrapped phase, linear in log f.

        The path's bulk delay `tau` (default: the path's arrival) is taken out first and put
        back after: a delay turns the phase by 2π·τ·Δf between neighbouring columns (half a
        turn at 10 kHz for 3.6 ms over 1/48 octave), which no unwrap can follow."""
        tau = getattr(self, "bulk_delay", 0.0) if tau is None else tau
        f_src, H_src = np.asarray(f_src), np.asarray(H_src)
        f_tgt = np.asarray(f_tgt, dtype=float)
        ok = np.isfinite(H_src) & (f_src > 0)
        fs_, Hs = f_src[ok], H_src[ok] * np.exp(2j * np.pi * f_src[ok] * tau)
        if len(fs_) < 2:
            return np.full(len(f_tgt), np.nan + 0j)
        lf = np.log(fs_)
        m = np.interp(np.log(f_tgt), lf, dsp.db(Hs), left=np.nan, right=np.nan)
        p = np.interp(np.log(f_tgt), lf, np.unwrap(np.angle(Hs)), left=np.nan, right=np.nan)
        return 10 ** (m / 20) * np.exp(1j * p) * np.exp(-2j * np.pi * f_tgt * tau)

    # ------------------------------------------------------------ entry
    def run_all(self) -> dict:
        for p in self.run.paths.values():
            self.path(p)
        if self.run.ambient:
            from . import ambient
            ambient.analyse(self, self.run)
        if self.dut_cells:
            self.dut_coverage()
        return self.results()

    def results(self) -> dict:
        counts = {}
        for c in self.checks:
            counts[c.status] = counts.get(c.status, 0) + 1
        from . import baseline
        res = {"source": str(self.run.root), "fixture": self.run.fixture, "manifest": self.run.manifest,
               "summary": counts, "checks": [asdict(c) for c in self.checks], "tables": self.tables,
               "notes": self.notes}
        return baseline.annotate(res, baseline.nominal_maps_from_truth(
            {n: p.truth for n, p in self.run.paths.items()}))

    def path(self, p: PathData):
        self.notes.extend(f"{p.name}: {n}" for n in p.notes)
        grid = self.sources(p)
        if grid is None:
            self.notes.append(f"{p.name}: no ac2 sweep to set the comparison grid; path skipped"
                              + (" (DUT truth rows still made)" if p.dut else ""))
            if p.dut:
                self.dut_harmonics(p)
            return
        self.bands(p)
        self.level_convention(p)
        self.delays(p)
        self.etc(p)
        self.group_delay(p)
        self.harmonics(p)
        if p.dut:
            self.dut_harmonics(p)
        self.lf_h2_onset(p)
        self.coherence(p)
        if p.kind == "speaker":
            self.absolute_spl(p)
            self.room(p)

    # ------------------------------------------------------------ sources on one grid
    def sources(self, p: PathData):
        if not p.primary:
            return None
        sw = p.sweeps[p.primary]
        f, H, _ = sw.trace.complex_response()
        self.f = f
        if len(f) > 2:
            # the grid ac2 stored is the truth on its resolution; the manifest only says what was asked
            got = int(round(1 / float(np.median(np.diff(np.log2(f))))))
            if got != self.ppo:
                self.notes.append(f"{p.name}: ac2's sweep grid is 1/{got} octave, the run asked for 1/{self.ppo}; "
                                  f"read on 1/{got}")
                self.ppo, self.frac = got, 1 / got
        # One phase reference for every source: meas ÷ ref as the inputs saw it, the path's
        # delay inside the phase, as the steady sines and the direct cross-spectra measure it.
        # ac2 refers its sweep phase to the arrival it reports, so the arrival goes back in.
        self.arrival = {}
        self.bulk_delay = _f((sw.trace.sweep_info or {}).get("arrival")) or 0.0
        src: dict[str, np.ndarray] = {}
        for name, s in p.sweeps.items():
            fs_, Hs, _ = s.trace.complex_response()
            Hs = Hs if name == p.primary else self.at(fs_, Hs, f, tau=0.0)  # referred to its arrival
            arr = _f((s.trace.sweep_info or {}).get("arrival")) or 0.0
            self.arrival[name] = arr
            src[f"ac2 sweep {name}"] = Hs * np.exp(-2j * np.pi * f * arr)
        src = {f"ac2 sweep {p.primary}": src.pop(f"ac2 sweep {p.primary}"), **src}
        self.coh = {}
        self.fine_u = {}  # path → tone → the room's fine structure between a column and the sine, dB
        self.coh_by_path = getattr(self, "coh_by_path", {})
        self.coh_by_path[p.name] = self.coh
        if p.tf is not None:
            b = p.tf.freq
            Ht = 10 ** (b["mag_db"] / 20) * np.exp(1j * np.deg2rad(b["phase_deg"]))
            # the live TF's phase is referred to its inserted delay (an operator's delay-finder
            # result): interpolated there, where it is smooth, then the delay put back like a
            # sweep's arrival so every source shares one phase reference
            tf_delay = ((_f(p.tf.meta.get("delay_ms")) or 0.0) + (_f(p.tf.meta.get("delay_nudge_ms")) or 0.0)) / 1e3
            src["ac2 TF"] = self.at(b["freq_hz"], Ht, f) * np.exp(-2j * np.pi * f * tf_delay)
            if "coherence" in b:
                ok = np.isfinite(b["coherence"])
                self.coh["ac2 TF"] = np.interp(np.log(f), np.log(b["freq_hz"][ok]), b["coherence"][ok],
                                               left=np.nan, right=np.nan)
        if p.rew and p.rew.meas_fr is not None:
            if p.rew.ref_fr is not None:
                Hr = p.rew.meas_fr.H / p.rew.ref_fr.H
                src["REW offline"] = dsp.band_mean(p.rew.meas_fr.f, Hr, f, self.frac)
            else:
                src["REW offline"] = dsp.band_mean(p.rew.meas_fr.f, p.rew.meas_fr.H, f, self.frac)
        if p.rew_live and p.rew_live.meas_fr is not None:
            src["REW live"] = dsp.band_mean(p.rew_live.meas_fr.f, p.rew_live.meas_fr.H, f, self.frac)
        # each direct estimate with its recording's own delay (IR peak) taken out of the band sums
        self.direct_delay = {}
        self.capture_coh = {}
        if p.rec is not None:
            self.direct_delay["direct (REW recording)"] = tau_d = direct_ir_peak(p.rec)
            Hd, cd = dsp.cross_spectrum_bands(p.rec.meas, p.rec.ref, p.rec.fs, f, self.frac, delay_s=tau_d)
            src["direct (REW recording)"] = Hd
            self.coh["direct (REW recording)"] = cd
        for name, s in p.sweeps.items():
            if s.raw is not None:
                tau_d = self.direct_delay[f"direct (ac2 capture {name})"] = direct_ir_peak(s.raw)
                Hd, cd = dsp.cross_spectrum_bands(s.raw.meas, s.raw.ref, s.raw.fs, f, self.frac, delay_s=tau_d)
                src[f"direct (ac2 capture {name})"] = Hd
                self.capture_coh[f"direct (ac2 capture {name})"] = cd  # the distortion gate in bands()
        if p.tf_raw is not None:
            tau_d = self.direct_delay["direct (TF capture)"] = direct_ir_peak(p.tf_raw)
            Hd, cd = dsp.cross_spectrum_bands(p.tf_raw.meas, p.tf_raw.ref, p.tf_raw.fs, f, self.frac, delay_s=tau_d)
            src["direct (TF capture)"] = Hd
            self.coh["direct (TF capture)"] = cd
        # REW's offline import refers each channel to its own timing marker, which drops the
        # path's delay: put back the delay difference to the direct cross-spectrum of the very
        # same recording (phase slope over the band the delay checks use), so REW's phase is
        # judged for its shape on the same reference as everything else.
        self.rew_put_back = 0.0
        if "REW offline" in src and p.rew.ref_fr is not None and "direct (REW recording)" in src:
            lo, hi = (1000, 20000) if p.kind != "speaker" else (500, 5000)
            Hd, Hr = src["direct (REW recording)"], src["REW offline"]
            md, mr = np.isfinite(Hd), np.isfinite(Hr)
            self.rew_put_back = dsp.delay_from_phase(f[md], Hd[md], lo, hi) - dsp.delay_from_phase(f[mr], Hr[mr], lo, hi)
            src["REW offline"] = Hr * np.exp(-2j * np.pi * f * self.rew_put_back)
        # Stationary lines on the measurement input (mains) are no part of the response; takes
        # and estimators read them differently, so the band comparisons leave those columns out.
        # A line's level drifts between the silent recording and the takes, so a line only 6 dB
        # above its neighbourhood in the silence can still stand out in one take's column.
        self.mains_hz = []
        if p.noise is not None and p.mains_hz:
            self.mains_hz = [x["hz"] for x in dsp.mains_lines(p.noise.meas, p.noise.fs,
                                                              p.mains_hz, thr_db=6.0)]
        self.mains_cols = dsp.mains_columns(f, self.mains_hz, self.frac, guard_hz=MAINS_GUARD_HZ)
        # Each source's per-column noise (relative error of H) from the SNR of the recording it
        # came from, against the silent recording of the same input. A windowed deconvolution
        # (ac2's sweep) rejects more noise than the whole recording holds, so this is an upper bound.
        self.noise_rel = {}
        if p.noise is not None:
            rec_of, coh_of = {}, {}
            for name, s in p.sweeps.items():
                if s.raw is not None:
                    rec_of[f"ac2 sweep {name}"] = rec_of[f"direct (ac2 capture {name})"] = s.raw
                    coh_of[id(s.raw)] = self.capture_coh.get(f"direct (ac2 capture {name})")
            if p.rec is not None:
                rec_of["REW offline"] = rec_of["direct (REW recording)"] = p.rec
            cache = {}
            for name, raw in rec_of.items():
                if name in src and raw.fs == p.noise.fs:
                    if id(raw) not in cache:
                        cache[id(raw)] = dsp.band_noise_rel(raw.meas, p.noise.meas, raw.fs, f, self.frac,
                                                            coh=coh_of.get(id(raw)))
                    self.noise_rel[name] = cache[id(raw)]
        # The live TF's own noise from its coherence over its FIFO blocks: σ = √((1 − γ²) / (2 γ² n)).
        # n = blocks counts every block as independent, though they overlap, so this is a lower
        # bound on the TF's noise and the gate leaves out too few columns rather than too many.
        if "ac2 TF" in self.coh and p.tf_blocks:
            g = np.clip(np.nan_to_num(self.coh["ac2 TF"]), 1e-6, 1.0)
            self.noise_rel["ac2 TF"] = np.sqrt((1 - g) / (2 * g * p.tf_blocks))
        self.src = src
        self.series[p.name] = {"f": f, "src": src}
        return f

    # ------------------------------------------------------------ magnitude and phase
    def bands(self, p: PathData):
        f, S = self.f, self.src
        primary = f"ac2 sweep {p.primary}"
        elec = p.kind != "speaker"
        pairs = []
        for direct in [k for k in S if k.startswith("direct")]:
            pass
        if "REW offline" in S:
            pairs.append((primary, "REW offline", False, "ac2's sweep against REW's offline import "
                          "(REW meas ÷ REW ref, each referred to REW's own timing marker)"))
        if "direct (REW recording)" in S:
            pairs.append(("REW offline", "direct (REW recording)", False,
                          "REW against numpy on the very same recording: REW's deconvolution alone"))
            pairs.append((primary, "direct (REW recording)", False,
                          "ac2's sweep against the direct cross-spectrum of REW's run (another take, "
                          "maybe another level)"))
        for k in S:
            if k.startswith("direct (ac2 capture"):
                name = k[len("direct (ac2 capture "):-1]
                pairs.append((f"ac2 sweep {name}", k, False,
                              "ac2's sweep against numpy on ac2's own raw capture of that sweep: ac2's "
                              "deconvolution and windows alone"))
        if "REW live" in S:
            pairs.append((primary, "REW live", True, "ac2's sweep against REW live; REW keeps the "
                          "measurement channel's absolute level, so the mean offset is the level convention "
                          "(see level checks) and only the spread is judged"))
        for name in S:
            if name.startswith("ac2 sweep ") and name != primary:
                pairs.append((name, primary, False, "two ac2 sweep variants against each other"))
        if "ac2 TF" in S:
            ref = "direct (TF capture)" if "direct (TF capture)" in S else (
                "direct (REW recording)" if "direct (REW recording)" in S else None)
            if ref:
                pairs.append(("ac2 TF", ref, False, "ac2's live transfer against the direct cross-spectrum "
                              "where its coherence γ² ≥ 0.99"))
        bands = [(20, 100, "mag_lf"), (100, 1000, "mag_mf"), (1000, 20000, "mag_hf")]
        # The sweeps' own band edges (fade-in, fade-out, regularisation) shape each estimator
        # its own way: the columns within 1/12 octave of the narrowest sweep's ends are left out,
        # 1/6 octave below the top on acoustic paths, where REW's response falls off from there.
        ends = [s.end_hz for s in p.sweeps.values() if s.end_hz]
        starts = [s.start_hz for s in p.sweeps.values() if s.start_hz]
        if p.rew and getattr(p.rew, "summary", None):
            ends.append(_f(p.rew.summary.get("endFreq")))
            starts.append(_f(p.rew.summary.get("startFreq")))
        f_hi = min([e for e in ends if e] or [np.inf]) * 2 ** (-1 / 12 if elec else -1 / 6)
        f_lo = max([x for x in starts if x] or [0.0]) * 2 ** (1 / 12)
        edge_cols = (f > f_hi) | (f < f_lo)
        # In a room a comb null's level is the small difference of large reflections: a take
        # later, or a window wider, moves it by dBs while the response around it stays put.
        # Acoustic paths leave out columns 10 dB or more below their 1/3-octave power mean.
        null = np.zeros(len(f), bool)
        if not elec:
            pa = np.abs(S[primary]) ** 2
            ok = np.isfinite(pa)
            for i in np.where(ok)[0]:
                w = ok & (f >= f[i] * 2 ** (-1 / 6)) & (f <= f[i] * 2 ** (1 / 6))
                null[i] = pa[i] < 0.1 * np.mean(pa[w])
        rows = []
        for a, b, offset_free, meaning in pairs:
            Ha, Hb = S[a], S[b]
            m_all = np.isfinite(Ha) & np.isfinite(Hb) & ~self.mains_cols & ~edge_cols
            if a == "ac2 TF" and "ac2 TF" in self.coh:
                m_all &= self.coh["ac2 TF"] >= 0.99
            # phase after removing any pure delay difference left between the two (every source is
            # on the absolute reference already): the delay itself is judged in the delay checks
            rm = (f >= 1000) & (f <= 20000) & m_all if elec else (f >= 200) & (f <= 5000) & m_all
            tau = dsp.delay_from_phase(f[rm], (Ha / Hb)[rm], 0, np.inf) if rm.sum() > 10 else 0.0
            ratio = Ha / Hb * np.exp(2j * np.pi * f * tau)
            # The nulls are the path's: found once on ac2's primary sweep, used for every pair.
            m_all &= ~null
            # A sweep's direct estimate reads the band's whole output power, |H|² = Σ|M|²/Σ|R|²:
            # the harmonics a nonlinear path made at f/k earlier in the sweep land in the band too
            # (an ac2 sweep windows them out). They do not correlate with the reference's chirp
            # at f, so they show as a coherence deficit; Σ|M|² exceeds the linear part by 1/γ², a
            # magnitude excess of −10·log10 γ² dB. Columns where that alone exceeds the pass limit
            # do not hold the linear response: left out and counted (electrical paths; in a room
            # the band's comb structure lowers γ² as well).
            excess = None
            if elec and b in self.capture_coh:
                with np.errstate(divide="ignore", invalid="ignore"):
                    excess = -10 * np.log10(self.capture_coh[b])
            # columns where the two sources' own noise (2σ) already exceeds the pass limit
            # cannot be judged: left out and counted
            za, zb = self.noise_rel.get(a, np.zeros(len(f))), self.noise_rel.get(b, np.zeros(len(f)))
            u_mag = 2 * 20 / np.log(10) * np.sqrt((za ** 2 + zb ** 2) / 2)
            for lo, hi, key in bands:
                tk = "mag_tf" if a == "ac2 TF" else (key if elec else "mag_acoustic")
                tm = _tol(self.tol["band"][tk])
                inb = m_all & (f >= lo) & (f < hi)
                noisy = inb & ~(u_mag <= tm[0])
                nn = int(noisy.sum())
                dist = inb & ~noisy & ~(excess <= tm[0]) if excess is not None else np.zeros(len(f), bool)
                nd = int(dist.sum())
                m = inb & ~noisy & ~dist
                if m.sum() < 3:
                    if nn or nd:
                        for g, unit in (("mag", "dB"), ("phase", "°")):
                            self.add(id=f"{p.name}.{g}.{a}|{b}.{lo}-{hi}", group="magnitude" if g == "mag" else "phase",
                                     path=p.name, title=f"{a} − {b}, {lo}–{hi} Hz, {'magnitude' if g == 'mag' else 'phase'}",
                                     value=None, unit=unit, tol=tm if g == "mag" else None, status="INCONCLUSIVE",
                                     meaning=meaning + f". Of {int(inb.sum())} columns, {nn} carry more noise (2σ, from "
                                             "the recordings' SNR against the silent recording, or the live TF's coherence over "
                                             f"its blocks) than the pass limit and {nd} more uncorrelated content (harmonics, "
                                             "−10·log10 γ² of the direct estimate); too few left to judge")
                    continue
                nm = int((self.mains_cols & (f >= lo) & (f < hi)).sum())
                dm = dsp.db(ratio[m])
                dp = np.rad2deg(np.angle(ratio[m]))
                mean, spread = float(np.mean(dm)), float(np.max(np.abs(dm - np.mean(dm))))
                pmean, pspread = float(np.mean(dp)), float(np.max(np.abs(dp - np.mean(dp))))
                tp = _tol(self.tol["band"]["phase" if elec else "phase_acoustic"])
                st_m = judge(spread, tm) if offset_free else worst(judge(spread, tm), judge(mean, tm))
                st_p = worst(judge(pspread, tp), judge(pmean, tp))
                self.add(id=f"{p.name}.mag.{a}|{b}.{lo}-{hi}", group="magnitude", path=p.name,
                         title=f"{a} − {b}, {lo}–{hi} Hz, magnitude", value=spread if offset_free else max(abs(mean), spread),
                         unit="dB", tol=tm, status=st_m,
                         meaning=meaning + f". Mean {mean:+.3f} dB, max deviation from the mean ±{spread:.3f} dB "
                                 f"over {m.sum()} columns"
                                 + (f" ({nm} columns within {MAINS_GUARD_HZ:g} Hz of a mains line left out)" if nm else "")
                                 + (f" ({nn} columns whose noise alone (2σ{', from its coherence' if a == 'ac2 TF' else ''}) exceeds the pass limit left out)" if nn else "")
                                 + (f" ({nd} columns where the direct estimate's band power holds more than the pass "
                                    "limit of uncorrelated content (harmonics, −10·log10 γ²) left out)" if nd else "")
                                 + (f" ({int((null & (f >= lo) & (f < hi)).sum())} comb-null columns ≥ 10 dB below "
                                    "their 1/3-octave mean left out)" if (null & (f >= lo) & (f < hi)).any() else "")
                                 + ".",
                         detail={"mean": mean, "spread": spread, "n": int(m.sum())})
                self.add(id=f"{p.name}.phase.{a}|{b}.{lo}-{hi}", group="phase", path=p.name,
                         title=f"{a} − {b}, {lo}–{hi} Hz, phase", value=max(abs(pmean), pspread), unit="°", tol=tp,
                         status=st_p,
                         meaning=f"phase difference after removing a pure delay difference of {tau*1e6:+.2f} µs "
                                 f"(fitted 1–20 kHz); mean {pmean:+.3f}°, spread ±{pspread:.3f}°",
                         detail={"mean": pmean, "spread": pspread, "n": int(m.sum()), "delay_removed_s": tau})
                rows.append([f"{a} − {b}", f"{lo}–{hi}", f"{mean:+.3f} ± {spread:.3f}", f"{pmean:+.2f} ± {pspread:.2f}",
                             f"{tau*1e6:+.2f}", int(m.sum())])
        self.table(f"{p.name}: magnitude and phase per band", ["pair", "band Hz", "Δ dB mean ± spread",
                   "Δ° mean ± spread", "delay removed µs", "n"], rows,
                   f"1/{self.ppo}-octave columns of ac2's primary sweep; the TF only where γ² ≥ 0.99. Every source on one "
                   "phase reference (meas ÷ ref with the path delay in it): ac2's arrival and REW's lost delay "
                   "put back. Columns within "
                   f"{MAINS_GUARD_HZ:g} Hz of a mains line on the measurement input left out "
                   f"(lines: {', '.join(f'{x:g}' for x in self.mains_hz) or 'none found'} Hz).")
        # against the steady sines
        t = p.truth
        if t:
            rows = []
            for tone in t.get("tones", []):
                fc = tone["f"]
                if "ratio_deg" not in tone and "ratio_db" not in tone:
                    continue
                # A room's response has structure finer than a column (reflections
                # comb it at 1/Δt): columns read between their centres and the sine's single frequency then
                # differ by that structure, not by an error. The direct estimate of REW's
                # recording read both ways measures it, and it counts as the comparison's
                # uncertainty; the narrow reading is also compared with the sine itself.
                u_db = u_deg = 0.0
                # Each direct estimate read narrow at the sine as well, on every raw capture there
                # is: its column minus its narrow reading is the column-to-tone difference of
                # that capture alone, so what a measurement differs from the sine by beyond it is
                # left as the remaining error.
                caps = {"direct (REW recording)": p.rec, "direct (TF capture)": p.tf_raw}
                pr = p.sweeps.get(p.primary)
                if pr is not None:
                    caps[f"direct (ac2 capture {p.primary})"] = pr.raw
                narrows = {}
                for cname, raw in caps.items():
                    if raw is None or cname not in S:
                        continue
                    nr = self.narrow_at(raw, fc, self.direct_delay.get(cname, 0.0))
                    if nr is None:
                        continue
                    narrows[cname] = nr
                    if cname == "direct (REW recording)" and not elec:
                        # the column estimate read the way every source is read: interpolated
                        # between the columns around fc; where the narrow band is
                        # noisy the structure is not known any better
                        hc = self.at(f, S[cname], np.array([fc]))[0]
                        if np.isfinite(hc):
                            hn, nu = nr
                            u_db = max(abs(float(dsp.db(hc) - dsp.db(hn))), nu[0])
                            u_deg = max(abs(float(dsp.wrap_deg(np.rad2deg(np.angle(hc / hn))))), nu[1])
                self.fine_u.setdefault(p.name, {})[fc] = u_db
                names = [primary, "REW live", "REW offline", "direct (REW recording)", "ac2 TF"]
                names += [c for c in ("direct (TF capture)", f"direct (ac2 capture {p.primary})") if c in narrows]
                names += [f"{c}, narrow at the sine" for c in narrows]
                for name in names:
                    narrow_row = name.endswith(", narrow at the sine")
                    if name not in S and not narrow_row:
                        continue
                    if narrow_row:
                        h, (ud, up) = narrows[name[:-len(", narrow at the sine")]]
                    else:
                        h, ud, up = self.at(f, S[name], np.array([fc]))[0], u_db, u_deg
                    # the readings the REW recording's narrow one does not carry: context for the
                    # column-to-tone split, not judged
                    info_only = name in ("direct (TF capture)", f"direct (ac2 capture {p.primary})") or (
                        narrow_row and not name.startswith("direct (REW recording)"))
                    if not np.isfinite(h):
                        continue
                    if name == "ac2 TF" and "ac2 TF" in self.coh:
                        cg = np.interp(np.log(fc), np.log(f), np.nan_to_num(self.coh["ac2 TF"]))
                        if cg < 0.99:
                            rows.append([f"{fc:g}", name, "", f"γ² {cg:.3f} < 0.99: not compared"])
                            continue
                    dpd = float(dsp.wrap_deg(np.rad2deg(np.angle(h)) - tone["ratio_deg"])) if "ratio_deg" in tone else None
                    dmd = float(dsp.db(h) - tone["ratio_db"]) if "ratio_db" in tone else None
                    tp = _tol(self.tol["band"]["phase" if elec else "phase_acoustic"])
                    if dpd is not None:
                        note = ""
                        if name == "REW offline" and self.rew_put_back:
                            note = (f" (REW offline: {self.rew_put_back*1e6:+.2f} µs put back, the delay its "
                                    "timing markers took out)")
                        elif name.startswith("ac2 sweep"):
                            note = (f" (ac2: its reported arrival {self.arrival.get(p.primary, 0)*1e6:.3f} µs put "
                                    "back into the phase it refers to that arrival)")
                        self.add(id=f"{p.name}.sine_phase.{name}.{fc:g}", group="phase vs sine", path=p.name,
                                 title=f"{name} phase at {fc:g} Hz vs steady sine", value=dpd, unit="°", tol=tp,
                                 status="INFO" if info_only else judge_u(dpd, tp, up),
                                 meaning=f"sine {tone['ratio_deg']:+.3f}°, {name} {np.rad2deg(np.angle(h)):+.3f}°{note}"
                                         + ((f"; the narrow band's own noise (coherence) ±{up:.1f}°" if narrow_row else
                                             f"; the room's fine structure: a 1/{self.ppo}-oct column and the sine's frequency "
                                             f"differ by {up:.1f}° in the direct estimate") if up else ""),
                                 detail={"uncertainty": up})
                    if dmd is not None:
                        tm = _tol(self.tol["band"]["mag_lf" if elec else "mag_acoustic"])
                        self.add(id=f"{p.name}.sine_mag.{name}.{fc:g}", group="magnitude vs sine", path=p.name,
                                 title=f"{name} magnitude at {fc:g} Hz vs steady sine", value=dmd, unit="dB", tol=tm,
                                 status=judge_u(dmd, tm, ud) if not (name.startswith("REW live") or info_only) else "INFO",
                                 meaning=f"sine {tone['ratio_db']:+.3f} dB (meas ÷ ref), {name} {dsp.db(h):+.3f} dB"
                                         + ((f"; the narrow band's own noise (coherence) ±{ud:.2f} dB" if narrow_row else
                                             f"; the room's fine structure: a 1/{self.ppo}-oct column and the sine's frequency "
                                             f"differ by {ud:.2f} dB in the direct estimate") if ud else ""),
                                 detail={"uncertainty": ud})
                    rows.append([f"{fc:g}", name, "" if dmd is None else f"{dmd:+.3f}", "" if dpd is None else f"{dpd:+.3f}"])
            self.table(f"{p.name}: against the steady sines", ["f Hz", "source", "Δ dB", "Δ°"], rows)

    # ------------------------------------------------------------ level convention
    def level_convention(self, p: PathData):
        sw = p.sweeps[p.primary]
        info = sw.trace.sweep_info or {}
        f = self.f
        k = int(np.argmin(np.abs(f - 1000)))
        a_db = float(dsp.db(self.src[f"ac2 sweep {p.primary}"][k]))
        ref_lvl = _f(info.get("reference_level"))
        rows = [["ac2 sweep meas ÷ ref at 1 kHz", f"{a_db:+.3f} dB", "dual-channel: 0 dB = the measurement "
                 "point carries what the reference point carries"],
                ["ac2 sweep reference level", "" if ref_lvl is None else f"{ref_lvl:+.3f} dB",
                 "reference input re the emitted level (the loopback's gain)"]]
        G = {}
        for rs in (p.rew, p.rew_live):
            if rs is None or rs.meas_fr is None:
                continue
            fr = rs.meas_fr
            kk = int(np.argmin(np.abs(fr.f - 1000)))
            gm = float(fr.mag[kk] - rs.level_dbfs)
            G[rs.label] = {"meas": gm}
            rows.append([f"{rs.label}: meas re emitted at 1 kHz", f"{gm:+.3f} dB",
                         f"REW's dBFS response minus the {rs.level_dbfs:g} dBFS it played: the path's own gain"])
            if rs.ref_fr is not None:
                gr = float(rs.ref_fr.mag[kk] - rs.level_dbfs)
                G[rs.label]["ref"] = gr
                rows.append([f"{rs.label}: ref re emitted at 1 kHz", f"{gr:+.3f} dB", "the loopback's gain as REW reads it"])
        t = p.truth or {}
        tone = next((x for x in t.get("tones", []) if abs(x["f"] / 1000 - 1) < 0.05 and "meas_level_dbfs" in x), None)
        if tone:
            gm_s = tone["meas_level_dbfs"] - t["emit_dbfs"]
            gr_s = tone["ref_level_dbfs"] - t["emit_dbfs"]
            rows.append(["steady sine: meas re emitted at 1 kHz", f"{gm_s:+.3f} dB", "least-squares phasor, sine convention"])
            rows.append(["steady sine: ref re emitted at 1 kHz", f"{gr_s:+.3f} dB", ""])
            if ref_lvl is not None:
                d = ref_lvl - gr_s
                self.add(id=f"{p.name}.level.ac2_ref_vs_sine", group="level", path=p.name,
                         title="ac2 sweep reference level vs steady sine (ref ÷ emitted)", value=d, unit="dB",
                         tol=_tol(self.tol["level"]["convention_db"]), status=judge(d, _tol(self.tol["level"]["convention_db"])),
                         meaning="both state the loopback's gain re the digital level emitted; a 3.01 dB difference "
                                 "would be a full-scale-sine vs RMS convention slip")
        # REW's responses are unsmoothed FFT bins (0.37 Hz at 96 kHz), ac2's are 1/48-octave
        # columns by default (14 Hz wide at 1 kHz): one bin against one column reads the room's structure
        # inside the column (0.1 dB and more on a speaker), not a level. REW's meas ÷ ref is
        # averaged into ac2's columns as ac2 reads them, and the median over 500 Hz–2 kHz judged.
        sel = (f >= 500) & (f < 2000) & np.isfinite(self.src[f"ac2 sweep {p.primary}"])
        rew_sets = {rs.label: rs for rs in (p.rew, p.rew_live) if rs is not None}
        for label, g in G.items():
            if "ref" in g:
                rs = rew_sets[label]
                Hm, Hr = rs.meas_fr, rs.ref_fr
                ratio = 10 ** ((Hm.mag - Hr.mag) / 20)
                rc = dsp.band_mean(Hm.f, ratio, f[sel], self.frac)
                d = float(np.median(dsp.db(self.src[f"ac2 sweep {p.primary}"][sel]) - dsp.db(rc)))
                tl = _tol(self.tol["level"]["offset_db"])
                self.add(id=f"{p.name}.level.ac2_vs_rew_meas_minus_ref", group="level", path=p.name,
                         title=f"ac2 meas÷ref vs {label} (meas − ref)", value=d, unit="dB", tol=tl, status=judge(d, tl),
                         meaning="ac2 states meas ÷ ref; REW states each channel re the stimulus; their difference "
                                 f"must be ac2's number. Median over the 1/{self.ppo}-octave columns 500 Hz–2 kHz, REW's "
                                 f"bins power-averaged into each (one bin at 1 kHz read {a_db - (g['meas'] - g['ref']):+.3f} dB)",
                         detail={"single_bin_1k": a_db - (g["meas"] - g["ref"]), "n": int(sel.sum())})
                if ref_lvl is not None:
                    d2 = ref_lvl - g["ref"]
                    tc = _tol(self.tol["level"]["convention_db"])
                    self.add(id=f"{p.name}.level.ac2_ref_vs_rew_ref", group="level", path=p.name,
                             title=f"ac2 reference level vs {label} ref re emitted", value=d2, unit="dB", tol=tc,
                             status=judge(d2, tc),
                             meaning="the same loopback gain read by both apps from their own emission level: checks "
                                     "that 'dBFS' means the same (full-scale sine) in both")
            else:
                off = a_db - g["meas"]
                self.add(id=f"{p.name}.level.offset_live", group="level", path=p.name,
                         title=f"ac2 meas÷ref minus {label} meas re emitted", value=off, unit="dB", tol=None,
                         status="INFO",
                         meaning="REW live with the loopback as cal keeps the measurement channel's absolute "
                                 "level re the stimulus; ac2 reports meas ÷ ref, so the offset is minus the "
                                 f"loopback gain (ac2's reference level {ref_lvl:+.2f} dB)" if ref_lvl is not None else
                                 "REW live keeps the measurement channel's absolute level")
                if ref_lvl is not None:
                    d = off + ref_lvl
                    tl = _tol(self.tol["level"]["offset_db"])
                    self.add(id=f"{p.name}.level.offset_live_explained", group="level", path=p.name,
                             title=f"offset to {label} explained by the reference level", value=d, unit="dB", tol=tl,
                             status=judge(d, tl),
                             meaning="ac2 meas÷ref + ac2 reference level − REW absolute: zero when the whole "
                                     "offset is the loopback's gain")
        # ac2's own raw capture: the emitted level as the loopback returns it
        for name, s in p.sweeps.items():
            if s.raw is None or not np.isfinite(s.level_dbfs) or ref_lvl is None:
                continue
            env = dsp.envelope(s.raw.ref)
            on = env > 0.5 * env.max()
            lvl = 20 * np.log10(np.median(env[on]))
            d = lvl - (s.level_dbfs + (_f((s.trace.sweep_info or {}).get("reference_level")) or 0))
            tc = _tol(self.tol["level"]["convention_db"])
            self.add(id=f"{p.name}.level.raw_sweep_amplitude.{name}", group="level", path=p.name,
                     title=f"ac2 sweep {name}: loopback envelope vs level + reference level", value=d, unit="dB", tol=tc,
                     status=judge(d, tc),
                     meaning="the sweep's envelope amplitude on the reference input (median over its body) is "
                             "10^(L/20) × the loopback gain when L is a full-scale-sine level")
        self.table(f"{p.name}: level conventions", ["quantity", "value", "meaning"], rows)

    # ------------------------------------------------------------ delays
    def delays(self, p: PathData):
        f, S = self.f, self.src
        elec = p.kind != "speaker"
        lo, hi = (1000, 20000) if elec else (500, 5000)
        tl = _tol(self.tol["delay"]["electrical_us" if elec else "acoustic_us"])
        rows = []
        truth_tau = None  # arrival: the direct IR's peak (the definition REW's delay and ac2's design use)
        slope_tau = None  # delay in the phase: slope of the direct response
        if "direct (REW recording)" in S:
            Hd = S["direct (REW recording)"]
            m = np.isfinite(Hd)
            slope_tau = dsp.delay_from_phase(f[m], Hd[m], lo, hi)
            rows.append(["direct cross-spectrum, phase slope", f"{slope_tau*1e6:.3f}", f"{lo}–{hi} Hz on REW's recording"])
            truth_tau = direct_ir_peak(p.rec)
            rows.append(["direct IR peak (band-limited interpolation)", f"{truth_tau*1e6:.3f}",
                         "IFFT of M·R*/(|R|²+ε) of REW's recording"])
        elif p.truth and p.truth.get("direct_delay_s") is not None:
            truth_tau = p.truth["direct_delay_s"]
            rows.append(["direct cross-spectrum (documented)", f"{truth_tau*1e6:.3f}", "reference/"])
        for name, s in p.sweeps.items():
            info = s.trace.sweep_info or {}
            arr = _f(info.get("arrival"))
            H = S.get(f"ac2 sweep {name}")  # the arrival is back in this phase (see sources)
            in_phase = None
            total_phase = None
            if H is not None and arr is not None:
                m = np.isfinite(H)
                total_phase = dsp.delay_from_phase(f[m], H[m], lo, hi)
                in_phase = total_phase - arr
            if arr is None:
                continue
            total = total_phase if elec and total_phase is not None else arr
            rows.append([f"ac2 sweep {name}: arrival / delay left in phase", f"{arr*1e6:.3f} / {(in_phase or 0)*1e6:.3f}",
                         "arrival re the reference; the phase is referred to it"])
            if truth_tau is not None:
                d = (arr - truth_tau) * 1e6
                self.add(id=f"{p.name}.delay.ac2_arrival.{name}", group="delay", path=p.name,
                         title=f"ac2 sweep {name}: reported arrival vs direct", value=d, unit="µs", tol=tl,
                         status=judge(d, tl),
                         meaning="the arrival ac2 reports (sweep_info.arrival) against the direct cross-spectrum's "
                                 "delay; whole-sample builds report 0 for the 3.7 µs Xone path (the delay then "
                                 "stays inside the phase, see the next check)")
                if elec and slope_tau is not None:
                    d2 = (total - slope_tau) * 1e6
                    self.add(id=f"{p.name}.delay.ac2_total.{name}", group="delay", path=p.name,
                             title=f"ac2 sweep {name}: arrival + delay in phase vs direct", value=d2, unit="µs", tol=tl,
                             status=judge(d2, tl),
                             meaning="the delay ac2's response carries in total (reported arrival + phase slope "
                                     f"{lo}–{hi} Hz) against the direct response's phase slope over the same band")
        for rs in (p.rew, p.rew_live):
            if rs is None:
                continue
            if rs.meas_ir is not None:
                ir = rs.meas_ir
                pos, _ = dsp.fractional_peak(ir.h)
                tpk = ir.t[0] + pos / ir.fs
                rep = ir.delay
                ref_t = 0.0
                if rs.ref_ir is not None:
                    rpos, _ = dsp.fractional_peak(rs.ref_ir.h)
                    ref_t = rs.ref_ir.t[0] + rpos / rs.ref_ir.fs
                    rep = None if ir.delay is None or rs.ref_ir.delay is None else ir.delay - rs.ref_ir.delay
                rows.append([f"{rs.label}: IR peak (− ref peak)", f"{(tpk - ref_t)*1e6:.3f}", "band-limited interpolation"])
                if rep is not None:
                    rows.append([f"{rs.label}: reported delay", f"{rep*1e6:.3f}", "REW's own 'delay'"])
                if truth_tau is not None:
                    for what, v in (("IR peak", tpk - ref_t), ("reported delay", rep)):
                        if v is None:
                            continue
                        d = (v - truth_tau) * 1e6
                        offline = rs.ref_ir is not None
                        self.add(id=f"{p.name}.delay.rew.{rs.label}.{what}", group="delay", path=p.name,
                                 title=f"{rs.label}: {what} vs direct", value=d, unit="µs",
                                 tol=None if offline else tl, status="INFO" if offline else judge(d, tl),
                                 meaning="REW's arrival against the direct cross-spectrum"
                                         + ("; an offline import refers each channel to its own timing marker, so "
                                            "the path's delay is gone from it by design: context, not judged"
                                            if rs.ref_ir is not None else ""))
            elif rs.ref_fr is not None and rs.meas_fr is not None and "REW offline" in S:
                Hr = S["REW offline"]
                m = np.isfinite(Hr)
                tau = dsp.delay_from_phase(f[m], Hr[m], lo, hi)
                rows.append([f"{rs.label}: meas÷ref phase slope", f"{tau*1e6:.3f}",
                             "offline import: each channel re its own timing marker, so the in5−in2 delay is gone"])
                self.add(id=f"{p.name}.delay.rew_offline_phase", group="delay", path=p.name,
                         title=f"{rs.label}: delay left in meas÷ref phase", value=tau * 1e6, unit="µs", tol=None, status="INFO",
                         meaning="REW's offline import removes each channel's own delay; expected ≈ 0, not the path delay")
        self.table(f"{p.name}: delay / arrival (µs)", ["source", "µs", "how"], rows)

    # ------------------------------------------------------------ ETC
    def etc(self, p: PathData):
        sw = p.sweeps[p.primary]
        ir = sw.trace.ir
        rs = p.rew_live if (p.rew_live and p.rew_live.meas_ir is not None) else p.rew
        if ir is None or rs is None or rs.meas_ir is None:
            return
        t_a, etc_a = ir["t_s"], ir["etc_db"]
        etc_a = etc_a - np.nanmax(etc_a)
        dt = float(np.median(np.diff(t_a)))
        top = sw.end_hz or 40000.0
        rir = rs.meas_ir
        h = dsp.bandlimit(rir.h, rir.fs, min(top, rir.fs / 2 * 0.99))
        env = dsp.envelope(h)
        # REW's time re its peak, ac2's re its arrival: align peak to peak (delays are judged apart)
        pos, _ = dsp.fractional_peak(h)
        t_r = (np.arange(len(h)) - pos) / rir.fs
        # ac2's rows are cells [t, t + dt) re its arrival; REW's time re its own peak
        cells = dsp.etc_cells(t_r, env, t_a[0], dt, len(t_a))
        etc_r = 20 * np.log10(np.maximum(cells / np.nanmax(cells), 1e-30))
        rel = t_a
        # Where either ETC is near its own noise floor, each shows its own noise (sweep length,
        # repeats and ambient differ), not the response: compare only cells 10 dB above both
        # floors (the median of the late tail, 0.3-0.6 s, where an electrical path has decayed).
        tail = (rel > 0.3) & (rel < 0.6)
        fa = float(np.nanmedian(etc_a[tail])) if tail.any() else -np.inf
        fr = float(np.nanmedian(etc_r[tail])) if tail.any() else -np.inf
        lim_a, lim_r = max(-60.0, fa + 10), max(-60.0, fr + 10)
        m = (rel >= -0.005) & (rel <= 0.05) & (etc_a > lim_a) & (etc_r > lim_r) & np.isfinite(etc_r)
        if m.sum() < 5:
            return
        d = etc_a[m] - etc_r[m]
        med = float(np.median(np.abs(d)))
        tl = _tol(self.tol["etc"]["median_db"])
        # A loopback's response is flat up to the sweep's top: its ETC away from the peak is the
        # band edge's kernel (skirts ~1/(π·f_top·|t|), −42 dB at 1 ms for 40 kHz), shaped by each
        # tool's own fade-out and regularisation, not by the path. Judged on acoustic paths only.
        electrical = p.kind != "speaker"
        self.add(id=f"{p.name}.etc", group="ETC", path=p.name, title=f"ETC ac2 sweep vs {rs.label}", value=med,
                 unit="dB", tol=None if electrical else tl, status="INFO" if electrical else judge(med, tl),
                 meaning=f"median |Δ| over {m.sum()} cells of {dt*1e3:.3f} ms from −5 to 50 ms where both are within "
                         f"60 dB of their peak and 10 dB above their own floor (ac2 {fa:.1f}, REW {fr:.1f} dB); REW's IR band-limited to ac2's sweep top ({top:g} Hz) and taken as the "
                         "envelope maximum in each of ac2's cells"
                         + ("; an electrical path: these cells are the band edge's skirts, which each tool shapes "
                            "its own way, so context only" if electrical else ""),
                 detail={"max_abs": float(np.max(np.abs(d)))})
        rows = []
        for lvl in (-40, -60, -80):
            la = rel[(rel > 0.0005) & (etc_a > lvl)]
            lr = rel[(rel > 0.0005) & (etc_r > lvl)]
            rows.append([f"last above {lvl} dB", f"{la[-1]*1e3:.2f}" if len(la) else "—", f"{lr[-1]*1e3:.2f}" if len(lr) else "—"])
        tail = (rel > 0.3) & (rel < 0.6)
        if tail.any():
            rows.append(["median 300–600 ms", f"{np.nanmedian(etc_a[tail]):.1f} dB", f"{np.nanmedian(etc_r[tail]):.1f} dB"])
        self.table(f"{p.name}: ETC decay (ms re peak)", ["quantity", "ac2", rs.label], rows)
        self.series[p.name]["etc"] = {"t": rel, "ac2": etc_a, "rew": etc_r, "rew_label": rs.label}

    # ------------------------------------------------------------ group delay
    def group_delay(self, p: PathData):
        t = p.truth
        if not t:
            return
        tones = [x for x in t.get("tones", []) if x.get("gd_s") is not None]
        if not tones:
            return
        f, S = self.f, self.src
        fc = np.array([x["f"] for x in tones])
        truth = np.array([x["gd_s"] for x in tones])
        sig_t = np.array([self._sine_gd_sigma(t, x) for x in tones])
        ests, sig = {}, {}
        H = S[f"ac2 sweep {p.primary}"]
        m = np.isfinite(H)
        # A mains line adds a stationary phasor to its column: the phase there is not the
        # path's, so the fits leave those columns out.
        mf = m & ~self.mains_cols
        g, se, sp = dsp.gd_slope_err(f[mf], np.angle(H[mf]), fc, 1 / 12)
        gcen = dsp.gd_central(f[m], np.angle(H[m]))
        ests["ac2 as displayed (central difference)"] = np.interp(np.log(fc), np.log(f[m]), gcen)
        # the displayed derivative uses the two columns around fc; a mains column among them
        # makes that value the line's, not the path's
        fm = f[m]
        jc = np.searchsorted(fm, fc)
        cen_mains = np.array([bool(self.mains_cols[m][max(j - 2, 0):j + 2].any()) for j in jc])
        # the neighbour difference of two columns, each with the fit's phase noise σφ
        sig["ac2 as displayed (central difference)"] = np.sqrt(2) * sp / (2 * np.pi * fc * (2 ** self.frac - 2 ** -self.frac))
        ests["ac2 ±1/12-oct fit"], sig["ac2 ±1/12-oct fit"] = g, se
        tau = self.rew_put_back
        for rs in (p.rew_live, p.rew):
            if rs is None or rs.meas_fr is None:
                continue
            offline = rs.ref_fr is not None
            Hr = rs.meas_fr.H / rs.ref_fr.H if offline else rs.meas_fr.H
            Hr = Hr * np.exp(-2j * np.pi * rs.meas_fr.f * (tau if offline else 0.0))
            keep = ~dsp.mains_columns(rs.meas_fr.f, self.mains_hz, 1 / 48, guard_hz=MAINS_GUARD_HZ)
            ests[f"{rs.label} ±1/12-oct fit"], sig[f"{rs.label} ±1/12-oct fit"], _ = dsp.gd_slope_err(
                rs.meas_fr.f[keep], np.angle(Hr[keep]), fc, 1 / 12)
            if rs.meas_gd is not None:
                # REW's export is the group delay of one channel re the stimulus: for an offline
                # import meas ÷ ref is the difference of the two exports, plus the delay put back
                gm = rs.meas_gd
                gr = rs.ref_gd if offline else None
                v = []
                for c in fc:
                    sel = (gm.f >= c * 2 ** (-1 / 12)) & (gm.f <= c * 2 ** (1 / 12))
                    if not sel.any():
                        v.append(np.nan)
                        continue
                    x = np.nanmean(gm.mag[sel])
                    if gr is not None:
                        sr = (gr.f >= c * 2 ** (-1 / 12)) & (gr.f <= c * 2 ** (1 / 12))
                        x = x - np.nanmean(gr.mag[sr]) + tau if sr.any() else np.nan
                    v.append(x)
                ests[f"{rs.label} own GD export (±1/12-oct mean)"] = np.array(v)
        if "direct (REW recording)" in S:
            # the direct cross-spectrum at 96 ppo, then the same fit
            fd = dsp.log_centres(max(8.0, fc.min() / 2), min(40000, fc.max() * 2), 96)
            Hd, _ = dsp.cross_spectrum_bands(p.rec.meas, p.rec.ref, p.rec.fs, fd, 1 / 96,
                                             delay_s=self.direct_delay.get("direct (REW recording)", 0.0))
            ok = np.isfinite(Hd) & ~dsp.mains_columns(fd, self.mains_hz, 1 / 96, guard_hz=MAINS_GUARD_HZ)
            ests["direct ±1/12-oct fit"], sig["direct ±1/12-oct fit"], _ = dsp.gd_slope_err(
                fd[ok], np.angle(Hd[ok]), fc, 1 / 12)
        # The sine's group delay is the phase difference of two probes at f·2^(±1/48); a mains
        # line within the probe's analysis lobe (about 3 bins of 1/probe_s) adds to that phase.
        sine_mains = np.array([self._sine_near_mains(t, x) for x in tones])
        rows = []
        tf_ = _tol(self.tol["gd"]["fit_rel"])
        tc_ = _tol(self.tol["gd"]["central_rel"])
        ta_ = _tol(self.tol["gd"]["high_abs_us"])
        for i, c in enumerate(fc):
            row = [f"{c:g}", f"{truth[i]*1e6:.1f}" + (f" ±{2*sig_t[i]*1e6:.1g}" if np.isfinite(sig_t[i]) else "")]
            for name, gv in ests.items():
                v = gv[i]
                row.append("—" if not np.isfinite(v) else f"{v*1e6:.1f} ({(v/truth[i]-1)*100:+.0f} %)")
                if c < 16 or not np.isfinite(v):
                    continue
                own = "own GD export" in name
                s_est = sig.get(name, np.full(len(fc), np.nan))[i]
                u2 = 2 * np.sqrt(np.nan_to_num(s_est) ** 2 + np.nan_to_num(sig_t[i]) ** 2)
                if c >= 1000:
                    d, tol, unit, u = (v - truth[i]) * 1e6, ta_, "µs", u2 * 1e6
                else:
                    d, tol, unit, u = v / truth[i] - 1, (tc_ if "central" in name else tf_), "rel", u2 / truth[i]
                st = "INFO" if own else judge_u(d, tol, u)
                why = ""
                if not own and sine_mains[i] is not None:
                    st, why = "INCONCLUSIVE", f": the sine pair lies within {sine_mains[i]:.1f} Hz of a mains line, so the truth is the line's"
                elif not own and "central" in name and cen_mains[i]:
                    st, why = "INCONCLUSIVE", ": a mains column is among the two columns the displayed derivative uses"
                self.add(id=f"{p.name}.gd.{name}.{c:g}", group="group delay", path=p.name,
                         title=f"{name} at {c:g} Hz vs steady sine", value=float(d), unit=unit,
                         tol=None if own else tol, status=st,
                         meaning=f"sine {truth[i]*1e6:.1f} µs (phase of meas÷ref at f·2^(±1/48)), {name} {v*1e6:.1f} µs; "
                                 f"uncertainty of the comparison (2σ, sine probe floor and the estimate's own phase "
                                 f"scatter) ±{(u2/truth[i]*100 if unit == 'rel' else u):.1f} {'%' if unit == 'rel' else 'µs'}"
                                 + (why or (": wider than the pass limit, so neither a pass nor a fail can be read"
                                            if st == "INCONCLUSIVE" else ""))
                                 + ("; REW's per-bin derivative of its own exports, context only" if own else "")
                                 + (f"; ac2's displayed derivative takes −Δφ/Δω between neighbouring 1/{self.ppo}-oct columns"
                                    if "central" in name else ""),
                         detail={"uncertainty_2sigma": float(u)})
            rows.append(row)
        self.series[p.name]["gd"] = {"fc": fc, "truth": truth, "ests": ests}
        self.table(f"{p.name}: group delay vs steady sine (µs)", ["f Hz", "sine ±2σ"] + list(ests), rows,
                   f"All on the absolute reference: ac2's arrival put back, REW offline {tau*1e6:+.2f} µs put back "
                   "(the direct delay its timing markers removed). Fit: least-squares slope of the unwrapped phase "
                   "over f·2^(±1/12). A comparison whose 2σ uncertainty exceeds its pass limit is INCONCLUSIVE.")

    def _sine_near_mains(self, t: dict, tone: dict) -> float | None:
        """Distance (Hz) from the sine pair to the nearest mains line when it is inside the
        probe's analysis lobe, else None."""
        probe = next((x.get("probe_s") for x in (t.get("plan") or {}).get("tones", [])
                      if abs(x.get("f", -1) - tone["f"]) < 1e-6), None) or 2.0
        lobe = 3.0 / probe
        d = min((abs(tone["f"] * 2 ** (s / 48) - h) for s in (-1, 1) for h in self.mains_hz), default=np.inf)
        return float(d) if d <= lobe else None

    @staticmethod
    def _sine_gd_sigma(t: dict, tone: dict) -> float:
        """σ of the sine's group delay from its probe floor (the pair takes are probe-length)."""
        if tone.get("gd_sigma_s") is not None:
            return float(tone["gd_sigma_s"])
        for x in (t.get("plan") or {}).get("tones", []):
            if abs(x.get("f", -1) - tone["f"]) < 1e-6 and x.get("probe_floor_dbr") is not None:
                return dsp.sine_gd_sigma(tone["f"], float(x["probe_floor_dbr"]))
        return float("nan")

    # ------------------------------------------------------------ harmonics
    def harmonics(self, p: PathData):
        margin = float(self.tol["distortion"]["margin_db"])
        th = _tol(self.tol["harmonics"]["vs_truth_db"])
        t = p.truth or {}
        tones = [x for x in t.get("tones", []) if x.get("h_dbr")]
        doc_floor = t.get("floor_dbr")
        rows = []
        sources = {}
        for name, s in p.sweeps.items():
            b = s.trace.freq
            if "h2_db" in b:
                sources[f"ac2 sweep {name}"] = s
        # above the top tone the path's transfer at k·f comes from a direct cross-spectrum of
        # a raw recording (independent of either app), on the same phase reference as the tones
        dkey = next((x for x in ("direct (REW recording)", f"direct (ac2 capture {p.primary})") if x in self.src), None)

        def transfer(fk):
            if dkey is None or not self.f[0] <= fk <= self.f[-1]:
                return None
            return complex(self.at(self.f, self.src[dkey], np.array([fk]))[0])

        for tone in tones:
            fc = tone["f"]
            for k in range(2, 6):
                tv = tone["h_dbr"].get(str(k), tone["h_dbr"].get(k))
                if tv is None:
                    continue
                tfl = (tone.get("floor_dbr") or {}).get(str(k)) if isinstance(tone.get("floor_dbr"), dict) else doc_floor
                ct = dsp.classify(tv, tfl if tfl is not None else np.nan, margin)
                rows.append([f"{fc:g}", f"H{k}", "steady sine", _fmt_lvl(tone.get("meas_level_dbfs")), _fmt_h(ct),
                             _fmt(ct.get("floor")), _fmt(ct.get("margin")), ct["kind"]])
                rv = (tone.get("h_ref_dbr") or {}).get(str(k))
                if rv is not None:
                    rows.append([f"{fc:g}", f"H{k}", "steady sine, reference input", _fmt_lvl(tone.get("ref_level_dbfs")),
                                 f"{rv:.1f}", "—", "—", "info"])
                # ac2 divides by the measured reference, so its truth is the meas harmonic less
                # the reference's carried through the path at k·f
                net = dual_channel_harmonic_dbr(tones, tone, k, transfer)
                ct_net = ct
                if net is not None:
                    ct_net = {**dsp.classify(net, tfl if tfl is not None else np.nan, margin),
                              "label": f"sine net of reference (meas alone {tv:.1f})"}
                    rows.append([f"{fc:g}", f"H{k}", "steady sine, net of reference", "—", _fmt_h(ct_net),
                                 _fmt(ct_net.get("floor")), _fmt(ct_net.get("margin")), ct_net["kind"]])
                for name, s in sources.items():
                    b = s.trace.freq
                    i = _nearest_finite(b["freq_hz"], b[f"h{k}_db"], fc)
                    if i is None:
                        continue
                    hv, hf = b[f"h{k}_db"][i], b[f"h{k}_floor_db"][i]
                    ca = dsp.classify(hv, hf, margin)
                    fund = None
                    ri = _f((s.trace.sweep_info or {}).get("reference_level"))
                    if ri is not None and np.isfinite(s.level_dbfs):
                        fund = s.level_dbfs + ri + b["mag_db"][i]
                    rows.append([f"{b['freq_hz'][i]:.1f}", f"H{k}", name, _fmt_lvl(fund), _fmt_h(ca), _fmt(ca.get("floor")),
                                 _fmt(ca.get("margin")), ca["kind"]])
                    self._cmp_h(p, name, fc, k, ca, ct_net, th)
                for rs in (p.rew, p.rew_live):
                    if rs is None or rs.meas_dist is None:
                        continue
                    d = rs.meas_dist
                    i = int(np.argmin(np.abs(np.log(d.f / fc))))
                    if abs(np.log2(d.f[i] / fc)) > 1 / 6 or f"H{k}" not in d.cols:
                        continue
                    cr = dsp.classify(d.cols[f"H{k}"][i], d.cols.get("Noise", np.full(len(d.f), np.nan))[i], margin)
                    rows.append([f"{d.f[i]:g}", f"H{k}", rs.label, _fmt_lvl(d.cols.get("Fundamental", [np.nan] * len(d.f))[i]),
                                 _fmt_h(cr), _fmt(cr.get("floor")), _fmt(cr.get("margin")), cr["kind"]])
                    self._cmp_h(p, rs.label, fc, k, cr, ct, th)
        self.table(f"{p.name}: harmonics vs floor (dBr)", ["f Hz", "H", "source", "fundamental", "reading", "floor",
                   "margin", "kind"], rows,
                   f"A reading counts as a value only at floor + {margin:g} dB or above; below, it is an upper bound "
                   "'< X' and comparisons resting on it are INCONCLUSIVE. REW's floor is its 'Noise' column. "
                   "Fundamental: dBFS at the measurement input (ac2: level + reference level + meas÷ref).")
        # floor cross-check from ac2's raw capture
        for name, s in p.sweeps.items():
            if s.raw is None:
                continue
            self._floor_crosscheck(p, name, s)
        self._mains_flags(p)

    def _cmp_h(self, p, name, fc, k, ca, ct, th):
        # A sweep reads harmonic k as the energy of a band around k·f at least ~30 Hz wide; a
        # mains line inside it is read as harmonic, while the steady sine resolves k·f alone.
        fk = k * fc
        half = dsp.sweep_harmonic_half_band(fk, MAINS_GUARD_HZ)
        line = next((h for h in getattr(self, "mains_hz", []) if abs(h - fk) <= half), None)
        if line is not None and ca["kind"] != "none":
            self.add(id=f"{p.name}.h{k}.{name}.{fc:g}", group="harmonics", path=p.name,
                     title=f"{name} H{k} at {fc:g} Hz vs steady sine", value=None, unit="dB", tol=th,
                     status="INCONCLUSIVE",
                     meaning=f"the mains line at {line:g} Hz lies inside the band ±{half:.0f} Hz around H{k} = "
                             f"{fk:.1f} Hz that a sweep reads as the harmonic",
                     detail={"ac2_or_rew": ca, "sine": ct, "mains_hz": line})
            return
        if ca["kind"] == "value" and ct["kind"] == "value":
            d = ca["value"] - ct["value"]
            self.add(id=f"{p.name}.h{k}.{name}.{fc:g}", group="harmonics", path=p.name,
                     title=f"{name} H{k} at {fc:g} Hz vs steady sine", value=d, unit="dB", tol=th, status=judge(d, th),
                     meaning=f"{name} {ca['value']:.1f} dBr (floor {_fmt(ca.get('floor'))}), "
                             f"{ct.get('label', 'sine')} {ct['value']:.1f} dBr")
        elif ca["kind"] != "none" and ct["kind"] != "none":
            short = max(ca.get("shortfall") or 0, ct.get("shortfall") or 0)
            claim = ""
            if ca["kind"] == "value" and ct["kind"] == "bound" and ca["value"] > ct["bound"] + th[1]:
                claim = f"; {name} claims {ca['value']:.1f} dBr above the sine's bound {ct['bound']:.1f}"
                st = "FAIL"
            else:
                st = "INCONCLUSIVE"
            self.add(id=f"{p.name}.h{k}.{name}.{fc:g}", group="harmonics", path=p.name,
                     title=f"{name} H{k} at {fc:g} Hz vs steady sine", value=None, unit="dB", tol=th, status=st,
                     meaning=f"a bound on one side (shortfall {short:.1f} dB below floor + margin){claim}",
                     detail={"ac2_or_rew": ca, "sine": ct})

    def _floor_crosscheck(self, p, name, s: Sweep):
        info = s.trace.sweep_info or {}
        L = _f(info.get("rate"))
        if L is None:
            return
        fs = s.raw.fs
        ref, meas = s.raw.ref, s.raw.meas
        env = np.abs(ref)
        # The record ac2 analyses per repeat: 0.1 s before the emitted sweep, the sweep (its
        # emitted length, which starts below the asked band), then the post-roll. The sweep's
        # end is sharp (short fade-out); its start fades in, so it is found from the end.
        dur = _f(info.get("duration")) or (s.duration_s or 5.5)
        pre, post = info.get("window_pre", 0.0083229), info.get("window_post", 0.0916771)
        fine = s.lf_harmonics == "fine"
        own = {k: dsp.lf_window(L, k) for k in range(2, 6)} if fine else {}
        post_roll = max(1.0, 4 * (pre + post), *(4 * (a + b) for a, b in own.values()))
        repeats = int(info.get("repeats") or 1)
        last = len(env) - int(np.argmax(env[::-1] > 0.1 * env.max()))
        period = int(round((dur + post_roll) * fs))
        start = last - int(round(dur * fs)) - (repeats - 1) * period
        a0 = start - int(0.1 * fs)
        n_cut = int(round((0.1 + dur + post_roll) * fs))
        if a0 < 0 or a0 + n_cut > len(ref):
            self.notes.append(f"{p.name}: {name}: the raw capture does not hold the first repeat's whole record: "
                              "floor cross-check skipped")
            return
        seg_ref, seg_meas = ref[a0:a0 + n_cut], meas[a0:a0 + n_cut]
        # Noise only, as long as that record: the capture's own silence before the sweep when it
        # is long enough, else the silent recording of the sine stage (same inputs, same chain).
        if a0 - int(0.2 * fs) >= n_cut:
            q = slice(a0 - int(0.2 * fs) - n_cut, a0 - int(0.2 * fs))
            noise_meas, noise_ref, src = meas[q], ref[q], "the capture's silence before the sweep"
        elif p.noise is not None and len(p.noise.meas) >= n_cut:
            noise_meas, noise_ref, src = p.noise.meas[:n_cut], p.noise.ref[:n_cut], "the sine stage's silent recording"
        else:
            self.notes.append(f"{p.name}: {name}: no noise-only recording as long as the sweep's record "
                              f"({n_cut/fs:.1f} s): floor cross-check skipped")
            return
        # A constant offset on an input is not noise: it has no power in any harmonic band, but
        # divided by the reference's regularised spectrum near 0 Hz it spreads into the LF
        # windows. Each noise-only record is taken about its own mean.
        noise_meas, noise_ref = noise_meas - np.mean(noise_meas), noise_ref - np.mean(noise_ref)
        h = dsp.deconvolve(seg_ref, seg_meas)
        # both inputs' noise: ac2 divides by its captured reference, whose noise lands in the floor too
        hn = dsp.deconvolve_noise(seg_ref, seg_meas, noise_ref, noise_meas)
        d = int(np.argmax(np.abs(h[: int(0.05 * fs)])))
        b = s.trace.freq
        fsel = b["freq_hz"][(b["freq_hz"] >= 20) & (b["freq_hz"] <= 10000)][::max(1, self.ppo // 12)]
        r = dsp.sweep_harmonics(h, d, fs, L, fsel, pre=pre, post=post, noise_h=hn, repeats=repeats)
        # Fine: an order in its own window where the shared band had to be widened to the
        # window's resolution (3 cells against 1/24 octave), as ac2 picks it.
        half = 2 ** (1 / 48)
        for k, (a_, b_) in own.items():
            if k > 3:
                continue
            ro = dsp.sweep_harmonics(h, d, fs, L, fsel, pre=a_, post=b_, noise_h=hn, repeats=repeats)
            sel = 3 / (pre + post) > k * fsel * (half - 1 / half)
            r["floor"][k] = np.where(sel, ro["floor"][k], r["floor"][k])
        lim = float(self.tol["distortion"]["floor_disagree_db"])
        rows, flagged = [], 0
        for k in (2, 3):
            ac2f = np.interp(fsel, b["freq_hz"], b[f"h{k}_floor_db"])
            mine = r["floor"][k]
            dd = ac2f - mine
            bad = np.abs(dd) > lim
            flagged += int(np.nansum(bad))
            for i in np.where(bad)[0][:20]:
                rows.append([f"{fsel[i]:.1f}", f"H{k}", f"{ac2f[i]:.1f}", f"{mine[i]:.1f}", f"{dd[i]:+.1f}"])
            self.add(id=f"{p.name}.floor.{name}.h{k}", group="floor", path=p.name,
                     title=f"ac2 sweep {name}: H{k} floor vs raw-capture floor", value=float(np.nanmedian(dd)),
                     unit="dB", tol=(lim / 2, lim), status=judge(float(np.nanmedian(dd)), (lim / 2, lim)),
                     meaning="ac2's floor: noise windows in the silence after its linear response. This one: "
                             f"{src}, as long as ac2's record ({n_cut/fs:.2f} s), deconvolved by the same reference with "
                             "the reference input's noise scaled by the response (ac2 divides by a noisy reference too), "
                             f"8 windows at the harmonic's own lag, the floor band 1/3 octave like ac2's"
                             + (", each order in its own window where ac2 uses it (LF harmonics fine)" if fine else "")
                             + (f", ÷ {repeats} for the mean of {repeats} repeats" if repeats > 1 else "")
                             + f". Value: median difference; {int(np.nansum(bad))} columns differ by more than {lim:g} dB",
                     detail={"columns_flagged": int(np.nansum(bad))})
        self.table(f"{p.name}: {name}: floor columns ac2 vs raw capture differing > {lim:g} dB",
                   ["f Hz", "H", "ac2 floor", "raw-capture floor", "Δ"], rows)

    def _mains_flags(self, p: PathData):
        lines = []
        if p.noise is not None and p.mains_hz:
            lines = dsp.mains_lines(p.noise.meas, p.noise.fs, p.mains_hz)
        if not lines:
            return
        hz = np.array([x["hz"] for x in lines])
        rows = []
        for name, s in p.sweeps.items():
            b = s.trace.freq
            for k in range(2, 6):
                kf = k * b["freq_hz"]
                lo, hi = kf * 2 ** -self.frac, kf * 2 ** self.frac
                hit = [(i, hz[(hz >= lo[i]) & (hz <= hi[i])]) for i in range(len(kf))]
                for i, hh in hit:
                    if len(hh) and b["freq_hz"][i] <= 2000:
                        rows.append([name, f"{b['freq_hz'][i]:.1f}", f"H{k}", ", ".join(f"{x:g}" for x in hh)])
        self.table(f"{p.name}: sweep columns whose harmonic band holds a mains line", ["sweep", "f Hz", "H", "line Hz"],
                   rows[:200], f"mains lines found in the noise recording: {', '.join(f'{x:g}' for x in hz)} Hz")

    # ------------------------------------------------------------ digital DUT: analytic truth
    def dut_harmonics(self, p: PathData):
        """Every harmonic source on the digital path against the DUT's analytic truth, each at
        its own level and at the exact frequency of the value compared (dut.py)."""
        d = p.dut
        coef, fs = d["coefficients"], float(d["fs"])
        margin = float(self.tol["distortion"]["margin_db"])
        tc = self.tol.get("dut", {})
        ts, tw = _tol(tc["sine_db"]), _tol(tc["sweep_db"])
        ppo = int(tc.get("grid_per_octave", 3))
        stop = d.get("stop") or {}
        xr = stop.get("xruns")
        # an xrun leaves a stale block in dut_out: every take that spans it reads a wrong spectrum
        self.add(id=f"{p.name}.dut.xruns", group="dut", path=p.name, title="DUT xruns over the path's stages",
                 value=None if xr is None else float(xr), unit="", tol=None,
                 status="INFO" if xr == 0 else "WARN",
                 meaning=("none: the DUT ran every block" if xr == 0 else
                          f"{xr} xrun(s): JACK left a stale block in dut_out, so the path's results are suspect"
                          if xr is not None else "the DUT printed no `xruns` line (crashed or killed?): results suspect"),
                 detail={"stop": stop})
        rows: list[list] = []
        t = p.truth or {}
        if t.get("tones") and t.get("emit_dbfs") is not None:
            amp = levels.peak_amplitude(float(t["emit_dbfs"]))
            for tone in t["tones"]:
                truth = dut.analytic(float(tone["f"]), amp, coef, fs)
                for k in range(2, 6):
                    if k not in truth:
                        continue
                    tv = (tone.get("h_dbr") or {}).get(str(k))
                    ca = dsp.classify(np.nan if tv is None else tv, (tone.get("floor_dbr") or {}).get(str(k), np.nan),
                                      margin)
                    self._cmp_dut(p, "steady sine", float(tone["f"]), float(tone["f"]), k, ca, truth[k], ts, rows,
                                  coef, fs, sweep=False, level=float(t["emit_dbfs"]))
        elif d.get("tones") and d.get("amp") is not None and d.get("level_dbfs") is not None:
            # the sine stage planned these tones and left no results (a failed or discarded take)
            for tone in d["tones"]:
                truth = dut.analytic(float(tone["f"]), float(d["amp"]), coef, fs)
                for k, tv in truth.items():
                    self._cmp_dut(p, "steady sine", float(tone["f"]), float(tone["f"]), k, {"kind": "none"}, tv, ts,
                                  rows, coef, fs, sweep=False, level=float(d["level_dbfs"]))
        tones = [float(x["f"]) for x in t.get("tones", [])] or [float(x["f"]) for x in d.get("tones", [])]
        curves = []
        for name, s in p.sweeps.items():
            b = s.trace.freq
            if "h2_db" in b and np.isfinite(s.level_dbfs):
                lo = (s.start_hz or 10.0) * 2 ** (1 / 12)
                hi = (s.end_hz or fs / 2) * 2 ** (-1 / 12)
                curves.append((f"ac2 sweep {name}", s.level_dbfs, b["freq_hz"],
                               {k: (b[f"h{k}_db"], b[f"h{k}_floor_db"]) for k in range(2, 6) if f"h{k}_db" in b},
                               lo, hi, 1 / 24, hi))
        if p.rew is not None and p.rew.meas_dist is not None and np.isfinite(p.rew.level_dbfs):
            r = p.rew.meas_dist
            nf = r.cols.get("Noise", np.full(len(r.f), np.nan))
            curves.append((p.rew.label, p.rew.level_dbfs, r.f,
                           {k: (r.cols[f"H{k}"], nf) for k in range(2, 6) if f"H{k}" in r.cols},
                           10.0 * 2 ** (1 / 12), 40000.0 * 2 ** (-1 / 12), 1 / 6, 40000.0 * 2 ** (-1 / 12)))
        for name, lvl, f, hs, lo, hi, near, top in curves:
            amp = levels.peak_amplitude(float(lvl))
            for fc in tones:
                for k in range(2, 6):
                    # a reading is due where the source played both fc and k·fc, below fs/2
                    tr = dut.analytic(fc, amp, coef, fs).get(k)
                    due = tr is not None and lo <= fc <= hi and k * fc <= top
                    hv, hf = hs.get(k, (None, None))
                    i = None if hv is None else _nearest_finite(f, hv, fc, max_oct=near)
                    if i is None or not lo <= f[i] <= hi:
                        if due:
                            self._cmp_dut(p, name, fc, fc, k, {"kind": "none"}, tr, tw, rows, coef, fs, sweep=True,
                                          level=float(lvl))
                        continue
                    tr = dut.analytic(float(f[i]), amp, coef, fs).get(k)
                    if tr is None:
                        continue
                    self._cmp_dut(p, name, fc, float(f[i]), k, dsp.classify(hv[i], hf[i], margin), tr, tw, rows,
                                  coef, fs, sweep=True, level=float(lvl))
            # the log grid: one summary row per harmonic, every point in the table
            grid = dsp.log_centres(max(lo, 10.0), min(hi, fs / 2), ppo)
            for k, (hv, hf) in hs.items():
                # columns where the pre-filter is not flat read the truth only in the
                # instantaneous-frequency approximation (Wiener model): shown, not judged
                ds, fails, bounds, seen, mds, mfails = [], [], 0, set(), [], []
                for fc in grid:
                    i = _nearest_finite(f, hv, fc, max_oct=min(near, 1 / (2 * ppo)))
                    if i is None or i in seen:
                        continue
                    seen.add(i)
                    tr = dut.analytic(float(f[i]), amp, coef, fs).get(k)
                    if tr is None:
                        continue
                    ca = dsp.classify(hv[i], hf[i], margin)
                    model = _pre_note(coef, fs, float(f[i])) is not None
                    if ca["kind"] == "value":
                        (mds if model else ds).append((float(f[i]), ca["value"] - tr))
                    elif ca["kind"] == "bound":
                        bounds += 1
                        if tr > ca["bound"] + tw[1]:
                            (mfails if model else fails).append(float(f[i]))
                    rows.append([f"{f[i]:.1f}", f"H{k}", name + " (grid)", _fmt_h(ca), _fmt(ca.get("floor")),
                                 f"{tr:.2f}", f"{ca['value'] - tr:+.2f}" if ca["kind"] == "value" else "—",
                                 _pre_note(coef, fs, float(f[i])) or ""])
                fm, dm = max(mds, key=lambda x: abs(x[1])) if mds else (None, None)
                mnote = ((f"; {len(mds) + len(mfails)} model-limited columns not judged (pre-filter > 0.1 dB from flat, "
                          "Wiener model)") + (f", largest |Δ| {dm:+.2f} dB at {fm:.1f} Hz" if mds else "")
                         + (f", truth above the bound at {', '.join(f'{x:.0f}' for x in mfails)} Hz" if mfails else "")
                         if mds or mfails else "")
                if not ds and not fails:
                    if bounds or mds or mfails:
                        self.add(id=f"{p.name}.dut.grid.h{k}.{name}", group="dut", path=p.name,
                                 title=f"{name} H{k} vs analytic, log grid", value=None, unit="dB", tol=tw,
                                 status="INCONCLUSIVE",
                                 meaning="no quasi-static grid column holds a value"
                                         + (f"; {bounds} columns are bounds" if bounds else "") + mnote)
                    continue
                fw, dw = max(ds, key=lambda x: abs(x[1])) if ds else (None, None)
                st = "FAIL" if fails else judge(dw, tw)
                self.add(id=f"{p.name}.dut.grid.h{k}.{name}", group="dut", path=p.name,
                         title=f"{name} H{k} vs analytic, log grid", value=dw, unit="dB", tol=tw, status=st,
                         meaning=(f"largest |Δ| of {len(ds)} columns {ppo}/octave {grid[0]:.0f} Hz – {grid[-1]:.0f} Hz"
                                  + (f" at {fw:.1f} Hz" if fw else "")
                                  + (f"; {bounds} columns are bounds" if bounds else "")
                                  + (f"; the truth stands above the reading's bound at {', '.join(f'{x:.0f}' for x in fails)} Hz"
                                     if fails else "")
                                  + mnote),
                         detail={"columns": [[a, b] for a, b in ds], "bound_fails_hz": fails,
                                 "model_limited": [[a, b] for a, b in mds], "model_bound_fails_hz": mfails})
        self.table(f"{p.name}: harmonics vs the DUT's analytic truth (dBr)",
                   ["f Hz", "H", "source", "reading", "floor", "truth", "Δ", "pre-filter"], rows,
                   "Truth: dut.py's analytic Hk at each source's own level and at the column's frequency. "
                   "Δ = reading − truth, only where the reading is a value (floor + margin).")

    def _cmp_dut(self, p, name, fc, fcol, k, ca, truth, tol, rows, coef, fs, sweep: bool, level: float):
        note = _pre_note(coef, fs, fcol) if sweep else None
        d = ca["value"] - truth if ca["kind"] == "value" else None
        rows.append([f"{fcol:.1f}", f"H{k}", name, _fmt_h(ca) if ca["kind"] != "none" else "missing",
                     _fmt(ca.get("floor")), f"{truth:.2f}", f"{d:+.2f}" if d is not None else "—", note or ""])
        pre_db = float(20 * np.log10(abs(dut.response(coef.get("pre", []), [fcol], fs)[0])))
        case = ((self.run.manifest or {}).get("paths", {}).get(p.name) or {}).get("case")

        def cell(status):
            self.dut_cells.append(dutcov.entry(p.name, case, name, k, fc, status, d, truth, pre_db, sweep, level))

        if ca["kind"] == "none":
            cell("MISSING")
            return
        where = f" (column {fcol:.1f} Hz)" if abs(fcol - fc) > 1e-6 else ""
        base = dict(id=f"{p.name}.dut.h{k}.{name}.{fc:g}", group="dut", path=p.name,
                    title=f"{name} H{k} at {fc:g} Hz vs analytic", unit="dB", tol=tol)
        if d is not None:
            st = judge(d, tol)
            self.add(**base, value=d, status=st,
                     meaning=f"{name} {ca['value']:.2f} dBr{where}, truth {truth:.2f} dBr"
                             + (f"; {note}" if note else ""))
        elif truth > ca["bound"] + tol[1]:
            st = "FAIL"
            self.add(**base, value=None, status=st,
                     meaning=f"{name} reads below {ca['bound']:.1f} dBr{where}; the truth is {truth:.2f} dBr, above it",
                     detail={"reading": ca})
        else:
            st = "INCONCLUSIVE"
            self.add(**base, value=None, status=st,
                     meaning=f"a bound (shortfall {ca.get('shortfall', 0):.1f} dB below floor + margin){where}; "
                             f"truth {truth:.2f} dBr", detail={"reading": ca})
        cell(st)

    def dut_coverage(self):
        """Per harmonic order and analyser, over every DUT case of the run: how many cells
        were judged and the worst signed error (dutcov.py). The cells go in the check's
        detail so runs at other levels can be merged into one grid."""
        tc = self.tol.get("dut", {})
        tols = {"steady sine": _tol(tc["sine_db"])}
        cells = self.dut_cells
        for k in sorted({e["k"] for e in cells}):
            for src in sorted({e["class"] for e in cells if e["k"] == k}):
                sel = [e for e in cells if e["k"] == k and e["class"] == src]
                s = dutcov.summarise(sel)
                at = s["worst_at"]
                self.add(id=f"dut.coverage.h{k}.{src}", group="dut coverage", path="dut",
                         title=f"{src} H{k} vs analytic: coverage over the DUT cases", value=s["worst"], unit="dB",
                         tol=tols.get(src, _tol(tc["sweep_db"])), status=s["status"],
                         meaning=(f"{s['judged']} of {s['total']} cells judged ({s['inconclusive']} inconclusive, "
                                  f"{s['missing']} missing)"
                                  + (f"; worst quasi-static Δ at {at['f']:g} Hz ({at['source']}"
                                     + (f", case {at['case']})" if at['case'] else ")")
                                     if at else "; no quasi-static cell judged")
                                  + (f"; {s['model_judged']} model-limited cells judged, worst {s['model_worst']:+.2f} dB"
                                     if s["model_judged"] else "")),
                         detail={"summary": s, "cells": sel})
        cols, rows = dutcov.grid(cells)
        self.table("DUT coverage: harmonic order × tone, all cases", cols, rows, dutcov.NOTE)

    # ------------------------------------------------------------ LF H2 onset
    def lf_h2_onset(self, p: PathData):
        t = p.truth or {}
        tones = [x for x in t.get("tones", []) if x.get("h_dbr") and "2" in x["h_dbr"]]
        # ac2 divides by the measured reference: its truth is the meas H2 net of the reference's
        net = {id(x): dual_channel_harmonic_dbr(t.get("tones", []), x, 2) for x in tones}
        pts = sorted((x["f"], net[id(x)] if net[id(x)] is not None else x["h_dbr"]["2"]) for x in tones)
        if len(pts) < 2:
            return
        tf_, tv = np.array([a for a, _ in pts]), np.array([b for _, b in pts])
        freqs = np.array([16, 20, 22, 25, 31.5, 40, 50, 63, 80, 100])
        tl = _tol(self.tol["harmonics"]["lf_onset_excess_db"])
        margin = float(self.tol["distortion"]["margin_db"])
        settle_s = float(self.tol["harmonics"]["lf_onset_settle_s"])
        rows = []
        for name, s in p.sweeps.items():
            b = s.trace.freq
            if "h2_db" not in b:
                continue
            row = [name, f"{s.start_hz or float('nan'):g} Hz / {s.duration_s or float('nan'):g} s"]
            onset = None
            for fc in freqs:
                i = _nearest_finite(b["freq_hz"], b["h2_db"], fc)
                if i is None:
                    row.append("—")
                    continue
                hv, hf = b["h2_db"][i], b["h2_floor_db"][i]
                if fc < tf_[0] or fc > tf_[-1]:
                    row.append("—")
                    continue
                truth = float(np.interp(np.log(fc), np.log(tf_), tv))
                c = dsp.classify(hv, hf, margin)
                if c["kind"] == "value":
                    ex = hv - truth
                    row.append(f"{ex:+.1f}")
                    if ex > tl[1]:
                        onset = fc
                else:
                    row.append(f"<{c.get('bound', np.nan):.0f}")
                if fc == 22:
                    lag = _settle_lag_s(s, fc)
                    if c["kind"] == "value" and lag is not None and lag < settle_s:
                        ex = hv - truth
                        self.add(id=f"{p.name}.lf_h2.{name}", group="LF H2", path=p.name,
                                 title=f"ac2 sweep {name}: H2 excess at 22 Hz", value=ex, unit="dB", tol=tl,
                                 status="INFO",
                                 meaning=f"ac2 H2 {hv:.1f} dBr against the sine truth {truth:.1f} dBr; not judged: this "
                                         f"sweep reaches 22 Hz {lag:.2f} s after full level, inside the path's own "
                                         f"settling ({settle_s:g} s), where a steady sine reads the same rise")
                    elif c["kind"] == "value":
                        ex = hv - truth
                        self.add(id=f"{p.name}.lf_h2.{name}", group="LF H2", path=p.name,
                                 title=f"ac2 sweep {name}: H2 excess at 22 Hz", value=ex, unit="dB", tol=tl,
                                 status=judge(max(ex, 0.0), tl),
                                 meaning=f"ac2 H2 {hv:.1f} dBr (floor {hf:.1f}) against the sine truth interpolated in "
                                         f"log f ({truth:.1f} dBr): positive = ac2 overstates; seen on 10 Hz / 5.5 s sweeps")
                    else:
                        self.add(id=f"{p.name}.lf_h2.{name}", group="LF H2", path=p.name,
                                 title=f"ac2 sweep {name}: H2 excess at 22 Hz", value=None, unit="dB", tol=tl,
                                 status="INCONCLUSIVE", meaning=f"ac2's H2 at 22 Hz is a bound (shortfall {c.get('shortfall', 0):.1f} dB)")
            row.append(f"{onset:g}" if onset else "none")
            rows.append(row)
        self.table(f"{p.name}: LF H2 excess over the sine truth (dB) by sweep variant", ["sweep", "start / duration"]
                   + [f"{x:g}" for x in freqs] + ["overstates up to (Hz)"], rows,
                   f"truth points: {', '.join(f'{a:g} Hz {b:.0f}' for a, b in pts)} dBr (linear in log f between). "
                   + (t.get("level_note") or ""))

    # ------------------------------------------------------------ coherence
    def coherence(self, p: PathData):
        if "ac2 TF" not in self.coh:
            return
        c = self.coh["ac2 TF"]
        f = self.f
        m = (f >= 31) & (f <= 20000) & np.isfinite(c)
        if not m.any():
            return
        frac = float(np.mean(c[m] >= 0.99))
        need = float(self.tol["coherence"]["min_fraction_099"])
        self.add(id=f"{p.name}.coherence.tf", group="coherence", path=p.name, title="ac2 TF: share of columns with γ² ≥ 0.99",
                 value=frac, unit="", tol=None, status="PASS" if frac >= need else "INFO",
                 meaning=f"31 Hz – 20 kHz, {m.sum()} columns; min γ² {np.min(c[m]):.5f}. A one-shot sweep "
                         "through the 8-block FIFO leaves the average and drops γ² by design: INFO, not FAIL")
        for k, cd in self.coh.items():
            if k.startswith("direct") and np.isfinite(cd[m]).any():
                self.add(id=f"{p.name}.coherence.{k}", group="coherence", path=p.name, title=f"{k}: band coherence (min)",
                         value=float(np.nanmin(cd[m])), unit="", tol=None, status="INFO",
                         meaning=f"|Σ M·R*|² / (Σ|M|² Σ|R|²) within each 1/{self.ppo}-oct band of the whole recording")

    # ------------------------------------------------------------ speaker path
    def absolute_spl(self, p: PathData):
        cal = self.run.cal or {}
        S_db = _f(cal.get("sensitivity_db"))
        curve = cal.get("curve_points")
        if S_db is None:
            self.notes.append(f"{p.name}: no calibration excerpt: absolute SPL skipped")
            return
        t = p.truth or {}
        tl = _tol(self.tol["level"]["absolute_spl_db"])
        rows = []
        sw = p.sweeps[p.primary]
        info = sw.trace.sweep_info or {}
        rl = _f(info.get("reference_level"))
        curve_in_columns = model.mic_curve_in_columns(sw.trace.meta)
        for tone in t.get("tones", []):
            if "meas_level_dbfs" not in tone:
                continue
            fc = tone["f"]
            corr = float(dsp.mic_correction_db(curve, np.array([fc]))[0])
            sine = tone["meas_level_dbfs"] + S_db + corr - t["emit_dbfs"]  # dB SPL per 0 dBFS drive
            row = [f"{fc:g}", f"{sine:.2f}"]
            H = self.at(self.f, self.src[f"ac2 sweep {p.primary}"], np.array([fc]))[0]
            vals = {}
            if rl is not None and np.isfinite(H):
                vals["ac2 sweep"] = float(dsp.db(H)) + rl + S_db + (0.0 if curve_in_columns else corr)
            if "ac2 TF" in self.src and rl is not None:
                Ht = self.at(self.f, self.src["ac2 TF"], np.array([fc]))[0]
                tf_curve = model.mic_curve_in_columns(p.tf.meta)
                cg = float(np.interp(np.log(fc), np.log(self.f), self.coh["ac2 TF"])) if "ac2 TF" in self.coh else 1.0
                if np.isfinite(Ht) and cg >= 0.99:  # as in the sine table: below it the TF is noise-limited
                    vals["ac2 TF × ref level"] = float(dsp.db(Ht)) + rl + S_db + (0.0 if tf_curve else corr)
            if p.rew and p.rew.meas_fr_spl is not None:
                fr = p.rew.meas_fr_spl
                k = int(np.argmin(np.abs(fr.f - fc)))
                vals["REW (SPL, cal from ac2)"] = float(np.mean(fr.mag[max(k - 2, 0):k + 3])) - p.rew.level_dbfs
            # ac2's sources are read at their columns, REW's averaged around the sine's bin:
            # the columns carry the room's fine structure against the sine's single frequency
            ud = self.fine_u.get(p.name, {}).get(fc, 0.0)
            for name, v in vals.items():
                d = v - sine
                u = 0.0 if name.startswith("REW") else ud
                row.append(f"{v:.2f} ({d:+.2f})")
                self.add(id=f"{p.name}.spl.{name}.{fc:g}", group="absolute SPL", path=p.name,
                         title=f"{name} at {fc:g} Hz vs steady sine, dB SPL per 0 dBFS drive", value=d, unit="dB", tol=tl,
                         status=judge_u(d, tl, u),
                         meaning=f"sine: in-1 level + sensitivity {S_db:.2f} dB + mic-curve correction {corr:+.2f} dB "
                                 f"− drive; ac2: meas÷ref + reference level + sensitivity"
                                 + ("" if curve_in_columns else " + the same curve correction (not in ac2's columns)")
                                 + (f"; the room's fine structure: a 1/{self.ppo}-oct column and the sine's frequency "
                                    f"differ by {u:.2f} dB in the direct estimate" if u else ""),
                         detail={"uncertainty": u})
            rows.append(row)
        self.table(f"{p.name}: absolute response, dB SPL at the mic per 0 dBFS drive", ["f Hz", "sine",
                   "ac2 sweep", "ac2 TF × ref", "REW"], rows,
                   f"sensitivity {S_db:.2f} dB SPL at 0 dBFS, curve {cal.get('curve_label', 'none')} "
                   f"({'in ac2 columns' if curve_in_columns else 'applied here to ac2'}); REW's cal derived from ac2's store")

    def room(self, p: PathData):
        sw = p.sweeps[p.primary]
        rm = sw.trace.room_metrics or {}
        bb = rm.get("broadband") or {}
        ours = {}
        if p.rew and p.rew.meas_ir is not None:
            ir = p.rew.meas_ir
            on = int(np.argmax(np.abs(ir.h)))
            ours = dsp.room_parameters(ir.h, ir.fs, max(on - int(0.001 * ir.fs), 0))
        rew = _rew_rt60_broadband(p.rew.rt60) if p.rew and p.rew.rt60 else {}
        rows = []
        for key, tk, rel in (("edt", "edt_rel", True), ("t20", "t20_rel", True), ("t30", "t30_rel", True),
                             ("c50", "c50_db", False), ("c80", "c80_db", False), ("d50", "d50", False)):
            a = bb.get(key)
            av = _f(a.get("value")) if isinstance(a, dict) and a.get("type") == "value" else None
            refused = a.get("reason", {}).get("type") if isinstance(a, dict) and a.get("type") == "refused" else None
            tol = _tol(self.tol["room"][tk])
            rows.append([key, "refused: " + refused if refused else _fmt(av, 3), _fmt(rew.get(key), 3), _fmt(ours.get(key), 3)])
            for other, ov in (("REW", rew.get(key)), ("numpy on REW IR", ours.get(key))):
                if av is None or ov is None:
                    continue
                d = (av / ov - 1) if rel else av - ov
                st, why = judge(d, tol), ""
                trunc, d50 = _f(bb.get("truncation")), (bb.get("d50") or {}).get("value")
                boundary = {"c50": 0.05, "c80": 0.08, "d50": 0.05}.get(key)
                if boundary and trunc is not None and trunc - _f(bb.get("onset") or 0.0) < boundary:
                    # the decay meets the noise before the clarity boundary: the late energy is
                    # the noise-floor compensation, not a measurement
                    st, why = "INCONCLUSIVE", (f"; ac2's decay meets the noise at {trunc*1e3:.1f} ms, before the "
                                               f"{boundary*1e3:g} ms boundary (decay range "
                                               f"{_f(bb.get('decay_range')) or float('nan'):.1f} dB)")
                elif key == "edt" and d50 is not None and d50 > 0.98:
                    # the first 10 dB of the Schroeder decay fall within the direct sound itself:
                    # their slope is the impulse's shape and band edge, not the room's decay
                    st, why = "INCONCLUSIVE", (f"; D50 {d50:.3f}: the direct sound dominates, so the first 10 dB "
                                               "of decay are the impulse itself")
                self.add(id=f"{p.name}.room.{key}.{other}", group="room", path=p.name, title=f"{key.upper()} ac2 vs {other}",
                         value=float(d), unit="rel" if rel else ("dB" if key.startswith("c") else ""), tol=tol,
                         status=st, meaning=f"broadband; ac2 {av:.3f}, {other} {ov:.3f}" + why)
        self.table(f"{p.name}: room parameters, broadband", ["metric", "ac2", "REW", "numpy (REW IR)"], rows,
                   "numpy: Schroeder integral after noise subtraction, simple truncation; approximate band filters")


def _settle_lag_s(s, fc):
    """Seconds from a sweep's full level to its pass through `fc`: ac2 fades in below the asked
    start, so full level is at the asked start and the rate is the exponential sweep's L."""
    if not (s.start_hz and s.end_hz and s.duration_s) or fc < s.start_hz:
        return None
    return s.duration_s / np.log(s.end_hz / s.start_hz) * np.log(fc / s.start_hz)


def _pre_note(coef: dict, fs: float, f: float) -> str | None:
    """The DUT pre-filter's gain at f where it matters (> 0.1 dB): there a sweep reads the
    truth only in the instantaneous-frequency approximation (Wiener model)."""
    g = 20 * np.log10(abs(dut.response(coef.get("pre", []), [f], fs)[0]))
    if abs(g) <= 0.1:
        return None
    return f"pre-filter {g:+.1f} dB here: a sweep sees it only approximately (Wiener model)"


def _nearest_finite(f, v, fc, max_oct=1 / 24):
    ok = np.where(np.isfinite(v))[0]
    if not len(ok):
        return None
    i = ok[int(np.argmin(np.abs(np.log(f[ok] / fc))))]
    return int(i) if abs(np.log2(f[i] / fc)) <= max_oct else None


def direct_ir_peak(raw) -> float:
    """Arrival of meas re ref in one recording: peak of IFFT(M·R*/(|R|²+ε)), interpolated."""
    M, R = np.fft.rfft(raw.meas), np.fft.rfft(raw.ref)
    P = np.abs(R) ** 2
    h = np.fft.irfft(M * np.conj(R) / (P + 1e-10 * P.max()), len(raw.meas))
    pos, _ = dsp.fractional_peak(h)
    if pos > len(h) / 2:
        pos -= len(h)
    return pos / raw.fs


def _rew_rt60_broadband(j) -> dict:
    """REW's rt60 map: 'Unfiltered result is mapped to zero frequency'. Keys vary by version;
    take the entry at 0 and match names loosely."""
    if not isinstance(j, dict):
        return {}
    e = None
    for k, v in j.items():
        try:
            if float(k) == 0:
                e = v
        except ValueError:
            continue
    if e is None:
        e = j.get("0") or j.get("0.0")
    if not isinstance(e, dict):
        return {}
    out = {}
    for k, v in e.items():
        kl = k.lower()
        val = v.get("value") if isinstance(v, dict) else v
        for key in ("edt", "t20", "t30", "c50", "c80", "d50"):
            if kl.startswith(key):
                out[key] = _f(val)
    return out


def _fmt(x, nd=1):
    x = _f(x)
    return "—" if x is None else f"{x:.{nd}f}"


def _fmt_lvl(x):
    x = _f(x)
    return "—" if x is None else f"{x:.1f}"


def _fmt_h(c):
    if c["kind"] == "value":
        return f"{c['value']:.1f}"
    if c["kind"] == "bound":
        return f"< {c['bound']:.1f}"
    return "—"


def analyse(root, tolerances: dict | None = None) -> tuple[dict, Analysis]:
    from .model import load
    run = load(root)
    a = Analysis(run, tolerances)
    return a.run_all(), a
