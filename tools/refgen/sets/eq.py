"""EQ what-if sections (docs/design/eq-overlay.md): scipy filter design and the cookbook
formulas written out, independently of ac2_core::eq."""

from __future__ import annotations

import pathlib
import sys

import numpy as np
import scipy.optimize as opt
import scipy.signal as sig

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import generate as g  # noqa: E402

FS = 48000.0


def grid() -> np.ndarray:
    """1/12-octave grid 10 Hz … 20 kHz anchored at 1 kHz (below fs/2: no gaps)."""
    k = np.arange(int(np.floor(12 * np.log2(10 / 1000))), int(np.ceil(12 * np.log2(20000 / 1000))) + 1)
    return 1000.0 * 2.0 ** (k / 12)


def analog(b, a, f):
    _, h = sig.freqs(b, a, worN=2 * np.pi * f)
    return h


def digital(b, a, f):
    _, h = sig.freqz(b, a, worN=f, fs=FS)
    return h


def zpk_analog(z, p, k, f):
    _, h = sig.freqs_zpk(z, p, k, worN=2 * np.pi * f)
    return h


def zpk_digital(z, p, k, f):
    _, h = sig.freqz_zpk(z, p, k, worN=f, fs=FS)
    return h


def cookbook(kind: str, f0: float, gain_db: float, q: float):
    """RBJ Audio EQ Cookbook biquads (b, a), written out."""
    a_ = 10 ** (gain_db / 40)
    w0 = 2 * np.pi * f0 / FS
    al = np.sin(w0) / (2 * q)
    c = np.cos(w0)
    if kind == "peaking":
        b = [1 + al * a_, -2 * c, 1 - al * a_]
        a = [1 + al / a_, -2 * c, 1 - al / a_]
    elif kind == "low_shelf":
        r = 2 * np.sqrt(a_) * al
        b = [a_ * ((a_ + 1) - (a_ - 1) * c + r), 2 * a_ * ((a_ - 1) - (a_ + 1) * c),
             a_ * ((a_ + 1) - (a_ - 1) * c - r)]
        a = [(a_ + 1) + (a_ - 1) * c + r, -2 * ((a_ - 1) + (a_ + 1) * c),
             (a_ + 1) + (a_ - 1) * c - r]
    elif kind == "high_shelf":
        r = 2 * np.sqrt(a_) * al
        b = [a_ * ((a_ + 1) + (a_ - 1) * c + r), -2 * a_ * ((a_ - 1) + (a_ + 1) * c),
             a_ * ((a_ + 1) + (a_ - 1) * c - r)]
        a = [(a_ + 1) - (a_ - 1) * c + r, 2 * ((a_ - 1) - (a_ + 1) * c),
             (a_ + 1) - (a_ - 1) * c - r]
    elif kind == "notch":
        b = [1, -2 * c, 1]
        a = [1 + al, -2 * c, 1 - al]
    elif kind == "band_pass":
        b = [al, 0, -al]
        a = [1 + al, -2 * c, 1 - al]
    elif kind == "all_pass":
        b = [1 - al, -2 * c, 1 + al]
        a = [1 + al, -2 * c, 1 - al]
    else:
        raise ValueError(kind)
    return np.array(b), np.array(a)


def minus3db_symmetric_q(q3: float, gain_db: float) -> float:
    """Cookbook Q whose analog response has f0/q3 between the points 3 dB from the peak,
    found by root-finding on the response itself (not the closed form)."""
    a = 10 ** (gain_db / 40)

    def width(q):
        b, den = [1, a / q, 1], [1, 1 / (a * q), 1]
        target = abs(gain_db) - 10 * np.log10(2)

        def dev(w):
            _, h = sig.freqs(b, den, worN=[w])
            return abs(20 * np.log10(abs(h[0]))) - target

        lo = opt.brentq(dev, 1e-4, 1.0, xtol=1e-15)
        hi = opt.brentq(dev, 1.0, 1e4, xtol=1e-15)
        return hi - lo

    return opt.brentq(lambda q: width(q) - 1 / q3, 1e-3, 100, xtol=1e-15)


def gen_eq_sections() -> g.VectorSet:
    f = grid()
    vs = g.VectorSet(
        name="eq_sections",
        description=(
            "EQ sections on a 1/12-octave grid: scipy butter/bessel (analog and digital, "
            "prewarped at Wn), Linkwitz-Riley as squared Butterworth, cookbook biquads at "
            "48 kHz, and a peaking Q read 3 dB from the peak solved by root-finding."
        ),
        function="gen_eq_sections",
        parameters={
            "fs_hz": FS,
            "grid": "f = 1000 * 2^(k/12), 10 Hz … 20 kHz",
            "analog": "scipy.signal.freqs at w = 2 pi f",
            "digital": "scipy.signal.freqz at fs (butter/bessel digital: bilinear prewarped at Wn)",
            "cookbook": "RBJ Audio EQ Cookbook coefficients written out",
            "minus3db": "cookbook Q found by brentq so the 3 dB-from-peak band is f0/Q3",
            "cases": {
                "bw_hp4_80_analog": "butter(4, 80 Hz, high, analog)",
                "bw_lp3_2k_digital": "butter(3, 2 kHz, low, fs)",
                "lr4_lp_1k_digital": "butter(2, 1 kHz, low, fs) squared",
                "bessel5_hp_100_<norm>_analog": "bessel(5, 100 Hz, high, analog, norm)",
                "bessel4_lp_3k_phase_digital": "bessel(4, 3 kHz, low, fs, norm=phase)",
                "peak_1k_m6_q2": "cookbook peaking 1 kHz -6 dB Q 2",
                "peak_8k_p9_q07": "cookbook peaking 8 kHz +9 dB Q 0.7",
                "ls_120_p6_s1": "cookbook low shelf 120 Hz +6 dB S 1 (Q 0.7071)",
                "hs_6k_m4_q09": "cookbook high shelf 6 kHz -4 dB Q 0.9",
                "notch_60_q8": "cookbook notch 60 Hz Q 8",
                "bp_500_q14": "cookbook band-pass (0 dB peak) 500 Hz Q 1.4",
                "ap2_1k_q07": "cookbook all-pass 1 kHz Q 0.7071",
                "peak3db_2k_p12_q3_analog": "symmetric peaking 2 kHz +12 dB, Q read 3 dB from peak = 3",
            },
        },
        references=[
            "docs/design/eq-overlay.md 1, 5",
            "scipy.signal.butter / bessel / freqs / freqz / freqs_zpk",
            "R. Bristow-Johnson, Audio EQ Cookbook",
        ],
        scalars={"fs_hz": FS, "peak3db_cookbook_q": minus3db_symmetric_q(3.0, 12.0)},
    )
    tol = g.lin_tol(1e-9, 1e-9)
    vs.add("freq_hz", f, unit="Hz", description="evaluation frequencies", tolerance=g.lin_tol(0.0, 1e-12))

    def add(name, h, desc):
        vs.add(name, h, unit="1", description=desc, tolerance=tol, axis="freq_hz")

    add("bw_hp4_80_analog", zpk_analog(*sig.butter(4, 2 * np.pi * 80, "high", analog=True, output="zpk"), f),
        "Butterworth HP 4th order 80 Hz, analog")
    add("bw_lp3_2k_digital", zpk_digital(*sig.butter(3, 2000, "low", fs=FS, output="zpk"), f),
        "Butterworth LP 3rd order 2 kHz, 48 kHz")
    h2 = zpk_digital(*sig.butter(2, 1000, "low", fs=FS, output="zpk"), f)
    add("lr4_lp_1k_digital", h2 * h2, "Linkwitz-Riley LP 4th order 1 kHz, 48 kHz")
    for norm in ["phase", "delay", "mag"]:
        z, p, k = sig.bessel(5, 2 * np.pi * 100, "high", analog=True, norm=norm, output="zpk")
        add(f"bessel5_hp_100_{norm}_analog", zpk_analog(z, p, k, f), f"Bessel HP 5 100 Hz norm={norm}, analog")
    add("bessel4_lp_3k_phase_digital",
        zpk_digital(*sig.bessel(4, 3000, "low", fs=FS, norm="phase", output="zpk"), f),
        "Bessel LP 4 3 kHz norm=phase, 48 kHz")
    for name, kind, f0, gdb, q in [
        ("peak_1k_m6_q2", "peaking", 1000, -6, 2),
        ("peak_8k_p9_q07", "peaking", 8000, 9, 0.7),
        ("ls_120_p6_s1", "low_shelf", 120, 6, 1 / np.sqrt(2)),
        ("hs_6k_m4_q09", "high_shelf", 6000, -4, 0.9),
        ("notch_60_q8", "notch", 60, 0, 8),
        ("bp_500_q14", "band_pass", 500, 0, 1.4),
        ("ap2_1k_q07", "all_pass", 1000, 0, 1 / np.sqrt(2)),
    ]:
        b, a = cookbook(kind, f0, gdb, q)
        add(name, digital(b, a, f), f"cookbook {kind} {f0} Hz {gdb} dB Q {q}, 48 kHz")
    qc = minus3db_symmetric_q(3.0, 12.0)
    a_ = 10 ** (12 / 40)
    w = f / 2000
    add("peak3db_2k_p12_q3_analog", sig.freqs([1, a_ / qc, 1], [1, 1 / (a_ * qc), 1], worN=w)[1],
        "symmetric peaking 2 kHz +12 dB with Q 3 read 3 dB from the peak, analog")
    return vs


GENERATORS = [gen_eq_sections]
