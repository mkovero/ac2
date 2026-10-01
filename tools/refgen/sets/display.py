"""Display-path vector sets: fractional-octave smoothing and live IR views."""

from __future__ import annotations

import pathlib
import sys

import numpy as np
import scipy.signal as sig

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import generate as g  # noqa: E402


def log_grid(ppo: int, f_lo: float, f_hi: float) -> np.ndarray:
    """Base-2 grid anchored at 1 kHz: f_i = 1000 * 2^((k_min + i) / ppo)."""
    k_min = int(np.floor(np.log2(f_lo / 1000.0) * ppo))
    k_max = int(np.ceil(np.log2(f_hi / 1000.0) * ppo))
    k = np.arange(k_min, k_max + 1)
    return 1000.0 * 2.0 ** (k / ppo)


def hann_taps(ppo: int, b: int) -> np.ndarray:
    """Two-sided Hann taps in log2 frequency, full width 1.5/b octaves (ENBW = 1/b oct)."""
    half = 0.5 * 1.5 / b * ppo
    reach = int(np.ceil(half) - 1)
    d = np.arange(-reach, reach + 1)
    return 0.5 * (1.0 + np.cos(np.pi * d / half))


def smooth_runs(x: np.ndarray, valid: np.ndarray, taps: np.ndarray) -> np.ndarray:
    """Truncated, renormalised moving average inside each run of valid columns."""
    out = x.copy()
    reach = (len(taps) - 1) // 2
    idx = np.flatnonzero(np.diff(np.concatenate(([0], valid.astype(int), [0]))))
    for start, end in zip(idx[0::2], idx[1::2]):
        seg = x[start:end]
        num = np.convolve(seg, taps, mode="full")[reach:reach + len(seg)]
        den = np.convolve(np.ones_like(seg), taps, mode="full")[reach:reach + len(seg)]
        out[start:end] = num / den
    return out


def gen_display_smoothing_biquad() -> g.VectorSet:
    fs = 48000.0
    ppo, f_lo, f_hi = 48, 20.0, 20000.0
    freq = log_grid(ppo, f_lo, f_hi)
    # RBJ peaking EQ, +12 dB, Q = 8 at 2 kHz, plus a 0.4 ms delay: a narrow feature and a
    # phase that wraps many times across the band.
    f0, gain_db, q, delay_s = 2000.0, 12.0, 8.0, 0.4e-3
    a = 10.0 ** (gain_db / 40.0)
    w0 = 2.0 * np.pi * f0 / fs
    alpha = np.sin(w0) / (2.0 * q)
    bq = np.array([1 + alpha * a, -2 * np.cos(w0), 1 - alpha * a])
    aq = np.array([1 + alpha / a, -2 * np.cos(w0), 1 - alpha / a])
    _, hz = sig.freqz(bq, aq, worN=freq, fs=fs)
    h = hz * np.exp(-2j * np.pi * freq * delay_s)

    valid = np.ones(len(freq), dtype=bool)
    gap = np.flatnonzero((freq > 250.0) & (freq < 280.0))
    valid[gap] = False
    h[gap] = 1e3 * np.exp(1j * 0.3)  # garbage that must not leak into neighbours

    # Power mode, 1/3 octave: sqrt of smoothed |H|^2.
    mag_third = np.sqrt(smooth_runs(np.abs(h) ** 2, valid, hann_taps(ppo, 3)))
    mag_third[~valid] = np.abs(h[~valid])

    # Complex mode, 1/6 octave: smoothed power and smoothed unwrapped phase, unwrapped
    # independently per valid run.
    taps6 = hann_taps(ppo, 6)
    pw = smooth_runs(np.abs(h) ** 2, valid, taps6)
    ph = np.angle(h)
    idx = np.flatnonzero(np.diff(np.concatenate(([0], valid.astype(int), [0]))))
    for start, end in zip(idx[0::2], idx[1::2]):
        ph[start:end] = np.unwrap(ph[start:end])
    ph = smooth_runs(ph, valid, taps6)
    cpx_sixth = np.sqrt(pw) * np.exp(1j * ph)
    cpx_sixth[~valid] = h[~valid]

    vs = g.VectorSet(
        name="display_smoothing_biquad",
        description=(
            "Fractional-octave display smoothing on the 48 ppo base-2 grid of a peaking "
            "biquad with delay and a masked gap: power mode 1/3 oct, complex mode 1/6 oct."
        ),
        function="gen_display_smoothing_biquad",
        parameters={
            "fs_hz": fs,
            "biquad": "RBJ cookbook peaking EQ",
            "f0_hz": f0,
            "gain_db": gain_db,
            "q": q,
            "delay_s": delay_s,
            "grid": "f_i = 1000 * 2^((k_min + i)/ppo), k_min = floor(ppo log2(f_lo/1k))",
            "kernel": "w(d) = 0.5 (1 + cos(pi d / H)), |d| < H, H = 0.75 ppo / b columns",
            "edges": "truncated, renormalised by the sum of the weights present",
            "gap": "columns 250 < f < 280 Hz invalid; smoothing confined to valid runs",
            "power_mode": "|H_s| = sqrt(sum w |H|^2 / sum w), phase kept",
            "complex_mode": "|H_s| as power; phase = sum w unwrap(arg H) / sum w per run",
        },
        references=["PLAN.md 3.3, 5.1", "numpy.convolve / numpy.unwrap", "scipy.signal.freqz"],
        scalars={"ppo": float(ppo), "f_lo_hz": f_lo, "f_hi_hz": f_hi},
    )
    vs.add("freq_hz", freq, unit="Hz", description="grid column centres",
           tolerance=g.lin_tol(0.0, 1e-12))
    vs.add("h", h, unit="1", description="input H per column", axis="freq_hz")
    vs.add("valid", valid.astype(float), unit="1", description="1 = valid column")
    vs.add("power_third_mag", mag_third, unit="1",
           description="|H| after 1/3 octave power smoothing",
           tolerance=g.lin_tol(1e-12, 1e-10), axis="freq_hz")
    vs.add("complex_sixth", cpx_sixth, unit="1",
           description="H after 1/6 octave complex (power + unwrapped phase) smoothing",
           tolerance=g.lin_tol(1e-10, 1e-9), axis="freq_hz")
    return vs


def gen_display_ir_etc() -> g.VectorSet:
    fs = 48000.0
    n = 4096
    # Two arrivals: a band-passed direct sound at +40 samples and a weaker reflection at
    # +700 samples, plus a pre-arrival at -30 samples (negative time relative to the
    # inserted delay).
    sos = sig.butter(2, [500.0, 4000.0], btype="bandpass", fs=fs, output="sos")
    impulse = np.zeros(n)
    impulse[0] = 1.0
    bp = sig.sosfilt(sos, impulse)
    ir_circ = np.zeros(n)
    for lag, gain in ((40, 1.0), (700, 0.3), (-30, 0.1)):
        ir_circ += gain * np.roll(bp, lag)
    h = np.fft.rfft(ir_circ)
    ir_circ = np.fft.irfft(h, n)
    # Time axis: sample offset i - n/2 relative to the inserted delay (fftshift).
    ir = np.fft.fftshift(ir_circ)
    env = np.abs(sig.hilbert(ir))
    floor_db = -200.0
    etc_db = 20.0 * np.log10(np.maximum(env, 10.0 ** (floor_db / 20.0)))

    vs = g.VectorSet(
        name="display_ir_etc",
        description=(
            "Live IR view from a one-sided uniform-bin transfer function: circular IFFT, "
            "fftshift so time 0 (the inserted delay) sits at index n/2, ETC = 20 log10 of "
            "the Hilbert envelope."
        ),
        function="gen_display_ir_etc",
        parameters={
            "fs_hz": fs,
            "n_fft": n,
            "arrivals": "butter(2, [500, 4000] Hz) bandpass impulse at lags 40, 700, -30 "
                        "samples, gains 1.0, 0.3, 0.1 (circular)",
            "ir": "fftshift(irfft(h, n)); index i is time (i - n/2)/fs",
            "etc": "20 log10 |scipy.signal.hilbert(ir)|, floored at floor_db",
        },
        references=["PLAN.md 3.3 (live IR panel)", "scipy.signal.hilbert"],
        scalars={"fs_hz": fs, "n_fft": float(n), "floor_db": floor_db},
    )
    vs.add("h", h, unit="1", description="one-sided transfer function, n/2 + 1 bins")
    vs.add("ir", ir, unit="1", description="fftshifted impulse response",
           tolerance=g.lin_tol(1e-14, 1e-9))
    vs.add("etc_db", etc_db, unit="dB", description="Hilbert envelope in dB",
           tolerance=g.db_tol(1e-6, -150.0))
    return vs


GENERATORS = [gen_display_smoothing_biquad, gen_display_ir_etc]
