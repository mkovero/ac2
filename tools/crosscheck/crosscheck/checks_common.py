"""Shared pieces of the analysis: tolerances, the Check record and its judging, the shared
pass over twin checks, and small numeric and formatting helpers used by every check family."""
from __future__ import annotations

import math
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from . import dsp, dut

TOLERANCES = Path(__file__).resolve().parent.parent / "tolerances.toml"
# A mains line leaks about ±1/gate (≈1.1 Hz for ac2's 0.89 s default gate) into the gated
# estimates; twice that keeps the masked columns clear of it.
MAINS_GUARD_HZ = 2.0
# How long after a clarity boundary the decay must stay above the noise before C50/C80 count as
# measured: within it, the late energy past the boundary is mostly the noise-floor correction.
CLARITY_GUARD_S = 0.010
# Above this D50 the late energy is a trace and C = 10·log10(D/(1 − D)) rests on it alone.
CLARITY_D_MAX = 0.998


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


def take_difference(h_take: complex, h_ref: complex) -> tuple[float, float]:
    """How far one take's path differs from another's at one frequency, |Δ dB| and |Δ°|, from
    each recording's direct estimate read narrow at that frequency (no column, so no fine
    structure in it). A path whose gain depends on its stimulus or drifts with exposure (a
    loudspeaker's drivers under continuous noise against a sweep's few milliseconds per
    frequency) reads differently in each take; a source of one take judged against a truth
    taken under another stimulus carries that difference, not an error of its own."""
    if not (np.isfinite(h_take) and np.isfinite(h_ref)) or h_take == 0 or h_ref == 0:
        return 0.0, 0.0
    r = h_take / h_ref
    return abs(float(dsp.db(r))), abs(float(np.rad2deg(np.angle(r))))


def sine_uncertainty(room: float, take: float, noise: float) -> float:
    """A source's uncertainty against a steady sine (2σ-like, the value's units): the room's
    fine structure between a column and the sine and the take's own path difference are
    systematic and add, each its own term of the source minus the sine; the column's random
    noise covers both where it is the larger."""
    return max(room + take, noise)


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


def _twin(path: str, kind: str, fc: float, source: str, agree: float) -> dict:
    """The shared pass's pairing of a check against the steady-sine truth: an ac2 sweep's and
    REW's reading of the same quantity at the same tone share a key. `agree` is the limit on
    |ac2 − REW| within which the two read the same (the pass limit of their own comparison).
    Other sources (the live TF, the direct estimates) are no sweep analyser and get none."""
    who = "ac2" if source.startswith("ac2 sweep") or source.startswith("ac2 as displayed") or \
        source.startswith("ac2 ±") else "REW" if source.startswith("REW") else None
    return {"twin": {"key": f"{path}|{kind}|{fc:g}", "analyser": who, "agree": agree}} if who else {}


def _finite(v) -> bool:
    return v is not None and np.isfinite(v)


def shared_pass(checks: list[dict]) -> int:
    """Annotates, never re-judges: an ac2 sweep and REW's twin both miss the same truth (WARN or
    FAIL) by residuals of one sign whose difference is within `agree` (the two analysers read
    the same number), against targets that compare. A harmonic's targets differ by analyser
    (ac2's net of the reference's harmonics, REW's the meas input alone), so they compare only
    within half the vs-truth pass limit; otherwise the residuals answer different questions.
    The annotation says the miss is shared, not why: a common stimulus, device, convention or
    reference calculation would each produce it. Severity and value stay as judged, so a
    shared error that grows is still a regression. Works on check dicts (`vars(Check)` mutates
    the Check). Returns the number annotated."""
    groups: dict[str, dict[str, list]] = {}
    for c in checks:
        t = (c.get("detail") or {}).get("twin")
        if t and t.get("analyser") and c["status"] in ("WARN", "FAIL") and _finite(c.get("value")):
            groups.setdefault(t["key"], {}).setdefault(t["analyser"], []).append(c)
    best: dict[int, tuple[float, dict, dict]] = {}
    for key, g in groups.items():
        for a in g.get("ac2", []):
            for r in g.get("REW", []):
                va, vr = a["value"], r["value"]
                if va == 0 or vr == 0 or (va > 0) != (vr > 0):
                    continue
                ta, tr = a["detail"]["twin"], r["detail"]["twin"]
                agree = min(ta["agree"], tr["agree"])
                d = va - vr
                if abs(d) > agree:
                    continue
                if _finite(ta.get("truth")) or _finite(tr.get("truth")):
                    if not (_finite(ta.get("truth")) and _finite(tr.get("truth"))):
                        continue
                    if abs(ta["truth"] - tr["truth"]) > min(a["tol"][0], r["tol"][0]) / 2:
                        continue
                ann = {"key": key, "ac2": float(va), "REW": float(vr), "diff": float(d), "agree": float(agree)}
                for me, other in ((a, r), (r, a)):
                    if id(me) not in best or abs(d) < best[id(me)][0]:
                        best[id(me)] = (abs(d), me, {**ann, "twin": other["title"]})
    for _, c, ann in best.values():
        c["detail"] = {**c["detail"], "shared": ann}
        c["meaning"] += (f"; ac2 and REW miss the truth alike (ac2 − REW {ann['diff']:+.3g} {c.get('unit') or ''}"
                         f" within {ann['agree']:g}; twin: {ann['twin']})")
    return len(best)


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
