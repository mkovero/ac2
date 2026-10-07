"""Offline, re-runnable analysis of a run directory (or the 2026-10-07 fixtures).

Every comparison becomes a Check with a value, its tolerance and a sentence saying what the
number means. Sources:

- ac2 sweep (each variant), ac2 live transfer (TF) — ac2's own exports;
- REW offline import (meas and ref imported separately, meas/ref formed here), REW live
  (fixtures only: needs REW Pro to start over the API);
- direct: numpy cross-spectrum Σ M·R*/Σ|R|² of a raw recording (REW's stage, or ac2's own
  capture of its sweep) in the same 1/48-octave bands;
- steady sines: least-squares phasors, Blackman harmonics, group delay from ±1/48-oct pairs.
"""
from __future__ import annotations

import math
import tomllib
from dataclasses import asdict, dataclass, field
from pathlib import Path

import numpy as np

from . import dsp
from .model import PathData, RunData, Sweep

TOLERANCES = Path(__file__).resolve().parent.parent / "tolerances.toml"


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
        self.checks: list[Check] = []
        self.tables: dict[str, dict] = {}
        self.series: dict[str, dict] = {}  # for plots
        self.notes: list[str] = []

    # ------------------------------------------------------------ helpers
    def add(self, **kw) -> Check:
        c = Check(**kw)
        self.checks.append(c)
        return c

    def table(self, name: str, columns: list[str], rows: list[list], note: str = ""):
        self.tables[name] = {"columns": columns, "rows": rows, "note": note}

    @staticmethod
    def at(f_src, H_src, f_tgt):
        """Complex response interpolated onto f_tgt: dB and unwrapped phase, linear in log f."""
        f_src, H_src = np.asarray(f_src), np.asarray(H_src)
        ok = np.isfinite(H_src) & (f_src > 0)
        fs_, Hs = f_src[ok], H_src[ok]
        if len(fs_) < 2:
            return np.full(len(f_tgt), np.nan + 0j)
        lf = np.log(fs_)
        m = np.interp(np.log(f_tgt), lf, dsp.db(Hs), left=np.nan, right=np.nan)
        p = np.interp(np.log(f_tgt), lf, np.unwrap(np.angle(Hs)), left=np.nan, right=np.nan)
        return 10 ** (m / 20) * np.exp(1j * p)

    # ------------------------------------------------------------ entry
    def run_all(self) -> dict:
        for p in self.run.paths.values():
            self.path(p)
        if self.run.ambient:
            from . import ambient
            ambient.analyse(self, self.run)
        return self.results()

    def results(self) -> dict:
        counts = {}
        for c in self.checks:
            counts[c.status] = counts.get(c.status, 0) + 1
        return {"source": str(self.run.root), "fixture": self.run.fixture, "manifest": self.run.manifest,
                "summary": counts, "checks": [asdict(c) for c in self.checks], "tables": self.tables,
                "notes": self.notes}

    def path(self, p: PathData):
        self.notes.extend(f"{p.name}: {n}" for n in p.notes)
        grid = self.sources(p)
        if grid is None:
            self.notes.append(f"{p.name}: no ac2 sweep to set the comparison grid; path skipped")
            return
        self.bands(p)
        self.level_convention(p)
        self.delays(p)
        self.etc(p)
        self.group_delay(p)
        self.harmonics(p)
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
        src: dict[str, np.ndarray] = {f"ac2 sweep {p.primary}": H}
        for name, s in p.sweeps.items():
            if name != p.primary:
                fs_, Hs, _ = s.trace.complex_response()
                src[f"ac2 sweep {name}"] = self.at(fs_, Hs, f)
        self.coh = {}
        self.coh_by_path = getattr(self, "coh_by_path", {})
        self.coh_by_path[p.name] = self.coh
        if p.tf is not None:
            b = p.tf.freq
            Ht = 10 ** (b["mag_db"] / 20) * np.exp(1j * np.deg2rad(b["phase_deg"]))
            src["ac2 TF"] = self.at(b["freq_hz"], Ht, f)
            if "coherence" in b:
                ok = np.isfinite(b["coherence"])
                self.coh["ac2 TF"] = np.interp(np.log(f), np.log(b["freq_hz"][ok]), b["coherence"][ok],
                                               left=np.nan, right=np.nan)
        if p.rew and p.rew.meas_fr is not None:
            if p.rew.ref_fr is not None:
                Hr = p.rew.meas_fr.H / p.rew.ref_fr.H
                src["REW offline"] = dsp.band_mean(p.rew.meas_fr.f, Hr, f)
            else:
                src["REW offline"] = dsp.band_mean(p.rew.meas_fr.f, p.rew.meas_fr.H, f)
        if p.rew_live and p.rew_live.meas_fr is not None:
            src["REW live"] = dsp.band_mean(p.rew_live.meas_fr.f, p.rew_live.meas_fr.H, f)
        if p.rec is not None:
            Hd, cd = dsp.cross_spectrum_bands(p.rec.meas, p.rec.ref, p.rec.fs, f)
            src["direct (REW recording)"] = Hd
            self.coh["direct (REW recording)"] = cd
        for name, s in p.sweeps.items():
            if s.raw is not None:
                Hd, cd = dsp.cross_spectrum_bands(s.raw.meas, s.raw.ref, s.raw.fs, f)
                src[f"direct (ac2 capture {name})"] = Hd
        if p.tf_raw is not None:
            Hd, cd = dsp.cross_spectrum_bands(p.tf_raw.meas, p.tf_raw.ref, p.tf_raw.fs, f)
            src["direct (TF capture)"] = Hd
            self.coh["direct (TF capture)"] = cd
        self.src = src
        self.series[p.name] = {"f": f, "src": src}
        return f

    # ------------------------------------------------------------ magnitude and phase
    def bands(self, p: PathData):
        f, S = self.f, self.src
        primary = f"ac2 sweep {p.primary}"
        elec = p.kind == "electrical"
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
        rows = []
        for a, b, offset_free, meaning in pairs:
            Ha, Hb = S[a], S[b]
            m_all = np.isfinite(Ha) & np.isfinite(Hb)
            if a == "ac2 TF" and "ac2 TF" in self.coh:
                m_all &= self.coh["ac2 TF"] >= 0.99
            # phase after removing a pure delay difference (REW offline drops the in5-in2 delay;
            # ac2's sweep keeps it in the phase): the delay itself is judged in the delay checks
            rm = (f >= 1000) & (f <= 20000) & m_all if elec else (f >= 200) & (f <= 5000) & m_all
            tau = dsp.delay_from_phase(f[rm], (Ha / Hb)[rm], 0, np.inf) if rm.sum() > 10 else 0.0
            ratio = Ha / Hb * np.exp(2j * np.pi * f * tau)
            for lo, hi, key in bands:
                m = m_all & (f >= lo) & (f < hi)
                if m.sum() < 3:
                    continue
                dm = dsp.db(ratio[m])
                dp = np.rad2deg(np.angle(ratio[m]))
                mean, spread = float(np.mean(dm)), float(np.max(np.abs(dm - np.mean(dm))))
                pmean, pspread = float(np.mean(dp)), float(np.max(np.abs(dp - np.mean(dp))))
                tk = "mag_tf" if a == "ac2 TF" else (key if elec else "mag_acoustic")
                tm = _tol(self.tol["band"][tk])
                tp = _tol(self.tol["band"]["phase" if elec else "phase_acoustic"])
                st_m = judge(spread, tm) if offset_free else worst(judge(spread, tm), judge(mean, tm))
                st_p = worst(judge(pspread, tp), judge(pmean, tp))
                self.add(id=f"{p.name}.mag.{a}|{b}.{lo}-{hi}", group="magnitude", path=p.name,
                         title=f"{a} − {b}, {lo}–{hi} Hz, magnitude", value=spread if offset_free else max(abs(mean), spread),
                         unit="dB", tol=tm, status=st_m,
                         meaning=meaning + f". Mean {mean:+.3f} dB, max deviation from the mean ±{spread:.3f} dB "
                                 f"over {m.sum()} columns.",
                         detail={"mean": mean, "spread": spread, "n": int(m.sum())})
                self.add(id=f"{p.name}.phase.{a}|{b}.{lo}-{hi}", group="phase", path=p.name,
                         title=f"{a} − {b}, {lo}–{hi} Hz, phase", value=max(abs(pmean), pspread), unit="°", tol=tp,
                         status=st_p,
                         meaning=f"phase difference after removing a pure delay difference of {tau*1e6:+.2f} µs "
                                 f"(fitted 1–20 kHz); mean {pmean:+.3f}°, spread ±{pspread:.3f}°",
                         detail={"mean": pmean, "spread": pspread, "delay_removed_s": tau})
                rows.append([f"{a} − {b}", f"{lo}–{hi}", f"{mean:+.3f} ± {spread:.3f}", f"{pmean:+.2f} ± {pspread:.2f}",
                             f"{tau*1e6:+.2f}", int(m.sum())])
        self.table(f"{p.name}: magnitude and phase per band", ["pair", "band Hz", "Δ dB mean ± spread",
                   "Δ° mean ± spread", "delay removed µs", "n"], rows,
                   "1/48-octave columns of ac2's primary sweep; the TF only where γ² ≥ 0.99")
        # against the steady sines
        t = p.truth
        if t:
            rows = []
            for tone in t.get("tones", []):
                fc = tone["f"]
                if "ratio_deg" not in tone and "ratio_db" not in tone:
                    continue
                for name in [primary, "REW live", "REW offline", "direct (REW recording)", "ac2 TF"]:
                    if name not in S:
                        continue
                    h = self.at(f, S[name], np.array([fc]))[0]
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
                        note = " (REW offline refers each channel to its own timing marker: the path's delay is not in its phase)" if name == "REW offline" else ""
                        self.add(id=f"{p.name}.sine_phase.{name}.{fc:g}", group="phase vs sine", path=p.name,
                                 title=f"{name} phase at {fc:g} Hz vs steady sine", value=dpd, unit="°", tol=tp,
                                 status=judge(dpd, tp) if name != "REW offline" or fc < 1000 else "INFO",
                                 meaning=f"sine {tone['ratio_deg']:+.3f}°, {name} {np.rad2deg(np.angle(h)):+.3f}°{note}")
                    if dmd is not None:
                        tm = _tol(self.tol["band"]["mag_lf" if elec else "mag_acoustic"])
                        self.add(id=f"{p.name}.sine_mag.{name}.{fc:g}", group="magnitude vs sine", path=p.name,
                                 title=f"{name} magnitude at {fc:g} Hz vs steady sine", value=dmd, unit="dB", tol=tm,
                                 status=judge(dmd, tm) if not name.startswith("REW live") else "INFO",
                                 meaning=f"sine {tone['ratio_db']:+.3f} dB (meas ÷ ref), {name} {dsp.db(h):+.3f} dB")
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
        for label, g in G.items():
            if "ref" in g:
                d = a_db - (g["meas"] - g["ref"])
                tl = _tol(self.tol["level"]["offset_db"])
                self.add(id=f"{p.name}.level.ac2_vs_rew_meas_minus_ref", group="level", path=p.name,
                         title=f"ac2 meas÷ref vs {label} (meas − ref)", value=d, unit="dB", tol=tl, status=judge(d, tl),
                         meaning="ac2 states meas ÷ ref; REW states each channel re the stimulus; their difference "
                                 "must be ac2's number")
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
        elec = p.kind == "electrical"
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
            H = S.get(f"ac2 sweep {name}")
            in_phase = None
            if H is not None:
                m = np.isfinite(H)
                in_phase = dsp.delay_from_phase(f[m], H[m], lo, hi)
            if arr is None:
                continue
            total = arr + (in_phase or 0.0) if elec else arr
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
                        self.add(id=f"{p.name}.delay.rew.{rs.label}.{what}", group="delay", path=p.name,
                                 title=f"{rs.label}: {what} vs direct", value=d, unit="µs", tol=tl, status=judge(d, tl),
                                 meaning="REW's arrival against the direct cross-spectrum"
                                         + ("; an offline import refers each channel to its own timing marker, so "
                                            "only the meas − ref difference is meaningful" if rs.ref_ir is not None else ""))
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
        m = (rel >= -0.005) & (rel <= 0.05) & (etc_a > -60) & (etc_r > -60) & np.isfinite(etc_r)
        if m.sum() < 5:
            return
        d = etc_a[m] - etc_r[m]
        med = float(np.median(np.abs(d)))
        tl = _tol(self.tol["etc"]["median_db"])
        self.add(id=f"{p.name}.etc", group="ETC", path=p.name, title=f"ETC ac2 sweep vs {rs.label}", value=med,
                 unit="dB", tol=tl, status=judge(med, tl),
                 meaning=f"median |Δ| over {m.sum()} cells of {dt*1e3:.3f} ms from −5 to 50 ms where both are within "
                         f"60 dB of their peak; REW's IR band-limited to ac2's sweep top ({top:g} Hz) and taken as the "
                         "envelope maximum in each of ac2's cells",
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
        ests = {}
        H = S[f"ac2 sweep {p.primary}"]
        m = np.isfinite(H)
        gcen = dsp.gd_central(f[m], np.angle(H[m]))
        ests["ac2 as displayed (central difference)"] = np.interp(np.log(fc), np.log(f[m]), gcen)
        ests["ac2 ±1/12-oct fit"] = dsp.gd_slope(f[m], np.angle(H[m]), fc, 1 / 12)
        tau_add = 0.0
        for rs in (p.rew_live, p.rew):
            if rs is None or rs.meas_fr is None:
                continue
            Hr = rs.meas_fr.H / rs.ref_fr.H if rs.ref_fr is not None else rs.meas_fr.H
            if rs.ref_fr is not None and "direct (REW recording)" in S:
                # offline: put back the delay REW's own timing markers took out
                Hd = S["direct (REW recording)"]
                md = np.isfinite(Hd)
                tau_add = dsp.delay_from_phase(f[md], Hd[md], 1000, 20000) - dsp.delay_from_phase(
                    f[np.isfinite(S["REW offline"])], S["REW offline"][np.isfinite(S["REW offline"])], 1000, 20000)
            ests[f"{rs.label} ±1/12-oct fit"] = dsp.gd_slope(rs.meas_fr.f, np.angle(Hr), fc, 1 / 12) + (tau_add if rs.ref_fr is not None else 0)
            if rs.meas_gd is not None:
                g = rs.meas_gd
                ests[f"{rs.label} own GD export (1/48-oct mean)"] = np.array(
                    [np.nanmean(g.mag[(g.f >= c * 2 ** (-1 / 96)) & (g.f < c * 2 ** (1 / 96))]) if
                     ((g.f >= c * 2 ** (-1 / 96)) & (g.f < c * 2 ** (1 / 96))).any() else g.mag[np.argmin(abs(g.f - c))] for c in fc])
        if "direct (REW recording)" in S:
            # the direct cross-spectrum at 96 ppo, then the same fit
            fd = dsp.log_centres(max(8.0, fc.min() / 2), min(40000, fc.max() * 2), 96)
            Hd, _ = dsp.cross_spectrum_bands(p.rec.meas, p.rec.ref, p.rec.fs, fd, 1 / 96)
            ok = np.isfinite(Hd)
            ests["direct ±1/12-oct fit"] = dsp.gd_slope(fd[ok], np.angle(Hd[ok]), fc, 1 / 12)
        rows = []
        tf_ = _tol(self.tol["gd"]["fit_rel"])
        tc_ = _tol(self.tol["gd"]["central_rel"])
        ta_ = _tol(self.tol["gd"]["high_abs_us"])
        for i, c in enumerate(fc):
            row = [f"{c:g}", f"{truth[i]*1e6:.1f}"]
            for name, g in ests.items():
                v = g[i]
                row.append("—" if not np.isfinite(v) else f"{v*1e6:.1f} ({(v/truth[i]-1)*100:+.0f} %)")
                if c < 16 or not np.isfinite(v):
                    continue
                if c >= 1000:
                    d, tol, unit = (v - truth[i]) * 1e6, ta_, "µs"
                else:
                    d, tol, unit = v / truth[i] - 1, (tc_ if "central" in name else tf_), "rel"
                st = judge(d, tol)
                self.add(id=f"{p.name}.gd.{name}.{c:g}", group="group delay", path=p.name,
                         title=f"{name} at {c:g} Hz vs steady sine", value=float(d), unit=unit, tol=tol, status=st,
                         meaning=f"sine {truth[i]*1e6:.1f} µs (phase of meas÷ref at f·2^(±1/48)), {name} {v*1e6:.1f} µs"
                                 + ("; ac2's displayed derivative takes −Δφ/Δω between neighbouring 1/48-oct columns, "
                                    "so 0.03° of phase ripple is ~60 µs at 50 Hz" if "central" in name else ""))
            rows.append(row)
        self.series[p.name]["gd"] = {"fc": fc, "truth": truth, "ests": ests}
        self.table(f"{p.name}: group delay vs steady sine (µs)", ["f Hz", "sine"] + list(ests), rows,
                   (f"REW offline put back {tau_add*1e6:+.2f} µs (the direct delay its timing markers removed). " if tau_add else "")
                   + "Fit: least-squares slope of the unwrapped phase over f·2^(±1/12).")

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
                    self._cmp_h(p, name, fc, k, ca, ct, th)
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
        if ca["kind"] == "value" and ct["kind"] == "value":
            d = ca["value"] - ct["value"]
            self.add(id=f"{p.name}.h{k}.{name}.{fc:g}", group="harmonics", path=p.name,
                     title=f"{name} H{k} at {fc:g} Hz vs steady sine", value=d, unit="dB", tol=th, status=judge(d, th),
                     meaning=f"{name} {ca['value']:.1f} dBr (floor {_fmt(ca.get('floor'))}), sine {ct['value']:.1f} dBr")
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
        on = int(np.argmax(env > 0.1 * env.max()))
        n_sw = int((s.duration_s or info.get("duration", 5.5)) * fs) + int(1.0 * fs)
        if on < n_sw:
            self.notes.append(f"{p.name}: {name}: the raw capture has {on/fs:.1f} s before the sweep, not a whole "
                              "sweep length: floor cross-check skipped")
            return
        seg_ref = ref[on - int(0.1 * fs): on - int(0.1 * fs) + n_sw]
        seg_meas = meas[on - int(0.1 * fs): on - int(0.1 * fs) + n_sw]
        noise_meas = meas[on - n_sw - int(0.2 * fs): on - int(0.2 * fs)]
        h = dsp.deconvolve(seg_ref, seg_meas)
        hn = dsp.deconvolve(seg_ref, noise_meas)
        d = int(np.argmax(np.abs(h[: int(0.05 * fs)])))
        b = s.trace.freq
        fsel = b["freq_hz"][(b["freq_hz"] >= 20) & (b["freq_hz"] <= 10000)][::4]
        r = dsp.sweep_harmonics(h, d, fs, L, fsel, pre=info.get("window_pre", 0.0083229),
                                post=info.get("window_post", 0.0916771), noise_h=hn)
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
                     meaning="ac2's floor comes from one 100 ms noise window; this one from 8 windows of the "
                             "deconvolved noise before the sweep, power-averaged. Value: median difference; "
                             f"{int(np.nansum(bad))} columns differ by more than {lim:g} dB",
                     detail={"columns_flagged": int(np.nansum(bad))})
        self.table(f"{p.name}: {name}: floor columns ac2 vs raw capture differing > {lim:g} dB",
                   ["f Hz", "H", "ac2 floor", "raw-capture floor", "Δ"], rows)

    def _mains_flags(self, p: PathData):
        lines = []
        if p.noise is not None:
            lines = dsp.mains_lines(p.noise.meas, p.noise.fs, self.run.manifest.get("mains_hz", 50.0))
        if not lines:
            return
        hz = np.array([x["hz"] for x in lines])
        rows = []
        for name, s in p.sweeps.items():
            b = s.trace.freq
            for k in range(2, 6):
                kf = k * b["freq_hz"]
                lo, hi = kf * 2 ** (-1 / 48), kf * 2 ** (1 / 48)
                hit = [(i, hz[(hz >= lo[i]) & (hz <= hi[i])]) for i in range(len(kf))]
                for i, hh in hit:
                    if len(hh) and b["freq_hz"][i] <= 2000:
                        rows.append([name, f"{b['freq_hz'][i]:.1f}", f"H{k}", ", ".join(f"{x:g}" for x in hh)])
        self.table(f"{p.name}: sweep columns whose harmonic band holds a mains line", ["sweep", "f Hz", "H", "line Hz"],
                   rows[:200], f"mains lines found in the noise recording: {', '.join(f'{x:g}' for x in hz)} Hz")

    # ------------------------------------------------------------ LF H2 onset
    def lf_h2_onset(self, p: PathData):
        t = p.truth or {}
        pts = sorted((x["f"], x["h_dbr"]["2"]) for x in t.get("tones", []) if x.get("h_dbr") and "2" in x["h_dbr"])
        if len(pts) < 2:
            return
        tf_, tv = np.array([a for a, _ in pts]), np.array([b for _, b in pts])
        freqs = np.array([16, 20, 22, 25, 31.5, 40, 50, 63, 80, 100])
        tl = _tol(self.tol["harmonics"]["lf_onset_excess_db"])
        margin = float(self.tol["distortion"]["margin_db"])
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
                    if c["kind"] == "value":
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
                         meaning="|Σ M·R*|² / (Σ|M|² Σ|R|²) within each 1/48-oct band of the whole recording")

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
        curve_in_columns = str(sw.trace.meta.get("mic", "none")).strip().lower() not in ("none", "", "off")
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
                tf_curve = str(p.tf.meta.get("mic", "none")).strip().lower() not in ("none", "", "off")
                if np.isfinite(Ht):
                    vals["ac2 TF × ref level"] = float(dsp.db(Ht)) + rl + S_db + (0.0 if tf_curve else corr)
            if p.rew and p.rew.meas_fr_spl is not None:
                fr = p.rew.meas_fr_spl
                k = int(np.argmin(np.abs(fr.f - fc)))
                vals["REW (SPL, cal from ac2)"] = float(np.mean(fr.mag[max(k - 2, 0):k + 3])) - p.rew.level_dbfs
            for name, v in vals.items():
                d = v - sine
                row.append(f"{v:.2f} ({d:+.2f})")
                self.add(id=f"{p.name}.spl.{name}.{fc:g}", group="absolute SPL", path=p.name,
                         title=f"{name} at {fc:g} Hz vs steady sine, dB SPL per 0 dBFS drive", value=d, unit="dB", tol=tl,
                         status=judge(d, tl),
                         meaning=f"sine: in-1 level + sensitivity {S_db:.2f} dB + mic-curve correction {corr:+.2f} dB "
                                 f"− drive; ac2: meas÷ref + reference level + sensitivity"
                                 + ("" if curve_in_columns else " + the same curve correction (not in ac2's columns)"))
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
                self.add(id=f"{p.name}.room.{key}.{other}", group="room", path=p.name, title=f"{key.upper()} ac2 vs {other}",
                         value=float(d), unit="rel" if rel else ("dB" if key.startswith("c") else ""), tol=tol,
                         status=judge(d, tol), meaning=f"broadband; ac2 {av:.3f}, {other} {ov:.3f}")
        self.table(f"{p.name}: room parameters, broadband", ["metric", "ac2", "REW", "numpy (REW IR)"], rows,
                   "numpy: Schroeder integral after noise subtraction, simple truncation; approximate band filters")


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
