"""Check families: delays, ETC and group delay."""
from __future__ import annotations

import numpy as np

from . import dsp
from .checks_common import (
    MAINS_GUARD_HZ,
    _f,
    _tol,
    _twin,
    direct_ir_peak,
    judge,
    judge_u,
)
from .model import PathData


class DelayChecks:
    """Delays, ETC and group delay; mixed into Analysis."""

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
            # A loudspeaker's IR peak moves with the stimulus's top octave and end taper (the
            # peak is where the high band's energy lines up), so each sweep's arrival is judged
            # against the IR peak of its own capture: the same samples ac2 analysed. The REW
            # recording's peak stays the reference only where no capture was kept.
            own_key = f"direct (ac2 capture {name})"
            own_tau = self.direct_delay.get(own_key) if s.raw is not None else None
            ref_tau, ref_slope, ref_what = truth_tau, slope_tau, "REW's recording" if p.rec is not None else "the documented delay"
            if own_tau is not None:
                ref_tau, ref_what = own_tau, "its own capture"
                Hc = S.get(own_key)
                if Hc is not None and np.isfinite(Hc).any():
                    mc = np.isfinite(Hc)
                    ref_slope = dsp.delay_from_phase(f[mc], Hc[mc], lo, hi)
                rows.append([f"{own_key}: IR peak / phase slope",
                             f"{own_tau*1e6:.3f} / " + ("—" if ref_slope is None else f"{ref_slope*1e6:.3f}"),
                             f"the capture ac2 analysed; phase slope {lo}–{hi} Hz"])
                if truth_tau is not None and p.rec is not None:
                    dc = (own_tau - truth_tau) * 1e6
                    self.add(id=f"{p.name}.delay.capture_vs_rew_recording.{name}", group="delay", path=p.name,
                             title=f"IR peak: ac2 capture {name} vs REW's recording", value=dc, unit="µs", tol=None,
                             status="INFO",
                             meaning="the same IR-peak estimator on two recordings of the path with different "
                                     "stimuli: the peak follows the sweep's top octave and end taper (a loudspeaker's "
                                     "most), so the two differ by the stimulus, not by either analyser; context, not judged")
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
            if ref_tau is not None:
                d = (arr - ref_tau) * 1e6
                self.add(id=f"{p.name}.delay.ac2_arrival.{name}", group="delay", path=p.name,
                         title=f"ac2 sweep {name}: reported arrival vs direct", value=d, unit="µs", tol=tl,
                         status=judge(d, tl),
                         meaning=f"the arrival ac2 reports (sweep_info.arrival) against the direct IR peak of "
                                 f"{ref_what} ({ref_tau*1e6:.3f} µs); whole-sample builds report 0 for the 3.7 µs "
                                 "Xone path (the delay then stays inside the phase, see the next check)")
                if elec and ref_slope is not None:
                    d2 = (total - ref_slope) * 1e6
                    self.add(id=f"{p.name}.delay.ac2_total.{name}", group="delay", path=p.name,
                             title=f"ac2 sweep {name}: arrival + delay in phase vs direct", value=d2, unit="µs", tol=tl,
                             status=judge(d2, tl),
                             meaning="the delay ac2's response carries in total (reported arrival + phase slope "
                                     f"{lo}–{hi} Hz) against the phase slope of the direct response of {ref_what} "
                                     "over the same band")
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
                what = "meas (soundcard cal)" if rs.meas_alone else "meas÷ref"
                rows.append([f"{rs.label}: {what} phase slope", f"{tau*1e6:.3f}",
                             "offline import: each channel re its own timing marker, so the in5−in2 delay is gone"])
                self.add(id=f"{p.name}.delay.rew_offline_phase", group="delay", path=p.name,
                         title=f"{rs.label}: delay left in {what} phase", value=tau * 1e6, unit="µs", tol=None, status="INFO",
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
            div = offline and not rs.meas_alone
            Hr = rs.meas_fr.H / rs.ref_fr.H if div else rs.meas_fr.H
            Hr = Hr * np.exp(-2j * np.pi * rs.meas_fr.f * (tau if offline else 0.0))
            keep = ~dsp.mains_columns(rs.meas_fr.f, self.mains_hz, 1 / 48, guard_hz=MAINS_GUARD_HZ)
            ests[f"{rs.label} ±1/12-oct fit"], sig[f"{rs.label} ±1/12-oct fit"], _ = dsp.gd_slope_err(
                rs.meas_fr.f[keep], np.angle(Hr[keep]), fc, 1 / 12)
            if rs.meas_gd is not None:
                # REW's export is the group delay of one channel re the stimulus: for an offline
                # import meas ÷ ref is the difference of the two exports, plus the delay put back
                # (a soundcard-corrected meas import alone: its own export plus that delay)
                gm = rs.meas_gd
                gr = rs.ref_gd if div else None
                v = []
                for c in fc:
                    sel = (gm.f >= c * 2 ** (-1 / 12)) & (gm.f <= c * 2 ** (1 / 12))
                    if not sel.any():
                        v.append(np.nan)
                        continue
                    x = np.nanmean(gm.mag[sel]) + (tau if offline and not div else 0.0)
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
                         detail={"uncertainty_2sigma": float(u), **({} if own else _twin(p.name, "gd", c, name, tol[0]))})
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
