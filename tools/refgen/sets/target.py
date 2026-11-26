"""Generated target curves (ac2_core::target): a house tilt with a cookbook low shelf, and
the ISO 226:2023 normal equal-loudness-level contours with the perceived-balance shift
between two loudness levels.

The tilt is the closed form; the shelf is the RBJ Audio EQ Cookbook analog low shelf
(S = 1) written out and evaluated with scipy.signal.freqs. The contours are ISO 226:2023
Formula (1) with the parameters of its Table 1 (alpha_r = 0.3 and T_r = 2.4 dB at 1 kHz),
as Suzuki, Takeshima & Kurakata, Acoust. Sci. & Tech. 45(1) (2024) section 2.3 also print
the formula (open access).
"""

import pathlib
import sys

import numpy as np
import scipy.signal as sig

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import generate as g  # noqa: E402

ISO_F = np.array([
    20, 25, 31.5, 40, 50, 63, 80, 100, 125, 160, 200, 250, 315, 400, 500, 630, 800, 1000,
    1250, 1600, 2000, 2500, 3150, 4000, 5000, 6300, 8000, 10000, 12500,
], dtype=float)
ALPHA = np.array([
    0.635, 0.602, 0.569, 0.537, 0.509, 0.482, 0.456, 0.433, 0.412, 0.391, 0.373, 0.357, 0.343,
    0.330, 0.320, 0.311, 0.303, 0.300, 0.295, 0.292, 0.290, 0.290, 0.289, 0.289, 0.289, 0.293,
    0.303, 0.323, 0.354,
])
LU = np.array([
    -31.5, -27.2, -23.1, -19.3, -16.1, -13.1, -10.4, -8.2, -6.3, -4.6, -3.2, -2.1, -1.2, -0.5,
    0.0, 0.4, 0.5, 0.0, -2.7, -4.2, -1.2, 1.4, 2.3, 1.0, -2.3, -7.2, -11.2, -10.9, -3.5,
])
TF = np.array([
    78.1, 68.7, 59.5, 51.1, 44.0, 37.5, 31.5, 26.5, 22.1, 17.9, 14.4, 11.4, 8.6, 6.2, 4.4, 3.0,
    2.2, 2.4, 3.5, 1.7, -1.3, -4.2, -6.0, -5.4, -1.5, 6.0, 12.6, 13.9, 12.3,
])
REF = int(np.flatnonzero(ISO_F == 1000.0)[0])

PHONS = [20, 40, 60, 80, 85, 90, 100]
SHIFTS = [(85, 100), (100, 85), (60, 90)]
TILTS = [  # name, slope dB/oct, pivot Hz, shelf gain dB (0: none), shelf corner Hz
    ("tilt_m1_1k", -1.0, 1000.0, 0.0, 0.0),
    ("tilt_m1_1k_shelf_p6_100", -1.0, 1000.0, 6.0, 100.0),
    ("tilt_p05_200_shelf_m4_60", 0.5, 200.0, -4.0, 60.0),
]


def contour(phon: float) -> np.ndarray:
    # Formula (1) in its first form: (p0/pa)^(2 (alpha_r - alpha_f)) with p0 = 20 uPa, pa = 1 Pa.
    a_r, t_r = 0.3, 2.4
    k = (20e-6 / 1.0) ** (2 * (a_r - ALPHA))
    inner = k * (10 ** (a_r * phon / 10) - 10 ** (a_r * t_r / 10)) + 10 ** (ALPHA * (TF + LU) / 10)
    return 10 / ALPHA * np.log10(inner) - LU


def shift(a: float, b: float) -> np.ndarray:
    ca, cb = contour(a), contour(b)
    return (ca - ca[REF]) - (cb - cb[REF])


def low_shelf_db(f: np.ndarray, gain_db: float, f0: float) -> np.ndarray:
    """RBJ cookbook analog low shelf, S = 1 (Q = 1/sqrt(2)), half the dB gain at f0."""
    a = 10 ** (gain_db / 40)
    q = 1 / np.sqrt((a + 1 / a) * (1 / 1.0 - 1) + 2)
    b = [a, a * np.sqrt(a) / q, a * a]
    den = [a, np.sqrt(a) / q, 1]
    _, h = sig.freqs(b, den, worN=f / f0)
    return 20 * np.log10(np.abs(h))


def gen_target_curves() -> g.VectorSet:
    tilt_f = 1000.0 * 2.0 ** (np.arange(-72, 61) / 12)  # 1/12 octave, 15.6 Hz … 32 kHz
    vs = g.VectorSet(
        name="target_curves",
        description=(
            "Generated target curves: house tilts with an optional cookbook low shelf on a "
            "1/12-octave grid, ISO 226:2023 equal-loudness-level contours at the standard's 29 "
            "frequencies, and the perceived-balance shift between two loudness levels."
        ),
        function="gen_target_curves",
        parameters={
            "tilt": "slope * log2(f / pivot) dB",
            "shelf": "RBJ cookbook analog low shelf, S = 1, f0 = half-gain frequency, scipy.signal.freqs",
            "contour": "Lf = 10/af lg((p0/pa)^(2 (ar - af)) (10^(ar LN/10) - 10^(ar Tr/10)) + 10^(af (Tf + LU)/10)) - LU; ar = 0.3, Tr = 2.4 dB, p0 = 20 uPa, pa = 1 Pa",
            "shift": "(C_from - C_from(1 kHz)) - (C_to - C_to(1 kHz))",
            "tilts": {n: [s, p, gdb, fc] for n, s, p, gdb, fc in TILTS},
        },
        references=[
            "Suzuki, Takeshima, Kurakata, Acoust. Sci. & Tech. 45(1) 1-8 (2024), doi:10.1250/ast.e23.66, sec. 2.3",
            "ISO 226:2023, Formula (1) and Table 1",
            "R. Bristow-Johnson, Audio EQ Cookbook",
        ],
    )
    vs.add("tilt_freq_hz", tilt_f, unit="Hz", description="tilt evaluation frequencies",
           tolerance=g.lin_tol(0.0, 1e-12))
    for name, slope, pivot, gdb, fc in TILTS:
        y = slope * np.log2(tilt_f / pivot)
        if gdb != 0.0:
            y = y + low_shelf_db(tilt_f, gdb, fc)
        vs.add(name, y, unit="dB", description=f"tilt {slope} dB/oct @ {pivot} Hz, shelf {gdb} dB @ {fc} Hz",
               tolerance=g.lin_tol(1e-9, 1e-9), axis="tilt_freq_hz")
    vs.add("iso_freq_hz", ISO_F, unit="Hz", description="ISO 226 frequencies",
           tolerance=g.lin_tol(0.0, 0.0))
    for p in PHONS:
        vs.add(f"contour_{p}", contour(p), unit="dB", description=f"{p}-phon contour, dB SPL",
               tolerance=g.lin_tol(1e-9, 1e-12), axis="iso_freq_hz")
    for a, b in SHIFTS:
        vs.add(f"shift_{a}_{b}", shift(a, b), unit="dB", description=f"loudness shift {a} -> {b} phon",
               tolerance=g.lin_tol(1e-9, 1e-12), axis="iso_freq_hz")
    return vs


GENERATORS = [gen_target_curves]
