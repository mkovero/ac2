"""Offline, re-runnable analysis of a run directory (or the 2026-10-07 fixtures).

Every comparison becomes a Check with a value, its tolerance and a sentence saying what the
number means. Sources:

- ac2 sweep (each variant), ac2 live transfer (TF) — ac2's own exports;
- REW offline import: with REW's soundcard cal (the run's `[rew].soundcard_cal`) the meas
  import alone, as a REW user measures; without it meas and ref imported separately and
  meas/ref formed here. REW ÷ ref, REW's own Arithmetic A / B of those two imports (FR, IR,
  GD; no harmonics); REW live
  (fixtures only: needs REW Pro to start over the API);
- direct: numpy cross-spectrum Σ M·R*/Σ|R|² of a raw recording (REW's stage, or ac2's own
  capture of its sweep) in the same bands, ac2's column width (`--resolution`, 1/48 octave by default);
- steady sines: least-squares phasors, Blackman harmonics, group delay from ±1/48-oct pairs.
"""
from __future__ import annotations

import tomllib
from dataclasses import asdict

import numpy as np

from . import dsp, model
from .checks_bands import BandChecks
from .checks_common import (
    CLARITY_D_MAX,
    CLARITY_GUARD_S,
    MAINS_GUARD_HZ,
    TOLERANCES,
    Check,
    _f,
    _fmt,
    _pre_note,
    _rew_rt60_broadband,
    _settle_lag_s,
    _tol,
    _twin,
    direct_ir_peak,
    judge,
    judge_u,
    shared_pass,
)
from .checks_delay import DelayChecks
from .checks_dut import DutChecks
from .checks_harmonics import HarmonicChecks
from .checks_levels import LevelChecks
from .model import PathData, RunData

# Other modules and the tests import these from here.
__all__ = [
    "MAINS_GUARD_HZ", "TOLERANCES", "Analysis", "Check", "_pre_note", "_settle_lag_s", "_tol", "analyse",
    "direct_ir_peak", "judge", "judge_u", "shared_pass",
]


class Analysis(LevelChecks, BandChecks, DelayChecks, HarmonicChecks, DutChecks):
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
        shared_pass([vars(c) for c in self.checks])
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
            # ac2's sources are read at their columns, REW's averaged around the sine's bin. An
            # ac2 row is its magnitude row against the sine plus the reference level and the
            # sensitivity, which are the same numbers for every source: it carries that row's
            # uncertainty (fine structure, its take's path, its column noise), no more, no less
            twin = {"ac2 sweep": f"ac2 sweep {p.primary}", "ac2 TF × ref level": "ac2 TF"}
            for name, v in vals.items():
                d = v - sine
                u = self.sine_u.get(p.name, {}).get((twin[name], fc), 0.0) if name in twin else 0.0
                row.append(f"{v:.2f} ({d:+.2f})")
                self.add(id=f"{p.name}.spl.{name}.{fc:g}", group="absolute SPL", path=p.name,
                         title=f"{name} at {fc:g} Hz vs steady sine, dB SPL per 0 dBFS drive", value=d, unit="dB", tol=tl,
                         status=judge_u(d, tl, u),
                         meaning=f"sine: in-1 level + sensitivity {S_db:.2f} dB + mic-curve correction {corr:+.2f} dB "
                                 f"− drive; ac2: meas÷ref + reference level + sensitivity"
                                 + ("" if curve_in_columns else " + the same curve correction (not in ac2's columns)")
                                 + (f"; ±{u:.2f} dB, the uncertainty of its magnitude row against the sine (the room's "
                                    "fine structure, its take's own path, its column noise)" if u else ""),
                         detail={"uncertainty": u, **_twin(p.name, "spl", fc, name,
                                                         _tol(self.tol["band"]["mag_acoustic"])[0])})
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
                guard = CLARITY_GUARD_S if key in ("c50", "c80") else 0.0
                if boundary and trunc is not None and trunc - _f(bb.get("onset") or 0.0) < boundary + guard:
                    # the decay meets the noise before the clarity boundary, or so soon after it
                    # that the late energy is mostly the noise-floor compensation: not a measurement.
                    # D50 holds that energy only as a share of the total, too small to move it.
                    st, why = "INCONCLUSIVE", (f"; ac2's decay meets the noise at {trunc*1e3:.1f} ms, "
                                               + (f"within {guard*1e3:g} ms of" if guard else "before")
                                               + f" the {boundary*1e3:g} ms boundary (decay range "
                                               f"{_f(bb.get('decay_range')) or float('nan'):.1f} dB)")
                elif key in ("c50", "c80") and d50 is not None and d50 > CLARITY_D_MAX:
                    # C = 10·log10(D/(1 − D)), so dC/dD = 10/ln 10 / (D·(1 − D)): at D = 0.999 a
                    # 0.0005 difference in D, a trace of late energy, moves C by about 2 dB
                    st, why = "INCONCLUSIVE", (f"; D50 {d50:.4f}: so little late energy that C moves "
                                               f"{10/np.log(10)/(d50*(1-d50)):.0f} dB per unit of D")
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


def analyse(root, tolerances: dict | None = None) -> tuple[dict, Analysis]:
    from .model import load
    run = load(root)
    a = Analysis(run, tolerances)
    return a.run_all(), a
