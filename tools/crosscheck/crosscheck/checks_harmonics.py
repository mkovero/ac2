"""Check family: harmonics against the steady sines, REW and the noise floor."""
from __future__ import annotations

import numpy as np

from . import dsp
from .checks_common import (
    MAINS_GUARD_HZ,
    _f,
    _fmt,
    _fmt_h,
    _fmt_lvl,
    _nearest_finite,
    _tol,
    judge,
)
from .model import PathData, Sweep

# A meas harmonic less than this far above the reference input's own is flagged in the table
# (ac2-scene's REFERENCE_BOUND_DB): 6 dB above, shared converters add at most 1 dB to it.
REFERENCE_BOUND_DB = 6.0


class HarmonicChecks:
    """Harmonics; mixed into Analysis."""

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
                rv = (tone.get("h_ref_dbr") or {}).get(str(k))
                if rv is not None:
                    rows.append([f"{fc:g}", f"H{k}", "steady sine, reference input", _fmt_lvl(tone.get("ref_level_dbfs")),
                                 f"{rv:.1f}", "—", "—", "info"])
                reads_ac2, reads_rew = {}, {}
                for name, s in sources.items():
                    b = s.trace.freq
                    i = _nearest_finite(b["freq_hz"], b[f"h{k}_db"], fc)
                    reads_ac2[name] = None
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
                    reads_ac2[name] = ca
                    self._cmp_h(p, name, fc, k, ca, ct, th, "ac2")
                    # ac2's reading of the reference input's own order: where the loopback
                    # shares the meas path's converters, a meas harmonic within a few dB of it
                    # may be theirs
                    if f"ref_h{k}_db" in b:
                        cref = dsp.classify(b[f"ref_h{k}_db"][i], b[f"ref_h{k}_floor_db"][i], margin)
                        near = (ca["kind"] == "value" and cref["kind"] == "value"
                                and ca["value"] < cref["value"] + REFERENCE_BOUND_DB)
                        rows.append([f"{b['freq_hz'][i]:.1f}", f"H{k}", f"{name}, reference input", "—", _fmt_h(cref),
                                     _fmt(cref.get("floor")), _fmt(cref.get("margin")),
                                     f"{cref['kind']}, meas within {REFERENCE_BOUND_DB:g} dB" if near else cref["kind"]])
                for rs in (p.rew, p.rew_live):
                    if rs is None or rs.meas_dist is None:
                        continue
                    reads_rew[rs.label] = None
                    d = rs.meas_dist
                    i = int(np.argmin(np.abs(np.log(d.f / fc))))
                    if abs(np.log2(d.f[i] / fc)) > 1 / 6 or f"H{k}" not in d.cols:
                        continue
                    cr = dsp.classify(d.cols[f"H{k}"][i], d.cols.get("Noise", np.full(len(d.f), np.nan))[i], margin)
                    rows.append([f"{d.f[i]:g}", f"H{k}", rs.label, _fmt_lvl(d.cols.get("Fundamental", [np.nan] * len(d.f))[i]),
                                 _fmt_h(cr), _fmt(cr.get("floor")), _fmt(cr.get("margin")), cr["kind"]])
                    reads_rew[rs.label] = cr
                    self._cmp_h(p, rs.label, fc, k, cr, ct, th, "REW")
                for name, ca in reads_ac2.items():
                    for label, cr in reads_rew.items():
                        self._cmp_h_rew(p, name, label, fc, k, ca, cr)
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

    def _mains_in_harmonic(self, fc, k):
        # A sweep reads harmonic k as the energy of a band around k·f at least ~30 Hz wide; a
        # mains line inside it is read as harmonic, while the steady sine resolves k·f alone.
        fk = k * fc
        half = dsp.sweep_harmonic_half_band(fk, MAINS_GUARD_HZ)
        return next((h for h in getattr(self, "mains_hz", []) if abs(h - fk) <= half), None), half

    def _cmp_h(self, p, name, fc, k, ca, ct, th, who):
        line, half = self._mains_in_harmonic(fc, k)
        # ac2 and REW both separate the harmonics against the stimulus: the truth of either is
        # the meas input's own harmonic re its own fundamental. The twin carries it so the shared
        # pass can tell whether the targets compare
        twin = {"key": f"{p.name}|h{k}|{fc:g}", "analyser": who, "truth": ct.get("value"),
                "agree": _tol(self.tol["harmonics"]["ac2_vs_rew_db"])[0]}
        if line is not None and ca["kind"] != "none":
            self.add(id=f"{p.name}.h{k}.{name}.{fc:g}", group="harmonics", path=p.name,
                     title=f"{name} H{k} at {fc:g} Hz vs steady sine", value=None, unit="dB", tol=th,
                     status="INCONCLUSIVE",
                     meaning=f"the mains line at {line:g} Hz lies inside the band ±{half:.0f} Hz around H{k} = "
                             f"{k * fc:.1f} Hz that a sweep reads as the harmonic",
                     detail={"ac2_or_rew": ca, "sine": ct, "mains_hz": line, "twin": twin})
            return
        if ca["kind"] == "value" and ct["kind"] == "value":
            d = ca["value"] - ct["value"]
            self.add(id=f"{p.name}.h{k}.{name}.{fc:g}", group="harmonics", path=p.name,
                     title=f"{name} H{k} at {fc:g} Hz vs steady sine", value=d, unit="dB", tol=th, status=judge(d, th),
                     meaning=f"{name} {ca['value']:.1f} dBr (floor {_fmt(ca.get('floor'))}), "
                             f"{ct.get('label', 'sine')} {ct['value']:.1f} dBr",
                     detail={"twin": twin})
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
                     detail={"ac2_or_rew": ca, "sine": ct, "twin": twin})

    def _cmp_h_rew(self, p, name, label, fc, k, ca, cr):
        """ac2's harmonic against REW's at the same tone, both sweeps of the same device: the
        agreement the shared pass needs before it calls a miss against the sine a shared one."""
        th = _tol(self.tol["harmonics"]["ac2_vs_rew_db"])
        base = dict(id=f"{p.name}.h{k}.{name}|{label}.{fc:g}", group="harmonics ac2 vs REW", path=p.name,
                    title=f"{name} H{k} at {fc:g} Hz vs {label}", unit="dB", tol=th)
        line, half = self._mains_in_harmonic(fc, k)
        ka, kr = (ca or {}).get("kind", "none"), (cr or {}).get("kind", "none")
        if ka == "none" and kr == "none":
            return
        if ka == "value" and kr == "value" and line is None:
            d = ca["value"] - cr["value"]
            self.add(**base, value=d, status=judge(d, th),
                     meaning=f"{name} {ca['value']:.1f} dBr, {label} {cr['value']:.1f} dBr: positive = ac2 reads "
                             "more; both are the meas input's own harmonic re its own fundamental",
                     detail={"ac2": ca, "rew": cr})
            return
        if line is not None:
            why = f"the mains line at {line:g} Hz lies inside the band ±{half:.0f} Hz that a sweep reads as H{k}"
        else:
            def say(who, c):
                return (f"{who} has no reading" if c is None or c["kind"] == "none" else
                        f"{who} is a floor bound < {c['bound']:.1f} dBr" if c["kind"] == "bound" else
                        f"{who} {c['value']:.1f} dBr")
            why = f"{say(name, ca)}, {say(label, cr)}: a bound or a missing reading on one side"
        self.add(**base, value=None, status="INCONCLUSIVE", meaning=why, detail={"ac2": ca, "rew": cr})

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
            noise_meas, src = meas[q], "the capture's silence before the sweep"
        elif p.noise is not None and len(p.noise.meas) >= n_cut:
            noise_meas, src = p.noise.meas[:n_cut], "the sine stage's silent recording"
        else:
            self.notes.append(f"{p.name}: {name}: no noise-only recording as long as the sweep's record "
                              f"({n_cut/fs:.1f} s): floor cross-check skipped")
            return
        if None in (s.start_hz, s.end_hz, s.duration_s, s.fade_in_s, s.fade_out_s):
            self.notes.append(f"{p.name}: {name}: the sweep's asked fades are not in its info.json: "
                              "floor cross-check skipped")
            return
        # A constant offset on an input is not noise: it has no power in any harmonic band, but
        # divided by the sweep's regularised spectrum near 0 Hz it spreads into the LF windows.
        # The noise-only record is taken about its own mean.
        noise_meas = noise_meas - np.mean(noise_meas)
        # ac2 separates harmonics against its emitted sweep, aligned by the reference's delay:
        # the meas input's noise alone reaches the floor, none of the reference input's.
        sweep = np.concatenate([np.zeros(start - a0), dsp.emitted_sweep(
            fs, s.start_hz, s.end_hz, s.duration_s, s.fade_in_s, s.fade_out_s)])
        _, (h, hn) = dsp.deconvolve_emitted(sweep, seg_ref, seg_meas, noise_meas)
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
                             f"{src}, as long as ac2's record ({n_cut/fs:.2f} s), divided like the sweep by ac2's emitted "
                             "sweep aligned to the reference's delay (the meas input's noise only), "
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
