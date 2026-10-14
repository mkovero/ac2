"""Check family: magnitude and phase per band against the steady-sine truth and between sources."""
from __future__ import annotations

import numpy as np

from . import dsp
from .checks_common import (
    MAINS_GUARD_HZ,
    _f,
    _tol,
    _twin,
    judge,
    judge_u,
    sine_uncertainty,
    take_difference,
    worst,
)
from .model import PathData


class BandChecks:
    """Magnitude and phase per band; mixed into Analysis."""

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
                          + ("(REW's meas import with its soundcard cal, referred to REW's own timing marker)"
                             if p.rew.meas_alone else
                             "(REW meas ÷ REW ref, each referred to REW's own timing marker)")))
        if "REW ÷ ref" in S:
            pairs.append((primary, "REW ÷ ref", False, "ac2's sweep against REW's own reference division "
                          "(REW's Arithmetic A / B of its meas and ref imports)"))
        if "direct (REW recording)" in S:
            if "REW offline" in S:
                pairs.append(("REW offline", "direct (REW recording)", False,
                              "REW against numpy on the very same recording: REW's deconvolution alone"))
            if "REW ÷ ref" in S:
                pairs.append(("REW ÷ ref", "direct (REW recording)", False,
                              "REW's own A / B against numpy on the very same recording"))
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
            fit_lo, fit_hi = (1000, 20000) if elec else (200, 5000)
            rm = (f >= fit_lo) & (f <= fit_hi) & m_all
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
            # cannot be judged: left out and counted. Each noise_rel is a complex relative error
            # |δH|/|H|; half its variance is the magnitude's.
            zero = np.zeros(len(f))
            za = self.gate_noise_rel.get(a, self.noise_rel.get(a, zero))
            zb = self.gate_noise_rel.get(b, self.noise_rel.get(b, zero))
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
                                             "the recordings' SNR against their noise-only records, or the live TF's coherence over "
                                             f"its blocks) than the pass limit and {nd} more uncorrelated content (harmonics, "
                                             "−10·log10 γ² of the direct estimate); too few left to judge")
                    continue
                nm = int((self.mains_cols & (f >= lo) & (f < hi)).sum())
                # the band's columns before any mask: n of these is the population judged
                nall = int(((f >= lo) & (f < hi)).sum())
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
                         detail={"mean": mean, "spread": spread, "n": int(m.sum()), "of": nall})
                self.add(id=f"{p.name}.phase.{a}|{b}.{lo}-{hi}", group="phase", path=p.name,
                         title=f"{a} − {b}, {lo}–{hi} Hz, phase", value=max(abs(pmean), pspread), unit="°", tol=tp,
                         status=st_p,
                         meaning=f"phase difference after removing a pure delay difference of {tau*1e6:+.2f} µs "
                                 f"(fitted {fit_lo / 1000:g}–{fit_hi / 1000:g} kHz); mean {pmean:+.3f}°, spread ±{pspread:.3f}°",
                         detail={"mean": pmean, "spread": pspread, "n": int(m.sum()), "of": nall,
                                 "delay_removed_s": tau})
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
                # Each source of a take of its own, judged against the sine: its capture read
                # narrow at the sine against the REW recording's narrow reading is how far the
                # path itself stood apart in that take (a speaker under continuous pink noise
                # loses tweeter-band gain within seconds that a sweep or a 2 s sine never shows)
                take = {}
                ref_n = narrows.get("direct (REW recording)")
                if ref_n is not None and not elec:
                    tf_cap, sw_cap = "direct (TF capture)", f"direct (ac2 capture {p.primary})"
                    for src_name, cap in ((primary, sw_cap), (sw_cap, sw_cap), ("ac2 TF", tf_cap), (tf_cap, tf_cap)):
                        if cap in narrows:
                            take[src_name] = take_difference(narrows[cap][0], ref_n[0])
                names = [primary, "REW live", "REW offline", "REW ÷ ref", "direct (REW recording)", "ac2 TF"]
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
                    # A column's own noise (2σ from its recording's SNR at fc): where the tone sits
                    # near the noise, the column reads the noise's energy along with the response
                    # and the sine's probe cannot be told from it either
                    zc = 0.0
                    if not narrow_row and name in self.noise_rel:
                        zc = float(np.interp(np.log(fc), np.log(f), np.nan_to_num(self.noise_rel[name], posinf=1.0)))
                    # at a relative error of 1 the noise holds as much as the response: the column
                    # tells nothing, and no larger figure means more
                    swamped, zc = zc >= 1.0, min(zc, 1.0)
                    # zc is a complex relative error: half its variance is the magnitude's, half the phase's
                    nz_db, nz_deg = 2 * 20 / np.log(10) * zc / np.sqrt(2), 2 * np.degrees(zc / np.sqrt(2))
                    room_db, room_deg = ud, up
                    tk_db, tk_deg = (0.0, 0.0) if narrow_row else take.get(name, (0.0, 0.0))
                    ud, up = sine_uncertainty(ud, tk_db, nz_db), sine_uncertainty(up, tk_deg, nz_deg)
                    if not narrow_row:
                        self.sine_u.setdefault(p.name, {})[(name, fc)] = ud
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
                        elif name == "REW ÷ ref" and self.rew_div_put_back:
                            note = (f" (REW ÷ ref: {self.rew_div_put_back*1e6:+.2f} µs put back, the delay its "
                                    "imports' timing markers took out)")
                        elif name.startswith("ac2 sweep"):
                            note = (f" (ac2: its reported arrival {self.arrival.get(p.primary, 0)*1e6:.3f} µs put "
                                    "back into the phase it refers to that arrival)")
                        self.add(id=f"{p.name}.sine_phase.{name}.{fc:g}", group="phase vs sine", path=p.name,
                                 title=f"{name} phase at {fc:g} Hz vs steady sine", value=dpd, unit="°", tol=tp,
                                 status="INFO" if info_only else judge_u(dpd, tp, up),
                                 meaning=f"sine {tone['ratio_deg']:+.3f}°, {name} {np.rad2deg(np.angle(h)):+.3f}°{note}"
                                         + ((f"; the narrow band's own noise (coherence) ±{up:.1f}°" if narrow_row else
                                             f"; the room's fine structure: a 1/{self.ppo}-oct column and the sine's frequency "
                                             f"differ by {room_deg:.1f}° in the direct estimate") if room_deg else "")
                                         + (f"; its own take read narrow at the sine differs from the REW recording's "
                                            f"by {tk_deg:.1f}° (the path under this stimulus)" if tk_deg >= 0.05 else "")
                                         + (("; the column's noise-only records hold as much energy as its recording"
                                             if swamped else f"; the column's own noise (2σ, its recording's SNR) ±{nz_deg:.1f}°")
                                            if nz_deg > room_deg + tk_deg else ""),
                                 detail={"uncertainty": up, **_twin(p.name, "sine_phase", fc, name, tp[0])})
                    if dmd is not None:
                        tm = _tol(self.tol["band"]["mag_lf" if elec else "mag_acoustic"])
                        # ac2 and REW agree on a magnitude when they would pass against each
                        # other in that band (the same limits as their band comparison)
                        agree = _tol(self.tol["band"][(("mag_lf" if fc < 100 else "mag_mf" if fc < 1000 else "mag_hf")
                                                       if elec else "mag_acoustic")])[0]
                        self.add(id=f"{p.name}.sine_mag.{name}.{fc:g}", group="magnitude vs sine", path=p.name,
                                 title=f"{name} magnitude at {fc:g} Hz vs steady sine", value=dmd, unit="dB", tol=tm,
                                 status=judge_u(dmd, tm, ud) if not (name.startswith("REW live") or info_only) else "INFO",
                                 meaning=f"sine {tone['ratio_db']:+.3f} dB (meas ÷ ref), {name} {dsp.db(h):+.3f} dB"
                                         + ((f"; the narrow band's own noise (coherence) ±{ud:.2f} dB" if narrow_row else
                                             f"; the room's fine structure: a 1/{self.ppo}-oct column and the sine's frequency "
                                             f"differ by {room_db:.2f} dB in the direct estimate") if room_db else "")
                                         + (f"; its own take read narrow at the sine differs from the REW recording's "
                                            f"by {tk_db:.2f} dB (the path under this stimulus)" if tk_db >= 0.005 else "")
                                         + (("; the column's noise-only records hold as much energy as its recording"
                                             if swamped else f"; the column's own noise (2σ, its recording's SNR) ±{nz_db:.2f} dB")
                                            if nz_db > room_db + tk_db else ""),
                                 detail={"uncertainty": ud, **_twin(p.name, "sine_mag", fc, name, agree)})
                    rows.append([f"{fc:g}", name, "" if dmd is None else f"{dmd:+.3f}", "" if dpd is None else f"{dpd:+.3f}"])
            self.table(f"{p.name}: against the steady sines", ["f Hz", "source", "Δ dB", "Δ°"], rows)
