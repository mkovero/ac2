"""Delay-finder scenario fixtures (design Q1, docs/design/q1-delay-finder.md).

Synthetic, deterministic multipath scenarios: a reference (loopback) block and a
measurement block with known paths (fractional delays, gains, polarity), coloured
excitation and in-band noise at a stated SNR. Each golden set carries the inputs, the
analytic truth (paths, first significant arrival) and the expected finder outcome class.

The synthesis functions are also imported by tools/experiments/q1 for the Monte-Carlo runs.
"""


import math
import pathlib
import sys
from dataclasses import dataclass, field

import numpy as np

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))

FS = 48000.0
THRESHOLD_DB = -12.0  # decision 1a


# --------------------------------------------------------------------------------------
# Band weighting (identical definition to the finder: -6 dB edges, raised cosine in log f)
# --------------------------------------------------------------------------------------

BAND_EDGES = {"full": (2000.0, 16000.0), "mid": (300.0, 3000.0), "sub": (20.0, 120.0)}
TAPER_OCT = 1.0


def band_weights(freqs: np.ndarray, band: str, fs: float) -> np.ndarray:
    f_lo, f_hi = BAND_EDGES[band]
    f_hi = min(f_hi, 0.5 * fs * 2.0 ** (-TAPER_OCT / 2.0))
    out = np.zeros_like(freqs)
    pos = freqs > 0
    lf = np.log2(np.where(pos, freqs, 1.0))

    def ramp(x):
        u = np.clip(x / TAPER_OCT + 0.5, 0.0, 1.0)
        return 0.5 - 0.5 * np.cos(np.pi * u)

    w = ramp(lf - math.log2(f_lo)) * ramp(math.log2(f_hi) - lf)
    out[pos] = w[pos]
    return out


# --------------------------------------------------------------------------------------
# Excitation
# --------------------------------------------------------------------------------------


def _shape(white: np.ndarray, fs: float, mag) -> np.ndarray:
    n = len(white)
    f = np.fft.rfftfreq(n, 1.0 / fs)
    X = np.fft.rfft(white)
    return np.fft.irfft(X * mag(f), n)


def _pink_mag(f):
    return np.where(f > 0, 1.0 / np.sqrt(np.maximum(f, 10.0)), 0.0)


def _bp_mag(lo, hi, order=4):
    def mag(f):
        fp = np.maximum(f, 1e-9)
        return 1.0 / np.sqrt(1.0 + (lo / fp) ** (2 * order)) / np.sqrt(1.0 + (fp / hi) ** (2 * order))
    return mag


def music_like(n: int, fs: float, rng: np.random.Generator) -> np.ndarray:
    """Programme-like excitation: harmonic notes, kick, hats, and a quiet pink bed.

    Strongly coloured (most energy below 1 kHz, steep HF roll-off), tonal and
    non-stationary: the hard case for a whitening estimator.
    """
    t_note, t_beat = 0.25, 0.5
    x = np.zeros(n)
    i = 0
    note_len = int(t_note * fs)
    while i < n:
        midi = rng.integers(36, 84)
        f0 = 440.0 * 2.0 ** ((midi - 69) / 12.0)
        L = min(int(note_len * rng.uniform(1.0, 3.0)), n - i)
        tt = np.arange(L) / fs
        env = np.exp(-tt / rng.uniform(0.15, 0.6)) * (1 - np.exp(-tt / 0.005))
        h = np.zeros(L)
        for k in range(1, 12):
            if k * f0 >= 0.45 * fs:
                break
            h += (1.0 / k ** 1.3) * np.sin(2 * np.pi * k * f0 * tt + rng.uniform(0, 2 * np.pi))
        x[i : i + L] += 0.5 * env * h
        i += note_len
    beat = int(t_beat * fs)
    kl = int(0.2 * fs)
    tt = np.arange(kl) / fs
    kick = np.sin(2 * np.pi * (50 * tt + 70 * 0.04 * (1 - np.exp(-tt / 0.04)))) * np.exp(-tt / 0.08)
    hl = int(0.04 * fs)
    for b in range(0, n, beat):
        L = min(kl, n - b)
        x[b : b + L] += 0.8 * kick[:L]
    for b in range(beat // 4, n, beat // 2):
        L = min(hl, n - b)
        burst = _shape(rng.standard_normal(hl), fs, _bp_mag(5000.0, 16000.0))
        x[b : b + L] += 0.3 * burst[:L] * np.exp(-np.arange(L) / fs / 0.01) / (np.std(burst) + 1e-12)
    bed = _shape(rng.standard_normal(n), fs, _pink_mag)
    x /= np.std(x)
    bed /= np.std(bed)
    return x + 10 ** (-25 / 20) * bed


EXCITATIONS = ("white", "pink", "bandlimited", "narrow", "music", "music_hf", "subonly")


def excitation(kind: str, n: int, fs: float, rng: np.random.Generator) -> np.ndarray:
    if kind == "white":
        x = rng.standard_normal(n)
    elif kind == "pink":
        x = _shape(rng.standard_normal(n), fs, _pink_mag)
    elif kind == "bandlimited":  # pink, 100 Hz - 8 kHz
        x = _shape(rng.standard_normal(n), fs, lambda f: _pink_mag(f) * _bp_mag(100.0, 8000.0)(f))
    elif kind == "narrow":  # pink, 100 Hz - 1.2 kHz: does not excite the full-range band
        x = _shape(rng.standard_normal(n), fs, lambda f: _pink_mag(f) * _bp_mag(100.0, 1200.0)(f))
    elif kind == "subonly":  # 25 - 150 Hz only (a sub feed)
        x = _shape(rng.standard_normal(n), fs, _bp_mag(25.0, 150.0))
    elif kind == "music":
        x = music_like(n, fs, rng)
    elif kind == "music_hf":  # music with an extra -6 dB/oct above 2 kHz (dull programme)
        x = music_like(n, fs, rng)
        x = _shape(x, fs, lambda f: 1.0 / np.sqrt(1.0 + (np.maximum(f, 1.0) / 2000.0) ** 2))
    else:
        raise ValueError(kind)
    return x / np.std(x)


# --------------------------------------------------------------------------------------
# Scenario synthesis
# --------------------------------------------------------------------------------------


@dataclass
class Path:
    delay: float  # absolute, samples (fractional allowed), positive = meas late
    gain_db: float
    polarity: int = 1


@dataclass
class Scenario:
    name: str
    cls: str  # scenario class (acceptance table row)
    band: str
    paths: list[Path]
    excitation: str = "pink"
    snr_db: float | None = 30.0  # in-band SNR of meas (signal over noise), None = noiseless
    lm: int = 12000  # meas observation, samples
    search: tuple[int, int] = (-2400, 2400)  # signed absolute delay range, samples
    period: int | None = None  # periodic excitation period (samples)
    seed: int = 1
    fs: float = FS
    meas_start: int = 1_000_003
    level: float = 0.1  # excitation RMS (FS)
    ref_noise_db: float = -70.0  # loopback noise floor re excitation RMS
    noise: str = "pink"
    expect: str = "accept"  # accept | accept_or_ambiguous | ambiguous | refuse
    expect_reason: str = ""
    extra: dict = field(default_factory=dict)

    def truth(self) -> dict:
        g = max(p.gain_db for p in self.paths)
        sig = [p for p in self.paths if p.gain_db >= g + THRESHOLD_DB]
        first = min(sig, key=lambda p: p.delay)
        strongest = max(self.paths, key=lambda p: p.gain_db)
        return {"first": first.delay, "strongest": strongest.delay,
                "first_level_db": first.gain_db - g}


def synth(sc: Scenario):
    """Return ref, ref_start, meas, meas_start for a scenario."""
    fs = sc.fs
    rng = np.random.default_rng(sc.seed)
    lo, hi = sc.search
    ref_start = sc.meas_start + (-hi)  # ref covers meas +- search
    lr = sc.lm + (hi - lo)
    dmax = max(abs(p.delay) for p in sc.paths)
    if sc.period is None:
        # margin independent of the paths, so cases sharing a seed share the reference
        mg = max(abs(lo), abs(hi)) + 8192
        assert dmax <= mg - 4096, "path outside the synthesis margin"
        n_ext = lr + 2 * mg
        n_ext += n_ext % 2
        ext0 = ref_start - mg  # absolute index of x_ext[0]
        base = excitation(sc.excitation, n_ext, fs, rng)
        P = n_ext
    else:
        P = sc.period
        base = excitation(sc.excitation, P, fs, rng)
        ext0 = 0
    base = sc.level * base
    Xb = np.fft.rfft(base)
    k = np.arange(len(Xb))

    def shifted(d):
        # band-limited periodic shift: x(i - d), exact for the periodic extension of base
        return np.fft.irfft(Xb * np.exp(-2j * np.pi * k * d / P), P)

    def take(arr, start, n):
        idx = (np.arange(start, start + n) - ext0) % P
        return arr[idx]

    ref = take(base, ref_start, lr)
    y = np.zeros(sc.lm)
    for p in sc.paths:
        y += p.polarity * 10 ** (p.gain_db / 20.0) * take(shifted(p.delay), sc.meas_start, sc.lm)

    nrng = np.random.default_rng(sc.seed + 0x5EED)
    ref = ref + sc.level * 10 ** (sc.ref_noise_db / 20.0) * nrng.standard_normal(lr)
    if sc.snr_db is not None:
        if sc.noise == "pink":
            nz = _shape(nrng.standard_normal(sc.lm), fs, _pink_mag)
        else:
            nz = nrng.standard_normal(sc.lm)
        f = np.fft.rfftfreq(sc.lm, 1.0 / fs)
        B2 = band_weights(f, sc.band, fs) ** 2
        ps = np.sum(B2 * np.abs(np.fft.rfft(y)) ** 2)
        pn = np.sum(B2 * np.abs(np.fft.rfft(nz)) ** 2)
        if ps > 0:
            nz *= math.sqrt(ps / pn * 10 ** (-sc.snr_db / 10.0))
        else:
            nz *= sc.level / np.std(nz)
        y = y + nz
    return ref, ref_start, y, sc.meas_start


# --------------------------------------------------------------------------------------
# Golden sets
# --------------------------------------------------------------------------------------
#
# One set per (band, excitation): the cases of a set share one reference block (same
# excitation realisation, ref_start and coverage) and differ in the measurement block.
# Expected outcomes follow the rules of docs/design/q1-delay-finder.md; the truth is the
# analytic path list, never a finder output.

TOL = {"full": 1.0, "mid": 2.4, "sub": 4.8}  # samples at 48 kHz: 1 sample, 0.05 ms, 0.1 ms


@dataclass
class Case:
    name: str
    cls: str
    paths: list[Path]
    expect: str  # accepted | ambiguous | accepted_or_ambiguous | no_estimate
    snr_db: float | None = 30.0
    search: tuple[int, int] | None = None  # finder search; None = the set's coverage
    acceptable: list[float] | None = None  # first-arrival delays that count as correct
    span: tuple[float, float] | None = None  # accepted delay must lie inside (unresolved)
    reasons_any: list[str] = field(default_factory=list)  # for no_estimate
    note: str = ""


@dataclass
class GoldenSpec:
    name: str
    description: str
    band: str
    excitation: str
    lm: int
    coverage: int  # ref covers meas +- coverage samples
    seed: int
    cases: list[Case]
    period: int | None = None


P_ = Path
NARROW = (-2400, 2400)
SPECS = [
    GoldenSpec(
        "delay_finder_full_pink", "Full-range band (2-16 kHz), pink excitation: single and "
        "multipath arrivals, fractional, negative and large delays, low SNR, no signal.",
        "full", "pink", 12000, 48000, 0x0AC2_D001, [
            Case("single_frac", "single", [P_(137.37, 0.0)], "accepted", search=NARROW),
            Case("negative_frac", "single", [P_(-611.6, 0.0)], "accepted", search=NARROW),
            Case("refl_louder_inverted", "reflection_louder",
                 [P_(250.25, -6.0), P_(370.25, 0.0, -1)], "accepted", search=NARROW,
                 note="direct 6 dB below an inverted reflection 2.5 ms later"),
            Case("refl_louder_far", "reflection_louder",
                 [P_(-80.5, -10.0), P_(879.5, 0.0)], "accepted", search=NARROW,
                 note="direct 10 dB below a reflection 20 ms later"),
            Case("direct_buried", "direct_below_threshold",
                 [P_(400.0, -18.0), P_(544.0, 0.0)], "accepted", search=NARROW,
                 note="direct 18 dB down is not significant: first significant = reflection"),
            Case("borderline", "borderline",
                 [P_(300.0, -12.3), P_(500.0, 0.0)], "accepted_or_ambiguous",
                 search=NARROW, acceptable=[300.0, 500.0],
                 note="direct within 2 dB of the -12 dB threshold: either pick, or ambiguous"),
            Case("close_interfering", "unresolved",
                 [P_(-200.4, -2.0), P_(-195.4, 0.0, -1)], "accepted_or_ambiguous",
                 search=NARROW, span=(-201.4, -194.4),
                 note="0.1 ms apart (1.3 pulse widths): unresolved; accepted only inside the "
                      "cluster"),
            Case("room", "room",
                 [P_(1020.6, -3.0), P_(1164.6, -1.0, -1), P_(1404.6, -4.0), P_(1692.6, -6.0),
                  P_(1980.6, -9.0, -1), P_(2700.0, -12.0)], "accepted", snr_db=25.0,
                 search=(-2400, 4800), note="direct + 5 reflections 3-35 ms"),
            Case("large_negative", "single", [P_(-35040.3, 0.0), P_(-34740.3, -6.0)],
                 "accepted", search=(-48000, 48000),
                 note="-0.73 s inside the default +-1 s search range"),
            Case("low_snr", "noise", [P_(90.0, 0.0)], "no_estimate", snr_db=-15.0,
                 search=NARROW, reasons_any=["low_psr", "low_band_snr"]),
        ]),
    GoldenSpec(
        "delay_finder_full_music", "Full-range band, programme-like excitation (tonal, "
        "non-stationary, steep HF roll-off): direct below a louder reflection.",
        "full", "music", 12000, 2400, 0x0AC2_D002, [
            Case("music_refl_louder", "reflection_louder",
                 [P_(55.5, -6.0), P_(355.5, 0.0)], "accepted"),
        ]),
    GoldenSpec(
        "delay_finder_full_narrow", "Full-range band with excitation that only reaches "
        "1.2 kHz: the band is not excited, the finder must refuse.",
        "full", "narrow", 12000, 2400, 0x0AC2_D003, [
            Case("narrow_excitation", "excitation", [P_(100.0, 0.0)], "no_estimate",
                 reasons_any=["insufficient_excitation"]),
        ]),
    GoldenSpec(
        "delay_finder_full_periodic", "Periodic pink with a period (8192 samples) shorter "
        "than search width + tail allowance; a late reflection wraps to an apparent arrival "
        "before the direct sound. Must be refused, never accepted.",
        "full", "pink", 12000, 2400, 0x0AC2_D004, [
            Case("periodic_wrap", "periodic", [P_(480.0, 0.0), P_(8280.0, -4.0)],
                 "no_estimate", reasons_any=["periodic_excitation_too_short"],
                 note="reflection 7800 samples late appears at 8280 - 8192 = 88"),
        ], period=8192),
    GoldenSpec(
        "delay_finder_mid_pink", "Mid band (300 Hz - 3 kHz), pink excitation.",
        "mid", "pink", 24000, 4800, 0x0AC2_D005, [
            Case("mid_single_frac", "single", [P_(-1234.6, 0.0)], "accepted"),
            Case("mid_refl_louder_inverted", "reflection_louder",
                 [P_(700.3, -6.0), P_(940.3, 0.0, -1)], "accepted",
                 note="5 ms (12 pulse widths) apart"),
        ]),
    GoldenSpec(
        "delay_finder_sub_pink", "Sub band (20-120 Hz), pink excitation, 2 s observation: "
        "direct below a louder reflection 35 ms later (resolved: > 3 pulse widths).",
        "sub", "pink", 96000, 9600, 0x0AC2_D006, [
            Case("sub_refl_louder", "reflection_louder",
                 [P_(-600.3, -4.0), P_(1079.7, 0.0)], "accepted", snr_db=40.0),
        ]),
]

MEAS_START = 1_000_003


def scenario_for(spec: GoldenSpec, case: Case) -> Scenario:
    return Scenario(name=case.name, cls=case.cls, band=spec.band, paths=case.paths,
                    excitation=spec.excitation, snr_db=case.snr_db, lm=spec.lm,
                    search=(-spec.coverage, spec.coverage), period=spec.period,
                    seed=spec.seed, meas_start=MEAS_START)


def case_meta(spec: GoldenSpec, case: Case) -> dict:
    tr = scenario_for(spec, case).truth()
    search = case.search or (-spec.coverage, spec.coverage)
    return {
        "name": case.name, "class": case.cls, "expect": case.expect,
        "paths": [{"delay_samples": p.delay, "gain_db": p.gain_db, "polarity": p.polarity}
                  for p in case.paths],
        "snr_db": case.snr_db, "search": list(search),
        "first_delay": tr["first"], "strongest_delay": tr["strongest"],
        "acceptable_first_delays": case.acceptable or [tr["first"]],
        "span": list(case.span) if case.span else None,
        "reasons_any": case.reasons_any, "note": case.note,
    }


def _golden(spec: GoldenSpec):
    import generate as g

    def gen():
        ref0 = rs0 = None
        metas, arrays, scalars = [], [], {}
        for case in spec.cases:
            ref, ref_start, meas, meas_start = synth(scenario_for(spec, case))
            if ref0 is None:
                ref0, rs0 = ref, ref_start
            # every case of a set shares one reference realisation
            assert ref_start == rs0 and np.array_equal(ref, ref0), case.name
            m = case_meta(spec, case)
            metas.append(m)
            arrays.append((case.name, meas))
            pre = f"{case.name}."
            scalars |= {pre + "first_delay": m["first_delay"],
                        pre + "strongest_delay": m["strongest_delay"],
                        pre + "tol_samples": TOL[spec.band],
                        pre + "meas_start": float(meas_start),
                        pre + "search_min": float(m["search"][0]),
                        pre + "search_max": float(m["search"][1])}
        vs = g.VectorSet(
            name=spec.name,
            description=spec.description,
            function=f"tools/refgen/sets/delay_finder.py: _golden({spec.name})",
            parameters={
                "fs_hz": FS,
                "band": spec.band,
                "band_edges_hz": list(BAND_EDGES[spec.band]),
                "band_shape": ("-6 dB edges; raised cosine in log2(f), 1 octave wide, centred "
                               "on each edge; upper edge clipped to (fs/2) * 2^-0.5"),
                "excitation": spec.excitation,
                "excitation_period_samples": spec.period,
                "excitation_rms_fs": 0.1,
                "seed": spec.seed,
                "ref_start": float(rs0),
                "ref_coverage_samples": spec.coverage,
                "meas_length": spec.lm,
                "threshold_db": THRESHOLD_DB,
                "tolerance_samples": TOL[spec.band],
                "delay_sign": ("positive = measurement late: "
                               "meas[i] = sum_k g_k * ref_clean(i - d_k) + noise"),
                "construction": (
                    "x = excitation over a buffer covering the ref block and every path "
                    "(periodic extension when excitation_period_samples is set); each path is "
                    "an exact band-limited fractional shift of x (FFT phase ramp); ref = x + "
                    "white noise 70 dB below x; meas = sum of paths + pink noise scaled to "
                    "snr_db in the band (band weighting squared)"),
                "expect_semantics": {
                    "accepted": "status Accepted and |first.delay - an acceptable delay| <= tol",
                    "ambiguous": ("status Ambiguous and a listed candidate within tol of an "
                                  "acceptable delay"),
                    "accepted_or_ambiguous": (
                        "Ambiguous, or Accepted with first.delay within tol of an acceptable "
                        "delay (inside span instead, when span is given)"),
                    "no_estimate": "status NoEstimate with at least one of reasons_any",
                },
                "cases": metas,
            },
            references=["docs/design/q1-delay-finder.md (acceptance table)",
                        "PLAN.md 5.2 (delay finder), 5.4 (periodic pink)",
                        "docs/design/open-questions.md decisions 1a, 1c, 1d, 1e, 1f"],
            scalars=scalars,
        )
        vs.add("ref", ref0, unit="FS",
               description=(f"reference block starting at ref_start; covers meas_start - "
                            f"{spec.coverage} .. meas_start + meas_length + {spec.coverage}"))
        for name, meas in arrays:
            vs.add(f"meas.{name}", meas, unit="FS",
                   description=f"measurement block of case {name}, starting at {name}.meas_start")
        return vs

    gen.__name__ = f"gen_{spec.name}"
    return gen


GENERATORS: list = [_golden(s) for s in SPECS]
