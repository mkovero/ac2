"""Ambient SPL without emission: ac2's SPL meters, REW's SPL meter and a numpy Leq of the
same seconds recorded on the mic input, all with the calibration from ac2's store."""
from __future__ import annotations

import json

import numpy as np

from . import dsp, formats, wav


def _last_json_line(p):
    if not p.exists():
        return None
    last = None
    for line in p.read_text().splitlines():
        line = line.strip()
        if line.startswith("{"):
            try:
                last = json.loads(line)
            except json.JSONDecodeError:
                pass
    return last


def _key(d, *names):
    """First value under any of `names`, case-insensitive (REW's level JSON is not
    documented beyond its schema names)."""
    if not isinstance(d, dict):
        return None
    low = {k.lower(): v for k, v in d.items()}
    for n in names:
        v = low.get(n.lower())
        if isinstance(v, (int, float)):
            return float(v)
    return None


def _rew_rta(j):
    """REW's RTA data: a base64 curve like the measurement exports."""
    if not isinstance(j, dict) or "magnitude" not in j:
        return None
    try:
        c = formats.read_rew_curve(j)
    except (KeyError, ValueError, TypeError):
        return None
    return c.f, c.mag


def butterworth_band_gain(f: np.ndarray, f_lower: float, f_upper: float, fs: float, order: int = 3) -> np.ndarray:
    """|H(f)|² of ac2's RTA band filter, from its design equations: an analog Butterworth
    low-pass of `order` mapped to a band-pass (s → (s² + ω0²)/(s·BW)) whose −3 dB edges are the
    pre-warped band edges, then the bilinear transform. The bilinear map takes the digital
    frequency f to the analog Ω = 2fs·tan(πf/fs), so the digital response is the analog one at Ω:
    |H|² = 1 / (1 + ((Ω² − ωl·ωu) / (Ω·(ωu − ωl)))^(2N)), exactly 1 at the centre √(ωl·ωu)."""
    k = 2 * fs
    w = k * np.tan(np.pi * np.clip(np.asarray(f, float), 1e-9, fs / 2 * (1 - 1e-9)) / fs)
    wl, wu = k * np.tan(np.pi * f_lower / fs), k * np.tan(np.pi * f_upper / fs)
    r = (w * w - wl * wu) / (w * (wu - wl))
    return 1 / (1 + r ** (2 * order))


def band_curve_db(curve, lo: float, hi: float, n: int = 256) -> float:
    """The mic correction ac2 adds to a band level: the power average of the per-frequency
    correction over the band, sampled evenly in log-frequency (exact for pink noise in an ideal
    band; any other spectrum makes it differ from correcting each bin)."""
    lf = np.exp(np.log(lo) + (np.log(hi) - np.log(lo)) * (np.arange(n) + 0.5) / n)
    return float(10 * np.log10(np.mean(10 ** (dsp.mic_correction_db(curve, lf) / 10))))


def rta_filter_model(x: np.ndarray, fs: float, sensitivity_db: float, curve, fc: np.ndarray,
                     order: int = 3) -> tuple[np.ndarray, np.ndarray]:
    """Band levels in dB SPL as ac2's filterbank reads the whole recording: each base-10
    third-octave band's Butterworth power response applied to one periodogram of the segment
    (its mean square is the band output's mean square, by Parseval), then the band's
    log-frequency power average of the mic correction. Also returns the ideal-edge levels with
    that same band-averaged correction, which splits the filter skirts from the correction.

    A band whose upper edge lies above fs/4 is NaN: the bilinear map compresses its upper
    skirt there and ac2 raises its order until the class 1 mask passes, which this model
    does not repeat."""
    X = np.fft.rfft(x - np.mean(x))
    f = np.fft.rfftfreq(len(x), 1 / fs)
    P = np.abs(X) ** 2
    P[1:-1] *= 2
    filt, ideal = [], []
    for c in fc:
        lo, hi = c * 10 ** -0.05, c * 10 ** 0.05
        if hi > fs / 4:
            filt.append(np.nan)
            ideal.append(np.nan)
            continue
        corr = band_curve_db(curve, lo, hi) if curve else 0.0
        for out, g in ((filt, butterworth_band_gain(f, lo, hi, fs, order)), (ideal, (f >= lo) & (f < hi))):
            ms = float(np.sum(g * P)) / len(x) ** 2
            out.append(10 * np.log10(max(ms, 1e-300) * 2) + sensitivity_db + corr)
    return np.array(filt), np.array(ideal)


def _on_centres(fc: np.ndarray, f_src: np.ndarray, v: np.ndarray) -> np.ndarray:
    """A band-level column read at fc: the exported centres are the same base-10 frequencies,
    rounded differently, so a strict interpolation range would drop the outermost band."""
    f_src = np.asarray(f_src, float)
    out = np.full(len(fc), np.nan)
    for i, c in enumerate(fc):
        j = int(np.argmin(np.abs(np.log(np.maximum(f_src, 1e-9) / c))))
        if abs(np.log(f_src[j] / c)) < 1e-3:
            out[i] = v[j]
    return out


def analyse(a, run):
    from .analyse import _tol, judge
    d = run.ambient["dir"]
    win = run.ambient.get("window") or {}
    cal = run.cal or {}
    S = cal.get("sensitivity_db")
    curve = cal.get("curve_points")
    if not (d / "in1.wav").exists() or S is None:
        a.notes.append("ambient: no recording or no calibration excerpt: skipped")
        return
    fs, x = wav.read(d / "in1.wav")
    x = x[:, 0]
    tl = _tol(a.tol["level"]["ambient_leq_db"])
    rows = []
    for w in ("Z", "A", "C"):
        mine = dsp.leq_spectrum(x, fs, S, w, curve)
        ac2 = _last_json_line(d / f"ac2_spl_{w.lower()}.jsonl")
        spl = (ac2 or {}).get("spl") or {}
        ac2_leq, ac2_dur = _key(spl, "leq"), _key(spl, "duration")
        if spl and str(spl.get("scale", "")).lower() == "dbfs":
            a.notes.append(f"ambient: ac2's {w} meter read dBFS (input not calibrated in the daemon); "
                           "its Leq is not compared")
            ac2_leq = None
        rew = json.loads((d / f"rew_spl_{w}.json").read_text()) if (d / f"rew_spl_{w}.json").exists() else None
        rew_leq = _key(rew, "leq", "Leq")
        rew_dur = _key(rew, "elapsedTime", "elapsed", "duration")
        rows.append([w, f"{mine:.2f}",
                     "—" if ac2_leq is None else f"{ac2_leq:.2f}" + (f" ({ac2_dur:.1f} s)" if ac2_dur else ""),
                     "—" if rew_leq is None else f"{rew_leq:.2f}" + (f" ({rew_dur:.1f} s)" if rew_dur else "")])
        for name, v in (("ac2", ac2_leq), ("REW", rew_leq)):
            if v is None:
                continue
            dd = v - mine
            a.add(id=f"ambient.leq.{w}.{name}", group="ambient SPL", path="ambient",
                  title=f"L{w}eq {name} vs numpy on the recording", value=dd, unit="dB", tol=tl,
                  status=judge(dd, tl),
                  meaning=f"{name} {v:.2f} dB SPL, numpy {mine:.2f} dB SPL from the mic-input recording "
                          f"(full-scale-sine dBFS + {S:.2f} dB, mic curve {cal.get('curve_label', 'none')} "
                          f"normalised at 1 kHz, analytic {w} weighting); the three windows overlap "
                          "within the start skew in window.json")
        if ac2_leq is not None and rew_leq is not None:
            dd = ac2_leq - rew_leq
            a.add(id=f"ambient.leq.{w}.ac2_rew", group="ambient SPL", path="ambient", title=f"L{w}eq ac2 vs REW",
                  value=dd, unit="dB", tol=tl, status=judge(dd, tl),
                  meaning=f"ac2 {ac2_leq:.2f}, REW {rew_leq:.2f} dB SPL, both calibrated from ac2's store")
    a.table("ambient: Leq over the window (dB SPL)", ["weighting", "numpy", "ac2", "REW"], rows,
            f"window {win.get('seconds', '?')} s; start skew {win.get('skew_s', '?')} s")
    fc, lv = dsp.third_octave_levels(x, fs, S, curve)
    cols = {"numpy": lv}
    if (d / "ac2_rta.csv").exists():
        t = formats.read_ac2_csv(d / "ac2_rta.csv")
        b = next(iter(t.blocks.values()))
        keys = list(b)
        cols["ac2 RTA"] = _on_centres(fc, b[keys[0]], np.asarray(b[keys[1]], float))
    if (d / "rew_rta.json").exists():
        rr = _rew_rta(json.loads((d / "rew_rta.json").read_text()))
        if rr is not None:
            cols["REW RTA"] = np.interp(np.log(fc), np.log(np.maximum(rr[0], 1e-9)), rr[1], left=np.nan, right=np.nan)
        else:
            a.notes.append("ambient: rew_rta.json not in the curve format the suite decodes; not compared")
    tb = _tol(a.tol["level"]["ambient_band_db"])
    for k in cols:
        if k == "numpy":
            continue
        dd = cols[k] - lv
        m = np.isfinite(dd) & (fc >= 25) & (fc <= 16000)
        if m.any():
            v = float(np.max(np.abs(dd[m])))
            st, why = judge(v, tb), ""
            if k == "ac2 RTA":
                avg = win.get("ac2_rta_average")
                if avg is None:
                    # A run from before the stage set averaging: the captured trace is one
                    # analysis frame, a sample of a fluctuating noise, not the window's mean power
                    # the numpy column holds
                    st, why = "INCONCLUSIVE", ("; ac2's RTA was captured without averaging (one frame), "
                                               "so it is not an estimate over the window")
                else:
                    why = (f"; ac2's RTA averaged power with --average {avg}, a FIFO longer than the run: "
                           "every result interval from its start to the capture, weighted by its length "
                           "(the window plus the few seconds of the same room around it)")
            elif k == "REW RTA":
                # REW's 'Forever' average reads low against the power mean, increasingly at HF:
                # an average of levels in dB under-reads noise (the mean of a log is below the log
                # of the mean). Context, not a judgement of the suite's reference.
                st, why = "INFO", ("; REW's own averaging (Forever) reads below the power mean, consistent "
                                   "with averaging levels in dB")
            a.add(id=f"ambient.bands.{k}", group="ambient SPL", path="ambient",
                  title=f"{k} vs numpy, third octaves 25 Hz-16 kHz", value=v, unit="dB",
                  tol=None if st == "INFO" else tb, status=st,
                  meaning=f"largest |difference| over {int(m.sum())} bands, median {np.median(dd[m]):+.2f} dB; "
                          "an RTA reads a spectral density on its own resolution, the numpy column sums "
                          "FFT bins between ideal base-10 band edges over the whole window" + why,
                  detail={"n": int(m.sum()), "f_hz": float(fc[m][np.argmax(np.abs(dd[m]))])})
    if "ac2 RTA" in cols:
        # ac2's RTA against its own filters applied to the same recording: what is left is
        # ac2's implementation (and its averaging window), not the band definition
        filt, ideal = rta_filter_model(x, fs, S, curve, fc)
        dm = cols["ac2 RTA"] - filt
        m = np.isfinite(dm) & (fc >= 25) & (fc <= 16000)
        if m.any():
            dn = cols["ac2 RTA"] - lv
            dc = ideal - lv
            for i in np.where(m)[0]:
                a.add(id=f"ambient.bands.ac2 RTA|own filters.{fc[i]:.0f}", group="ambient SPL", path="ambient",
                      title=f"ac2 RTA vs its own filters on the recording, {fc[i]:.0f} Hz", value=float(dm[i]),
                      unit="dB", tol=None, status="INFO",
                      meaning=f"ac2 {cols['ac2 RTA'][i]:.2f} dB SPL, its order-3 Butterworth band applied to the "
                              f"recording's periodogram {filt[i]:.2f}, ideal edges {lv[i]:.2f}: definition "
                              f"(filter − ideal) {filt[i] - lv[i]:+.3f} dB, of it the band-averaged mic "
                              f"correction {dc[i]:+.3f} dB; ac2 − ideal {dn[i]:+.3f} dB",
                      detail={"ac2_minus_ideal": float(dn[i]), "filter_minus_ideal": float(filt[i] - lv[i]),
                              "curve_band_minus_bins": float(dc[i])})
            k = int(np.where(m)[0][np.argmax(np.abs(dm[m]))])
            kn = int(np.where(m)[0][np.argmax(np.abs(dn[m]))])
            a.add(id="ambient.bands.ac2 RTA|own filters", group="ambient SPL", path="ambient",
                  title="ac2 RTA vs its own filters on the recording, third octaves 25 Hz-16 kHz",
                  value=float(dm[k]), unit="dB", tol=None, status="INFO",
                  meaning=f"signed residual at the band of largest |residual| ({fc[k]:.0f} Hz) over {int(m.sum())} "
                          f"bands, median {np.median(dm[m]):+.3f} dB. The model is ac2's band filter from its design "
                          "equations (order-3 Butterworth, bilinear, pre-warped base-10 edges) and its band-averaged "
                          f"mic correction applied to the recording; against ideal edges ac2's largest |difference| "
                          f"is {dn[kn]:+.3f} dB at {fc[kn]:.0f} Hz, of which the definition accounts for "
                          f"{filt[kn] - lv[kn]:+.3f} dB",
                  detail={"n": int(m.sum()), "f_hz": float(fc[k]), "median": float(np.median(dm[m])),
                          "ac2_minus_ideal_worst": float(dn[kn]), "ac2_minus_ideal_worst_f_hz": float(fc[kn]),
                          "filter_minus_ideal_at_worst": float(filt[kn] - lv[kn])})
        cols["ac2 filter model"] = filt
    a.table("ambient: third-octave levels (dB SPL)", ["fc Hz"] + list(cols),
            [[f"{c:.0f}"] + [f"{cols[k][i]:.1f}" if np.isfinite(cols[k][i]) else "—" for k in cols]
             for i, c in enumerate(fc)])
    a.series["ambient.thirds"] = {"x": fc.tolist(), **{k: np.asarray(v).tolist() for k, v in cols.items()},
                                  "xlabel": "Hz", "ylabel": "dB SPL", "logx": True}
    lines = dsp.mains_lines(x, fs, run.manifest.get("mains_hz", 50.0))
    a.table("ambient: mains-family lines on the mic input", ["Hz", "above local median dB"],
            [[f"{q['hz']:g}", f"{q['above_db']:.1f}"] for q in lines],
            "sine frequencies are nudged so H2..H5 stay off 50 Hz multiples (dsp.avoid_mains)")
