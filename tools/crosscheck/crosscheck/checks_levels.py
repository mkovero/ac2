"""Check families: the sources on one grid and the level convention."""
from __future__ import annotations

import numpy as np

from . import dsp
from .checks_common import (
    MAINS_GUARD_HZ,
    _f,
    _tol,
    direct_ir_peak,
    judge,
)
from .model import PathData


class LevelChecks:
    """Sources on ac2's grid and the level convention; mixed into Analysis."""

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
        # came from, against noise-only records of the same input: the silent recording, and the
        # take's own stretches before and after its stimulus. Each column takes the larger: a
        # noise that is there during a take but not in the silent recording (a machine tone that
        # starts later) is in the take's own quiet stretches, and a short stretch that resolves
        # too little is covered by the long silent recording. A windowed deconvolution (ac2's
        # sweep) rejects more noise than the whole recording holds, so this is an upper bound.
        self.noise_rel = {}
        rec_of, coh_of = {}, {}
        for name, s in p.sweeps.items():
            if s.raw is not None:
                rec_of[f"ac2 sweep {name}"] = rec_of[f"direct (ac2 capture {name})"] = s.raw
                coh_of[id(s.raw)] = self.capture_coh.get(f"direct (ac2 capture {name})")
        if p.rec is not None:
            rec_of["REW offline"] = rec_of["direct (REW recording)"] = p.rec
        cache = {}
        for name, raw in rec_of.items():
            if name not in src:
                continue
            if id(raw) not in cache:
                z = np.zeros(len(f))
                if p.noise is not None and raw.fs == p.noise.fs:
                    z = dsp.band_noise_rel(raw.meas, p.noise.meas, raw.fs, f, self.frac, coh=coh_of.get(id(raw)))
                own = dsp.quiet_parts(raw.meas, raw.ref, raw.fs)
                if own:
                    z = np.fmax(z, dsp.band_noise_rel(raw.meas, own, raw.fs, f, self.frac, coh=coh_of.get(id(raw)),
                                                         hann=True))
                cache[id(raw)] = z
            self.noise_rel[name] = cache[id(raw)]
        # The live TF's own noise from its coherence over its FIFO blocks: σ = √((1 − γ²) / (2 γ² n)).
        # ac2's γ² is a per-bin coherence averaged over the column, so this is a bin's noise.
        # n = blocks counts every block as independent, though they overlap, so this is a lower
        # bound on the TF's noise and the gate leaves out too few columns rather than too many.
        if "ac2 TF" in self.coh and p.tf_blocks:
            g = np.clip(np.nan_to_num(self.coh["ac2 TF"]), 1e-6, 1.0)
            self.noise_rel["ac2 TF"] = np.sqrt((1 - g) / (2 * g * p.tf_blocks))
        self.src = src
        self.series[p.name] = {"f": f, "src": src}
        return f

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
