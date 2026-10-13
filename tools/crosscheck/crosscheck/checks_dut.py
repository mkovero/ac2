"""Check families: the digital DUT against its analytic truth, and the LF H2 onset."""
from __future__ import annotations

import numpy as np

from . import dsp, dut, dutcov, levels
from .checks_common import (
    _fmt,
    _fmt_h,
    _nearest_finite,
    _pre_note,
    _settle_lag_s,
    _tol,
    judge,
)
from .model import PathData


def _sweep_truth(s, coef: dict, fs: float):
    """The truth an ac2 sweep's harmonic reading is judged against (dut.sweep_analytic): the
    sweep's own rate and harmonic window from its export; the steady sine's where the export
    carries no rate."""
    info = s.trace.sweep_info or {}
    rate = info.get("rate")
    if not rate:
        return lambda fc, amp: dut.analytic(fc, amp, coef, fs)
    pre, post = info.get("window_pre"), info.get("window_post")
    window = pre + post if pre is not None and post is not None else None
    own = ({k: sum(dsp.lf_window(float(rate), k)) for k in range(2, 6)}
           if window is not None and s.lf_harmonics == "fine" else None)
    return lambda fc, amp: dut.sweep_analytic(fc, amp, coef, fs, float(rate), window, own)


class DutChecks:
    """Digital DUT truth and LF H2 onset; mixed into Analysis."""

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
                               lo, hi, 1 / 24, hi, _sweep_truth(s, coef, fs)))
        if p.rew is not None and p.rew.meas_dist is not None and np.isfinite(p.rew.level_dbfs):
            r = p.rew.meas_dist
            nf = r.cols.get("Noise", np.full(len(r.f), np.nan))
            curves.append((p.rew.label, p.rew.level_dbfs, r.f,
                           {k: (r.cols[f"H{k}"], nf) for k in range(2, 6) if f"H{k}" in r.cols},
                           10.0 * 2 ** (1 / 12), 40000.0 * 2 ** (-1 / 12), 1 / 6, 40000.0 * 2 ** (-1 / 12),
                           lambda fc, amp: dut.analytic(fc, amp, coef, fs)))
        for name, lvl, f, hs, lo, hi, near, top, truth_at in curves:
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
                    tr = truth_at(float(f[i]), amp).get(k)
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
                    tr = truth_at(float(f[i]), amp).get(k)
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
                   "Truth: dut.py's analytic Hk at each source's own level and at the column's frequency; for "
                   "ac2 sweeps the sweep's own (dut.sweep_analytic: the pre-filter's first-order chirp term at "
                   "the sweep's rate, ac2's band-mean reading over its harmonic window). "
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
        pts = sorted((x["f"], x["h_dbr"]["2"]) for x in tones)
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
                                         f"log f ({truth:.1f} dBr): positive = ac2 overstates; seen on 10 Hz / 5.5 s sweeps",
                                 detail={"twin": {"key": f"{p.name}|lf_h2|22", "analyser": "ac2", "truth": truth,
                                                  "agree": _tol(self.tol["harmonics"]["ac2_vs_rew_db"])[0]}})
                    else:
                        self.add(id=f"{p.name}.lf_h2.{name}", group="LF H2", path=p.name,
                                 title=f"ac2 sweep {name}: H2 excess at 22 Hz", value=None, unit="dB", tol=tl,
                                 status="INCONCLUSIVE", meaning=f"ac2's H2 at 22 Hz is a bound (shortfall {c.get('shortfall', 0):.1f} dB)")
            row.append(f"{onset:g}" if onset else "none")
            rows.append(row)
        # REW's distortion export at the same frequency, against the meas input's own sine H2:
        # REW's export is the meas channel alone, not divided by the reference
        meas_pts = sorted((x["f"], x["h_dbr"]["2"]) for x in tones)
        mf_, mv = np.array([a for a, _ in meas_pts]), np.array([b for _, b in meas_pts])
        for rs in (p.rew, p.rew_live):
            if rs is None or rs.meas_dist is None or "H2" not in rs.meas_dist.cols:
                continue
            d, fc = rs.meas_dist, 22.0
            i = _nearest_finite(d.f, d.cols["H2"], fc)
            if i is None or fc < mf_[0] or fc > mf_[-1]:
                continue
            truth = float(np.interp(np.log(fc), np.log(mf_), mv))
            hv = float(d.cols["H2"][i])
            hf = float(d.cols.get("Noise", np.full(len(d.f), np.nan))[i])
            c = dsp.classify(hv, hf, margin)
            base = dict(id=f"{p.name}.lf_h2.{rs.label}", group="LF H2", path=p.name,
                        title=f"{rs.label}: H2 excess at 22 Hz", unit="dB", tol=tl)
            if c["kind"] == "value":
                ex = hv - truth
                self.add(**base, value=ex, status=judge(max(ex, 0.0), tl),
                         meaning=f"REW H2 {hv:.1f} dBr at {d.f[i]:.1f} Hz (noise {_fmt(c.get('floor'))}) against the "
                                 f"meas input's sine H2 interpolated in log f ({truth:.1f} dBr): positive = REW "
                                 "overstates",
                         detail={"twin": {"key": f"{p.name}|lf_h2|22", "analyser": "REW", "truth": truth,
                                          "agree": _tol(self.tol["harmonics"]["ac2_vs_rew_db"])[0]}})
            else:
                self.add(**base, value=None, status="INCONCLUSIVE",
                         meaning=(f"REW's H2 at 22 Hz is a bound < {c['bound']:.1f} dBr (shortfall {c['shortfall']:.1f} dB)"
                                  if c["kind"] == "bound" else "REW has no H2 reading at 22 Hz")
                                 + f"; the meas input's sine H2 there {truth:.1f} dBr")
        self.table(f"{p.name}: LF H2 excess over the sine truth (dB) by sweep variant", ["sweep", "start / duration"]
                   + [f"{x:g}" for x in freqs] + ["overstates up to (Hz)"], rows,
                   f"truth points: {', '.join(f'{a:g} Hz {b:.0f}' for a, b in pts)} dBr (linear in log f between). "
                   + (t.get("level_note") or ""))
