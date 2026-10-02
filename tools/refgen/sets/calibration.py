"""Golden vectors for mic-curve correction (docs/design/q7-calibration.md).

An analog measurement-mic model (2nd-order Butterworth high-pass at 18 Hz, a +3 dB
resonance at 9 kHz, a 1st-order low-pass at 30 kHz) is sampled at 1/12-octave points:
that is the "file". Expected values are the model's exact analog magnitude, normalised at
the 1 kHz calibrator frequency, at IEC one-third-octave test frequencies, and its
log-frequency power average over each one-third-octave band. Interpolating the 1/12-octave
points and designing a filter from them must reproduce these within the tolerances given.
"""

import pathlib
import sys

import numpy as np
import scipy.integrate as integrate
import scipy.signal as sig

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import generate as g  # noqa: E402

G10 = 10.0 ** 0.3
F_NORM = 1000.0


def mic_model():
    """Analog transfer function (b, a) of the mic model, s in rad/s."""
    w_hp = 2 * np.pi * 18.0
    b_hp, a_hp = sig.butter(2, w_hp, btype="highpass", analog=True)
    # Peaking resonance (RBJ analog prototype): H(s) = (s² + s·(A/Q)·w0 + w0²) /
    # (s² + s/(A·Q)·w0 + w0²), A = 10^(gain/40).
    w0 = 2 * np.pi * 9000.0
    a_lin = 10.0 ** (3.0 / 40.0)
    q = 1.2
    b_pk = [1.0, a_lin / q * w0, w0 * w0]
    a_pk = [1.0, w0 / (a_lin * q), w0 * w0]
    w_lp = 2 * np.pi * 30000.0
    b_lp, a_lp = [w_lp], [1.0, w_lp]
    b = np.polymul(np.polymul(b_hp, b_pk), b_lp)
    a = np.polymul(np.polymul(a_hp, a_pk), a_lp)
    return b, a


def mag_db(b, a, f):
    _, h = sig.freqs(b, a, worN=2 * np.pi * np.atleast_1d(np.asarray(f, dtype=float)))
    return 20.0 * np.log10(np.abs(h))


def iec_third_centres(f_lo, f_hi):
    x = np.arange(-30, 14)
    f = 1000.0 * G10 ** (x / 3.0)
    return f[(f >= f_lo * 0.999) & (f <= f_hi * 1.001)]


def gen_calibration_mic_curve() -> g.VectorSet:
    b, a = mic_model()
    k = np.arange(-80, 56)  # 1/12-octave points around 1 kHz: 9.8 Hz … 17.96 kHz …
    curve_f = 1000.0 * 2.0 ** (k / 12.0)
    curve_f = curve_f[(curve_f >= 10.0) & (curve_f <= 25000.0)]
    curve_db = mag_db(b, a, curve_f)
    ref = mag_db(b, a, F_NORM)[0]

    test_f = iec_third_centres(20.0, 20000.0)
    subtract = mag_db(b, a, test_f) - ref

    def band(fm):
        lo, hi = fm / G10 ** (1 / 6), fm * G10 ** (1 / 6)

        def integrand(u):
            return 10.0 ** (-(mag_db(b, a, np.exp(u))[0] - ref) / 10.0)

        val, _ = integrate.quad(integrand, np.log(lo), np.log(hi), epsabs=0, epsrel=1e-12)
        return -10.0 * np.log10(val / (np.log(hi) - np.log(lo)))

    band_subtract = np.array([band(fm) for fm in test_f])

    vs = g.VectorSet(
        name="calibration_mic_curve",
        description=(
            "Analog mic model sampled at 1/12-octave points (the curve file); exact "
            "normalised correction at IEC 1/3-octave centres and its log-f power average "
            "over each 1/3-octave band"
        ),
        function="gen_calibration_mic_curve",
        parameters={
            "model": "butter(2, 18 Hz, highpass) · RBJ analog peaking (9 kHz, +3 dB, Q 1.2) "
                     "· 1st-order low-pass 30 kHz",
            "curve_points": "1000·2^(k/12) Hz within 10 Hz … 25 kHz",
            "f_norm_hz": F_NORM,
            "band_average": "−10·lg(mean over ln f of 10^(−c_n/10)), ideal base-10 1/3-oct edges",
        },
        references=[
            "docs/design/q7-calibration.md §5, §6",
            "scipy.signal.freqs, scipy.integrate.quad",
            "IEC 61260-1 base-10 mid-band frequencies",
        ],
        scalars={"f_norm_hz": F_NORM, "ref_db": float(ref)},
    )
    vs.add("curve_freq_hz", curve_f, unit="Hz", description="curve file frequencies")
    vs.add("curve_gain_db", curve_db, unit="dB", description="mic response at the points",
           axis="curve_freq_hz")
    vs.add("test_freq_hz", test_f, unit="Hz", description="IEC 1/3-octave centres 20 Hz–20 kHz")
    vs.add(
        "subtract_db",
        subtract,
        unit="dB",
        description="exact c_n(f) = G(f) − G(1 kHz): subtracted from displayed magnitude",
        tolerance=g.lin_tol(0.03, 0.0),
        axis="test_freq_hz",
    )
    vs.add(
        "band_subtract_db",
        band_subtract,
        unit="dB",
        description="log-f power average of c_n over each ideal 1/3-octave band",
        tolerance=g.lin_tol(0.03, 0.0),
        axis="test_freq_hz",
    )
    return vs


GENERATORS = [gen_calibration_mic_curve]
