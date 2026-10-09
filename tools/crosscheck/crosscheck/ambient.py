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
        cols["ac2 RTA"] = np.interp(np.log(fc), np.log(np.maximum(b[keys[0]], 1e-9)), b[keys[1]],
                                    left=np.nan, right=np.nan)
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
                          "FFT bins between ideal base-10 band edges over the whole window" + why)
    a.table("ambient: third-octave levels (dB SPL)", ["fc Hz"] + list(cols),
            [[f"{c:.0f}"] + [f"{cols[k][i]:.1f}" if np.isfinite(cols[k][i]) else "—" for k in cols]
             for i, c in enumerate(fc)])
    a.series["ambient.thirds"] = {"x": fc.tolist(), **{k: np.asarray(v).tolist() for k, v in cols.items()},
                                  "xlabel": "Hz", "ylabel": "dB SPL", "logx": True}
    lines = dsp.mains_lines(x, fs, run.manifest.get("mains_hz", 50.0))
    a.table("ambient: mains-family lines on the mic input", ["Hz", "above local median dB"],
            [[f"{q['hz']:g}", f"{q['above_db']:.1f}"] for q in lines],
            "sine frequencies are nudged so H2..H5 stay off 50 Hz multiples (dsp.avoid_mains)")
