#!/usr/bin/env python3
"""Golden-vector generator for ac2.

Writes reference data computed with numpy/scipy (and analytic formulas) into
``fixtures/golden/<name>.json`` + ``fixtures/golden/<name>.bin``. The format is
documented in ``tools/refgen/README.md`` and read by ``crates/ac2-testkit``.

Usage:
    python generate.py              # (re)write fixtures/golden
    python generate.py --check      # regenerate into a temp dir, fail on any difference
    python generate.py --out DIR    # write somewhere else

Everything is deterministic: fixed seeds, fixed parameters, no timestamps.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np
import scipy
import scipy.signal as sig

FORMAT = "ac2-golden"
FORMAT_VERSION = 1
SCRIPT = "tools/refgen/generate.py"

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUT = REPO_ROOT / "fixtures" / "golden"


# --------------------------------------------------------------------------------------
# Container
# --------------------------------------------------------------------------------------


def lin_tol(abs_: float, rel: float) -> dict:
    """Pass if |actual - expected| <= abs + rel * |expected| (element-wise)."""
    return {"kind": "linear", "abs": abs_, "rel": rel}


def db_tol(db: float, floor_db: float) -> dict:
    """Compare in dB; values below floor_db (in both) are treated as equal to the floor."""
    return {"kind": "db", "db": db, "floor_db": floor_db}


@dataclass
class VectorSet:
    name: str
    description: str
    function: str
    parameters: dict
    references: list[str] = field(default_factory=list)
    scalars: dict = field(default_factory=dict)
    arrays: list[tuple[str, np.ndarray, dict]] = field(default_factory=list)

    def add(
        self,
        name: str,
        data: np.ndarray,
        *,
        unit: str,
        description: str,
        tolerance: dict | None = None,
        axis: str | None = None,
    ) -> None:
        data = np.asarray(data)
        if np.iscomplexobj(data):
            data = data.astype(np.complex128)
        else:
            data = data.astype(np.float64)
        if not np.all(np.isfinite(data)):
            raise ValueError(f"{self.name}/{name}: non-finite values")
        meta = {"unit": unit, "description": description}
        if tolerance is not None:
            meta["tolerance"] = tolerance
        if axis is not None:
            meta["axis"] = axis
        self.arrays.append((name, data, meta))

    def write(self, out_dir: Path) -> None:
        blob = bytearray()
        arrays_meta = []
        names = set()
        for name, data, meta in self.arrays:
            if name in names:
                raise ValueError(f"{self.name}: duplicate array {name}")
            names.add(name)
            if np.iscomplexobj(data):
                dtype = "c128"
                # complex128 is stored as interleaved (re, im) little-endian f64 pairs.
                raw = np.ascontiguousarray(data).view(np.float64).astype("<f8").tobytes()
            else:
                dtype = "f64"
                raw = np.ascontiguousarray(data).astype("<f8").tobytes()
            if "axis" in meta and meta["axis"] not in names:
                raise ValueError(f"{self.name}/{name}: axis {meta['axis']} must come first")
            entry = {
                "name": name,
                "dtype": dtype,
                "shape": list(data.shape),
                "offset": len(blob),
                "nbytes": len(raw),
            }
            entry.update(meta)
            arrays_meta.append(entry)
            blob += raw
        meta = {
            "format": FORMAT,
            "format_version": FORMAT_VERSION,
            "name": self.name,
            "description": self.description,
            "generator": {"script": SCRIPT, "function": self.function},
            "versions": {"numpy": np.__version__, "scipy": scipy.__version__},
            "parameters": self.parameters,
            "references": self.references,
            "scalars": self.scalars,
            "blob": {
                "file": f"{self.name}.bin",
                "nbytes": len(blob),
                "sha256": hashlib.sha256(blob).hexdigest(),
            },
            "arrays": arrays_meta,
        }
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / f"{self.name}.bin").write_bytes(bytes(blob))
        text = json.dumps(meta, indent=2, ensure_ascii=False, allow_nan=False) + "\n"
        (out_dir / f"{self.name}.json").write_text(text, encoding="utf-8")


# --------------------------------------------------------------------------------------
# Shared conventions
# --------------------------------------------------------------------------------------


def periodic_hann(n: int) -> np.ndarray:
    """Periodic Hann: w[i] = 0.5 - 0.5 cos(2 pi i / N), i = 0..N-1 (DFT-even)."""
    w = sig.get_window("hann", n, fftbins=True)
    i = np.arange(n)
    explicit = 0.5 - 0.5 * np.cos(2.0 * np.pi * i / n)
    assert np.allclose(w, explicit, rtol=0, atol=1e-15)
    return w


def one_sided_factor(n_fft: int) -> np.ndarray:
    """Factor folding negative frequencies into the one-sided spectrum.

    Interior bins are doubled; DC and (even N) Nyquist have no mirror image and are not.
    """
    f = np.full(n_fft // 2 + 1, 2.0)
    f[0] = 1.0
    if n_fft % 2 == 0:
        f[-1] = 1.0
    return f


# --------------------------------------------------------------------------------------
# (a) Amplitude spectrum and PSD
# --------------------------------------------------------------------------------------


def gen_spectrum_hann_tone_noise() -> VectorSet:
    fs = 48000.0
    n = 4096
    tone_bin = 256  # 3000 Hz, exactly bin-centred
    tone_freq = tone_bin * fs / n
    tone_peak = 0.5  # -6.02 dBFS (0 dBFS = full-scale sine)
    noise_rms = 1.0e-3
    seed = 0x0AC2_0001

    rng = np.random.default_rng(seed)
    t = np.arange(n) / fs
    x = tone_peak * np.sin(2.0 * np.pi * tone_freq * t) + noise_rms * rng.standard_normal(n)

    w = periodic_hann(n)
    s1 = float(np.sum(w))  # coherent gain * N
    s2 = float(np.sum(w * w))  # energy gain * N
    spec = np.fft.rfft(w * x)
    fold = one_sided_factor(n)
    freqs = np.fft.rfftfreq(n, 1.0 / fs)

    # Amplitude spectrum in FS RMS: a bin-centred sine of peak a reads a/sqrt(2).
    # |X[k]| = a*S1/2 for that sine, so RMS = |X| * sqrt(2) / S1 = sqrt(fold) * |X| / S1.
    # DC and Nyquist: a constant (or alternating) signal of value c gives |X| = c*S1 and
    # its RMS is c, so fold = 1 there.
    amp_rms = np.sqrt(fold) * np.abs(spec) / s1
    # PSD, one-sided, FS^2/Hz: fold * |X|^2 / (fs * S2).
    psd = fold * np.abs(spec) ** 2 / (fs * s2)

    # Independent route through scipy (same conventions: periodic window, DC/Nyquist
    # not doubled, 'spectrum' scaling normalised by S1^2, 'density' by fs*S2).
    f_sp, p_spec = sig.periodogram(x, fs, window="hann", nfft=n, detrend=False,
                                   return_onesided=True, scaling="spectrum")
    f_sd, p_dens = sig.periodogram(x, fs, window="hann", nfft=n, detrend=False,
                                   return_onesided=True, scaling="density")
    assert np.array_equal(f_sp, freqs) and np.array_equal(f_sd, freqs)
    assert np.allclose(np.sqrt(p_spec), amp_rms, rtol=1e-9, atol=1e-15)
    assert np.allclose(p_dens, psd, rtol=1e-9, atol=1e-18)
    assert abs(amp_rms[tone_bin] - tone_peak / np.sqrt(2.0)) < 1e-4

    psd_db = 10.0 * np.log10(psd)
    amp_dbfs = 20.0 * np.log10(amp_rms * np.sqrt(2.0))

    vs = VectorSet(
        name="spectrum_hann_tone_noise",
        description=(
            "Single-frame Hann-windowed one-sided amplitude spectrum (FS RMS) and PSD "
            "(FS^2/Hz) of a bin-centred sine plus white Gaussian noise."
        ),
        function="gen_spectrum_hann_tone_noise",
        parameters={
            "fs_hz": fs,
            "n": n,
            "window": "hann, periodic (w[i] = 0.5 - 0.5 cos(2 pi i / N))",
            "tone_bin": tone_bin,
            "tone_freq_hz": tone_freq,
            "tone_peak_fs": tone_peak,
            "tone_phase": "sin, zero phase at sample 0",
            "noise_rms_fs": noise_rms,
            "noise": "numpy default_rng(seed).standard_normal(n) * noise_rms_fs",
            "seed": seed,
            "amplitude_formula": "amp_rms[k] = sqrt(c_k) * |X[k]| / sum(w)",
            "psd_formula": "psd[k] = c_k * |X[k]|^2 / (fs * sum(w^2))",
            "one_sided_c_k": "2 for 0 < k < N/2, 1 for k = 0 and k = N/2",
            "dbfs_formula": "amp_dbfs = 20 log10(amp_rms * sqrt(2)) (0 dBFS = full-scale sine)",
            "psd_db_formula": "psd_db = 10 log10(psd), dB re 1 FS^2/Hz",
            "dft": "X[k] = sum_n w[n] x[n] exp(-2 pi i k n / N) (numpy.fft.rfft)",
        },
        references=[
            "PLAN.md 5.3 (units per display)",
            "docs/design/open-questions.md Q4 / decision 4a",
            "scipy.signal.periodogram cross-check (scaling='spectrum' and 'density')",
        ],
        scalars={
            "window_sum": s1,
            "window_sum_sq": s2,
            "coherent_gain": s1 / n,
            "enbw_bins": n * s2 / s1**2,
            "tone_rms_expected_fs": tone_peak / np.sqrt(2.0),
        },
    )
    vs.add("x", x, unit="FS", description="input signal, n samples")
    vs.add("window", w, unit="1", description="periodic Hann window",
           tolerance=lin_tol(1e-15, 0.0))
    vs.add("freq_hz", freqs, unit="Hz", description="bin centre frequencies k*fs/N",
           tolerance=lin_tol(0.0, 1e-15))
    vs.add("amplitude_rms", amp_rms, unit="FS RMS",
           description="one-sided amplitude spectrum, sine RMS at bin centre",
           tolerance=lin_tol(1e-13, 1e-9), axis="freq_hz")
    vs.add("amplitude_dbfs", amp_dbfs, unit="dBFS",
           description="20 log10(amplitude_rms * sqrt 2)",
           tolerance=db_tol(1e-6, -300.0), axis="freq_hz")
    vs.add("psd", psd, unit="FS^2/Hz", description="one-sided PSD",
           tolerance=lin_tol(1e-15, 1e-9), axis="freq_hz")
    vs.add("psd_db", psd_db, unit="dB re 1 FS^2/Hz", description="10 log10(psd)",
           tolerance=db_tol(1e-6, -400.0), axis="freq_hz")
    return vs


# --------------------------------------------------------------------------------------
# (b) H1 transfer function and coherence
# --------------------------------------------------------------------------------------


def gen_transfer_h1_biquad_delay() -> VectorSet:
    fs = 48000.0
    n = 16384
    nperseg = 512
    noverlap = 256
    delay = 24  # samples, y late
    f0, q, gain_db = 1000.0, 2.0, 6.0
    noise_rms = 0.3  # uncorrelated output noise -> partial coherence
    seed = 0x0AC2_0002

    # RBJ audio-EQ-cookbook peaking EQ.
    a_lin = 10.0 ** (gain_db / 40.0)
    w0 = 2.0 * np.pi * f0 / fs
    alpha = np.sin(w0) / (2.0 * q)
    b = np.array([1.0 + alpha * a_lin, -2.0 * np.cos(w0), 1.0 - alpha * a_lin])
    a = np.array([1.0 + alpha / a_lin, -2.0 * np.cos(w0), 1.0 - alpha / a_lin])
    b, a = b / a[0], a / a[0]

    rng = np.random.default_rng(seed)
    # Generate extra leading samples so the delayed, filtered output has no start-up
    # transient or zero-fill inside the analysed window.
    lead = 4096
    xx = rng.standard_normal(n + lead)
    yy = sig.lfilter(b, a, xx)
    x = xx[lead:]
    y = yy[lead - delay : lead - delay + n] + noise_rms * rng.standard_normal(n)

    kw = dict(fs=fs, window="hann", nperseg=nperseg, noverlap=noverlap, detrend=False,
              return_onesided=True, scaling="density", average="mean")
    freqs, pxx = sig.welch(x, **kw)
    _, pyy = sig.welch(y, **kw)
    _, pxy = sig.csd(x, y, **kw)  # scipy: Pxy = conj(X) * Y, averaged
    _, coh = sig.coherence(x, y, fs=fs, window="hann", nperseg=nperseg, noverlap=noverlap,
                           detrend=False)
    h1 = pxy / pxx
    coh_manual = np.abs(pxy) ** 2 / (pxx * pyy)
    assert np.allclose(coh, coh_manual, rtol=1e-12, atol=0)

    _, h_biquad = sig.freqz(b, a, worN=freqs, fs=fs)
    h_true = h_biquad * np.exp(-2j * np.pi * freqs * delay / fs)

    vs = VectorSet(
        name="transfer_h1_biquad_delay",
        description=(
            "Welch-averaged H1 = Pxy/Pxx and magnitude-squared coherence for white noise "
            "through an RBJ peaking biquad, an integer delay and additive uncorrelated noise."
        ),
        function="gen_transfer_h1_biquad_delay",
        parameters={
            "fs_hz": fs,
            "n": n,
            "nperseg": nperseg,
            "noverlap": noverlap,
            "window": "hann, periodic (scipy get_window default)",
            "detrend": False,
            "average": "mean",
            "delay_samples": delay,
            "delay_sign": "positive = y (measurement) late relative to x (reference)",
            "biquad": f"RBJ peaking EQ f0={f0} Hz Q={q} gain={gain_db} dB",
            "biquad_b": b.tolist(),
            "biquad_a": a.tolist(),
            "x": "default_rng(seed).standard_normal(n + lead)[lead:], lead=4096",
            "y": "lfilter(b, a, x_full) delayed by delay_samples + noise_rms * N(0,1)",
            "noise_rms": noise_rms,
            "seed": seed,
            "csd_convention": "Pxy = mean(conj(X) * Y) (scipy.signal.csd)",
            "h1_formula": "H1 = Pxy / Pxx",
            "coherence_formula": "|Pxy|^2 / (Pxx * Pyy)",
        },
        references=[
            "PLAN.md 5.1 (H1 = Gxy/Gxx, gamma^2 = |Gxy|^2/(Gxx Gyy))",
            "PLAN.md 5.6 (loopback with known filter, delay and uncorrelated noise)",
            "R. Bristow-Johnson, Audio EQ Cookbook (peaking EQ)",
        ],
        scalars={"n_segments": int((n - noverlap) // (nperseg - noverlap))},
    )
    vs.add("x", x, unit="FS", description="reference (input) signal")
    vs.add("y", y, unit="FS", description="measurement (output) signal")
    vs.add("freq_hz", freqs, unit="Hz", description="bin frequencies",
           tolerance=lin_tol(0.0, 1e-15))
    vs.add("pxx", pxx, unit="FS^2/Hz", description="Welch PSD of x",
           tolerance=lin_tol(1e-18, 1e-9), axis="freq_hz")
    vs.add("pyy", pyy, unit="FS^2/Hz", description="Welch PSD of y",
           tolerance=lin_tol(1e-18, 1e-9), axis="freq_hz")
    vs.add("pxy", pxy, unit="FS^2/Hz", description="Welch CSD conj(X)*Y",
           tolerance=lin_tol(1e-18, 1e-9), axis="freq_hz")
    vs.add("h1", h1, unit="1", description="H1 estimate Pxy/Pxx",
           tolerance=lin_tol(1e-12, 1e-9), axis="freq_hz")
    vs.add("coherence", coh, unit="1", description="magnitude-squared coherence",
           tolerance=lin_tol(1e-12, 1e-9), axis="freq_hz")
    vs.add("h_true", h_true, unit="1",
           description=("analytic biquad * exp(-j 2 pi f D / fs); not equal to h1 "
                        "(finite averaging, noise, delay bias from segment length)"),
           axis="freq_hz")
    return vs


# --------------------------------------------------------------------------------------
# (c) IEC 61672-1 A and C weighting
# --------------------------------------------------------------------------------------

# IEC 61672-1:2013 Table 3, A and C weighting at nominal frequencies 10 Hz .. 20 kHz,
# rounded to 0.1 dB. Used only as a cross-check of the analytic evaluation below.
IEC_TABLE_A = [-70.4, -63.4, -56.7, -50.5, -44.7, -39.4, -34.6, -30.2, -26.2, -22.5,
               -19.1, -16.1, -13.4, -10.9, -8.6, -6.6, -4.8, -3.2, -1.9, -0.8,
               0.0, 0.6, 1.0, 1.2, 1.3, 1.2, 1.0, 0.5, -0.1, -1.1,
               -2.5, -4.3, -6.6, -9.3]
IEC_TABLE_C = [-14.3, -11.2, -8.5, -6.2, -4.4, -3.0, -2.0, -1.3, -0.8, -0.5,
               -0.3, -0.2, -0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
               0.0, 0.0, -0.1, -0.2, -0.3, -0.5, -0.8, -1.3, -2.0, -3.0,
               -4.4, -6.2, -8.5, -11.2]
NOMINAL_HZ = [10, 12.5, 16, 20, 25, 31.5, 40, 50, 63, 80,
              100, 125, 160, 200, 250, 315, 400, 500, 630, 800,
              1000, 1250, 1600, 2000, 2500, 3150, 4000, 5000, 6300, 8000,
              10000, 12500, 16000, 20000]


def iec61672_poles() -> tuple[float, float, float, float]:
    """Pole frequencies f1..f4 from IEC 61672-1:2013 Annex E (E.2-E.6).

    fr = 1 kHz, fL = 10^1.5 Hz, fH = 10^3.9 Hz, D = sqrt(1/2), fA = 10^2.45 Hz.
    b = [fr^2 + fL^2 fH^2 / fr^2 - D (fL^2 + fH^2)] / (1 - D), c = fL^2 fH^2,
    f1^2 = (-b - sqrt(b^2 - 4c)) / 2, f4^2 = (-b + sqrt(b^2 - 4c)) / 2.
    f2 = fA (3 - sqrt5)/2, f3 = fA (3 + sqrt5)/2.
    """
    fr, fl, fh, d, fa = 1000.0, 10.0**1.5, 10.0**3.9, np.sqrt(0.5), 10.0**2.45
    b = (fr**2 + fl**2 * fh**2 / fr**2 - d * (fl**2 + fh**2)) / (1.0 - d)
    c = fl**2 * fh**2
    disc = np.sqrt(b * b - 4.0 * c)
    f1 = np.sqrt((-b - disc) / 2.0)
    f4 = np.sqrt((-b + disc) / 2.0)
    f2 = fa * (3.0 - np.sqrt(5.0)) / 2.0
    f3 = fa * (3.0 + np.sqrt(5.0)) / 2.0
    return float(f1), float(f2), float(f3), float(f4)


def gen_weighting_iec61672() -> VectorSet:
    f1, f2, f3, f4 = iec61672_poles()
    # Annex E quotes f1 = 20.60 Hz, f2 = 107.7 Hz, f3 = 737.9 Hz, f4 = 12194 Hz.
    for got, want, tol in ((f1, 20.60, 0.01), (f2, 107.7, 0.1), (f3, 737.9, 0.1),
                           (f4, 12194.0, 1.0)):
        assert abs(got - want) < tol, (got, want)

    def c_raw(f):
        f = np.asarray(f, dtype=np.float64)
        return 20.0 * np.log10(f4**2 * f**2 / ((f**2 + f1**2) * (f**2 + f4**2)))

    def a_raw(f):
        f = np.asarray(f, dtype=np.float64)
        num = f4**2 * f**4
        den = ((f**2 + f1**2) * np.sqrt(f**2 + f2**2) * np.sqrt(f**2 + f3**2)
               * (f**2 + f4**2))
        return 20.0 * np.log10(num / den)

    # Normalisation constants: the exact gains that make the weighting 0 dB at 1 kHz.
    # The standard quotes the rounded values A1000 = -2.000 dB, C1000 = -0.062 dB.
    a1000 = float(a_raw(1000.0))
    c1000 = float(c_raw(1000.0))
    assert abs(a1000 - (-2.000)) < 5e-4 and abs(c1000 - (-0.062)) < 5e-4

    n_idx = np.arange(-20, 14)  # 10 Hz .. 20 kHz in base-10 one-third octaves
    exact = 1000.0 * 10.0 ** (n_idx / 10.0)
    a_db = a_raw(exact) - a1000
    c_db = c_raw(exact) - c1000
    table_a = np.array(IEC_TABLE_A)
    table_c = np.array(IEC_TABLE_C)
    # Table 3 values are the formula at exact frequencies, rounded to 0.1 dB.
    assert np.all(np.abs(a_db - table_a) <= 0.05 + 1e-9), np.c_[exact, a_db, table_a]
    assert np.all(np.abs(c_db - table_c) <= 0.05 + 1e-9), np.c_[exact, c_db, table_c]

    vs = VectorSet(
        name="weighting_iec61672",
        description=(
            "IEC 61672-1 A and C frequency weighting (dB) from the analytic Annex E "
            "expressions at the exact base-10 one-third-octave frequencies 10 Hz - 20 kHz."
        ),
        function="gen_weighting_iec61672",
        parameters={
            "frequencies": "f_m = 1000 * 10^(n/10) Hz, n = -20..13 (exact, base-10)",
            "C_formula": "C(f) = 20 log10[f4^2 f^2 / ((f^2 + f1^2)(f^2 + f4^2))] - C1000",
            "A_formula": ("A(f) = 20 log10[f4^2 f^4 / ((f^2 + f1^2)(f^2 + f2^2)^(1/2)"
                          "(f^2 + f3^2)^(1/2)(f^2 + f4^2))] - A1000"),
            "poles": ("IEC 61672-1:2013 Annex E: fr = 1000, fL = 10^1.5, fH = 10^3.9, "
                      "D = sqrt(1/2), fA = 10^2.45; "
                      "b = (fr^2 + fL^2 fH^2/fr^2 - D (fL^2 + fH^2))/(1 - D), c = fL^2 fH^2; "
                      "f1^2, f4^2 = (-b -/+ sqrt(b^2 - 4c))/2; "
                      "f2,3 = fA (3 -/+ sqrt5)/2"),
            "normalisation": ("A1000, C1000 computed exactly so A(1 kHz) = C(1 kHz) = 0; "
                              "standard's rounded constants are -2.000 / -0.062 dB"),
        },
        references=[
            "IEC 61672-1:2013 Electroacoustics - Sound level meters - Part 1, Annex E "
            "(analytical expressions for frequency weightings C, A and Z), Table 3",
            "PLAN.md 5.3 (A/C IIR verified per rate against IEC 61672-1 tables)",
        ],
        scalars={"f1_hz": f1, "f2_hz": f2, "f3_hz": f3, "f4_hz": f4,
                 "a1000_db": a1000, "c1000_db": c1000},
    )
    vs.add("nominal_hz", np.array(NOMINAL_HZ, dtype=np.float64), unit="Hz",
           description="IEC 61672-1 Table 3 nominal frequencies (labels)")
    vs.add("exact_hz", exact, unit="Hz", description="exact frequencies the weights are at",
           tolerance=lin_tol(0.0, 1e-14))
    vs.add("a_weight_db", a_db, unit="dB", description="A weighting at exact_hz",
           tolerance=lin_tol(1e-9, 0.0), axis="exact_hz")
    vs.add("c_weight_db", c_db, unit="dB", description="C weighting at exact_hz",
           tolerance=lin_tol(1e-9, 0.0), axis="exact_hz")
    vs.add("a_table_db", table_a, unit="dB",
           description="IEC 61672-1 Table 3 A weighting (rounded to 0.1 dB)",
           tolerance=lin_tol(0.05, 0.0), axis="exact_hz")
    vs.add("c_table_db", table_c, unit="dB",
           description="IEC 61672-1 Table 3 C weighting (rounded to 0.1 dB)",
           tolerance=lin_tol(0.05, 0.0), axis="exact_hz")
    return vs


# --------------------------------------------------------------------------------------
# (d) Known delays
# --------------------------------------------------------------------------------------


def gen_delay_integer_pos_neg() -> VectorSet:
    fs = 48000.0
    n = 8192
    delays = {"pos": 137, "neg": -61}  # samples, positive = y late
    noise_rms = 0.1
    seed = 0x0AC2_0004
    margin = 1024

    rng = np.random.default_rng(seed)
    src = rng.standard_normal(n + 2 * margin)
    x = src[margin : margin + n]

    vs = VectorSet(
        name="delay_integer_pos_neg",
        description=(
            "White-noise reference x and two measurement signals y = x delayed by a known "
            "integer number of samples (one positive, one negative) plus uncorrelated noise."
        ),
        function="gen_delay_integer_pos_neg",
        parameters={
            "fs_hz": fs,
            "n": n,
            "delay_sign": "positive lag = y (measurement) late: y[i] = x[i - lag] + noise",
            "construction": ("src = default_rng(seed).standard_normal(n + 2*margin); "
                             "x = src[margin : margin+n]; y = src[margin-lag : margin-lag+n]"
                             " + noise_rms * N(0,1) (no circular wrap, no zero fill)"),
            "margin": margin,
            "noise_rms": noise_rms,
            "seed": seed,
            "lag_check": ("argmax of scipy.signal.correlate(y, x, 'full') with "
                          "scipy.signal.correlation_lags(n, n, 'full') equals the lag"),
        },
        references=["PLAN.md 5.2 (lag sign: positive = measurement late; negative delays "
                    "first-class)"],
        scalars={f"lag_{k}_samples": v for k, v in delays.items()}
        | {f"lag_{k}_ms": v / fs * 1000.0 for k, v in delays.items()},
    )
    vs.add("x", x, unit="FS", description="reference signal")
    for key, lag in delays.items():
        y = src[margin - lag : margin - lag + n] + noise_rms * rng.standard_normal(n)
        corr = sig.correlate(y, x, mode="full", method="direct")
        lags = sig.correlation_lags(n, n, mode="full")
        assert int(lags[np.argmax(corr)]) == lag
        vs.add(f"y_{key}", y, unit="FS",
               description=f"measurement, lag {lag} samples (scalar lag_{key}_samples)")
    return vs


# --------------------------------------------------------------------------------------
# Driver
# --------------------------------------------------------------------------------------

GENERATORS = [
    gen_spectrum_hann_tone_noise,
    gen_transfer_h1_biquad_delay,
    gen_weighting_iec61672,
    gen_delay_integer_pos_neg,
]


SETS_DIR = Path(__file__).resolve().parent / "sets"


def discovered_generators() -> list:
    """Generators from tools/refgen/sets/*.py (each module exports GENERATORS).

    Set modules use the helpers here via `import generate as g`, so independent areas of
    work add vector sets without editing this file.
    """
    import importlib.util

    gens = []
    for path in sorted(SETS_DIR.glob("*.py")):
        if path.name.startswith("_"):
            continue
        spec = importlib.util.spec_from_file_location(f"refgen_sets_{path.stem}", path)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        gens.extend(mod.GENERATORS)
    return gens


def generate_all(out_dir: Path) -> list[str]:
    names = []
    for gen in GENERATORS + discovered_generators():
        vs = gen()
        vs.write(out_dir)
        names.append(vs.name)
    return names


# Platform libm differences (e.g. log10) move derived values by a few ulps between
# machines with identical numpy/scipy pins, so data is compared with a tight tolerance
# rather than bit-exact. Anything beyond it is a real change.
CHECK_RTOL = 1e-12
CHECK_ATOL = 1e-12


def compare_bins(a: Path, b: Path) -> list[str]:
    """Per-array comparison; returns failure lines (empty if within tolerance)."""
    try:
        meta = json.loads(b.with_suffix(".json").read_text(encoding="utf-8"))
        da, db = a.read_bytes(), b.read_bytes()
        if len(da) != len(db):
            return [f"    blob size {len(da)} != {len(db)}"]
        lines = []
        for arr in meta["arrays"]:
            lo, hi = arr["offset"], arr["offset"] + arr["nbytes"]
            va = np.frombuffer(da[lo:hi], dtype="<f8")
            vb = np.frombuffer(db[lo:hi], dtype="<f8")
            if va.shape != vb.shape:
                lines.append(f"    {arr['name']}: size differs")
            elif not np.allclose(va, vb, rtol=CHECK_RTOL, atol=CHECK_ATOL, equal_nan=True):
                lines.append(f"    {arr['name']}: max |diff| = {np.max(np.abs(va - vb)):.3e}")
        return lines
    except (OSError, ValueError, KeyError) as e:
        return [f"    (could not compare arrays: {e})"]


def json_without_hash(path: Path) -> dict:
    meta = json.loads(path.read_text(encoding="utf-8"))
    meta.get("blob", {}).pop("sha256", None)
    return meta


def check(committed: Path) -> int:
    with tempfile.TemporaryDirectory(prefix="ac2-refgen-") as tmp:
        tmp_dir = Path(tmp)
        generate_all(tmp_dir)
        fresh = {p.name for p in tmp_dir.iterdir()}
        existing = {p.name for p in committed.iterdir()} if committed.is_dir() else set()
        failures = []
        for name in sorted(fresh - existing):
            failures.append(f"missing in {committed}: {name}")
        for name in sorted(existing - fresh):
            failures.append(f"stale (not produced by generator): {name}")
        for name in sorted(fresh & existing):
            a, b = committed / name, tmp_dir / name
            if a.read_bytes() == b.read_bytes():
                continue
            if name.endswith(".bin"):
                lines = compare_bins(a, b)
                if lines:
                    failures.append(f"differs: {name}\n" + "\n".join(lines))
            elif name.endswith(".json"):
                # The blob hash legitimately changes with last-ulp drift; the rest must match.
                if json_without_hash(a) != json_without_hash(b):
                    failures.append(f"differs: {name} (metadata)")
            else:
                failures.append(f"differs: {name}")
    if failures:
        print(f"refgen --check FAILED (rtol={CHECK_RTOL}, atol={CHECK_ATOL}):", file=sys.stderr)
        for f in failures:
            print("  " + f, file=sys.stderr)
        print("Regenerate with tools/refgen/generate.py using the pinned numpy/scipy.",
              file=sys.stderr)
        return 1
    print(f"refgen --check OK: {len(fresh)} files in {committed} are up to date")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT, help="output directory")
    ap.add_argument("--check", action="store_true",
                    help="regenerate into a temp dir and fail if --out differs")
    args = ap.parse_args()
    if args.check:
        return check(args.out)
    names = generate_all(args.out)
    for name in names:
        size = (args.out / f"{name}.bin").stat().st_size
        print(f"wrote {name}.json + {name}.bin ({size} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
