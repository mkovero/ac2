"""MTW ladder reference vectors.

The ladder's per-stage pipeline (integer alignment at full rate -> Kaiser lowpass ->
keep every M-th output once the filter is full -> Hann-windowed Welch at the stage rate)
is rebuilt here from scipy primitives (kaiserord, firwin, lfilter, welch, csd), so the
Rust engine's framing, decimator phase, warm-up discard and spectral scaling are checked
against an independent implementation.
"""

import pathlib
import sys

import numpy as np
import scipy.signal as sig

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import generate as g  # noqa: E402

NFFT = 4096
DESIGN_PPO = 48
SERVED_FRACTION = 0.45
ATTEN_DB = 92.0


def kappa(ppo: int) -> float:
    """A 1/ppo-octave base-2 column at f is >= one bin wide where f >= kappa * df."""
    h = 0.5 / ppo
    return 1.0 / (2.0**h - 2.0**-h)


def stage_filter(sr: float, prev_bin_hz: float, m: int):
    """Decimator for a stage whose crossover sits at the shallower stage's validity edge.

    Passband: top of the 1/3-octave blend above that edge plus half a design column.
    Stopband: rate - min(1.25 * passband, rate / 2), so anything folding back lands
    above the served band.
    """
    rate = sr / m
    lo = kappa(DESIGN_PPO) * prev_bin_hz
    fp = lo * 2.0 ** (1.0 / 3.0) * 2.0 ** (0.5 / DESIGN_PPO)
    assert fp <= SERVED_FRACTION * rate
    fstop = rate - min(1.25 * fp, 0.5 * rate)
    nyq = sr / 2.0
    numtaps, beta = sig.kaiserord(ATTEN_DB, (fstop - fp) / nyq)
    numtaps |= 1  # odd length: integer group delay
    h = sig.firwin(numtaps, 0.5 * (fp + fstop), window=("kaiser", beta), fs=sr)
    return h, fp, fstop, beta


def gen_mtw_stage_pipeline_48k() -> g.VectorSet:
    sr = 48000.0
    n = 30000
    delay = 7  # measurement late by 7 samples
    seed = 0x0AC2_0301
    factors = [1, 4, 12]
    hops = [2048, 1024, 512]
    noise_rms = 0.2

    rng = np.random.default_rng(seed)
    x = rng.standard_normal(n)
    # Peaking biquad at 400 Hz so stage 1's band (<= ~1 kHz) has structure.
    f0, q, gain_db = 400.0, 1.5, 6.0
    a_lin = 10.0 ** (gain_db / 40.0)
    w0 = 2.0 * np.pi * f0 / sr
    alpha = np.sin(w0) / (2.0 * q)
    b = np.array([1.0 + alpha * a_lin, -2.0 * np.cos(w0), 1.0 - alpha * a_lin])
    a = np.array([1.0 + alpha / a_lin, -2.0 * np.cos(w0), 1.0 - alpha / a_lin])
    b, a = b / a[0], a / a[0]
    filtered = sig.lfilter(b, a, x)
    y = np.zeros(n)
    y[delay:] = filtered[: n - delay]
    y += noise_rms * rng.standard_normal(n)

    # Aligned pairs (reference[n - D], measurement[n]) for n >= D.
    xa = x[: n - delay]
    ya = y[delay:]

    vs = g.VectorSet(
        name="mtw_stage_pipeline_48k",
        description=(
            "MTW stage pipeline at 48 kHz: integer alignment, Kaiser decimation with "
            "warm-up discarded, and Hann Welch auto/cross spectra per stage, built from "
            "scipy primitives."
        ),
        function="tools/refgen/sets/mtw.py:gen_mtw_stage_pipeline_48k",
        parameters={
            "fs_hz": sr,
            "n": n,
            "delay_samples": delay,
            "delay_sign": "positive = measurement late; pair n = (x[n - D], y[n])",
            "nfft": NFFT,
            "factors": factors,
            "hops": hops,
            "design_ppo": DESIGN_PPO,
            "attenuation_db": ATTEN_DB,
            "decimator": ("firwin(kaiserord(92 dB, width)), odd length; output j = "
                          "lfilter(h, 1, aligned)[L - 1 + j M]"),
            "welch": "hann periodic, detrend False, density, mean; csd conj(X) Y",
            "biquad": f"RBJ peaking f0={f0} Q={q} gain={gain_db} dB",
            "noise_rms": noise_rms,
            "seed": seed,
        },
        references=[
            "PLAN.md 5.1 (stages, alignment before decimation, shared phase)",
            "scipy.signal.kaiserord / firwin / welch / csd",
        ],
    )
    vs.add("x", x, unit="FS", description="reference input, index 0 = sample 0")
    vs.add("y", y, unit="FS", description="measurement input")

    prev_bin = None
    for s, (m, hop) in enumerate(zip(factors, hops)):
        rate = sr / m
        if m == 1:
            xd, yd = xa, ya
        else:
            h, fp, fstop, beta = stage_filter(sr, prev_bin, m)
            L = len(h)
            xd = sig.lfilter(h, 1.0, xa)[L - 1 :: m]
            yd = sig.lfilter(h, 1.0, ya)[L - 1 :: m]
            vs.scalars[f"stage{s}_passband_hz"] = fp
            vs.scalars[f"stage{s}_stopband_hz"] = fstop
            vs.scalars[f"stage{s}_beta"] = beta
            vs.add(f"stage{s}_taps", h, unit="1", description=f"stage {s} decimator taps",
                   tolerance=g.lin_tol(1e-13, 0.0))
        prev_bin = rate / NFFT
        if len(xd) < NFFT:
            continue
        kw = dict(fs=rate, window="hann", nperseg=NFFT, noverlap=NFFT - hop, detrend=False,
                  return_onesided=True, scaling="density", average="mean")
        f, pxx = sig.welch(xd, **kw)
        _, pyy = sig.welch(yd, **kw)
        _, pxy = sig.csd(xd, yd, **kw)
        blocks = (len(xd) - NFFT) // hop + 1
        vs.scalars[f"stage{s}_blocks"] = blocks
        vs.add(f"stage{s}_freq_hz", f, unit="Hz", description=f"stage {s} bin frequencies",
               tolerance=g.lin_tol(0.0, 1e-12))
        vs.add(f"stage{s}_gxx", pxx, unit="FS^2/Hz", description=f"stage {s} Welch Gxx",
               tolerance=g.lin_tol(1e-15, 1e-8), axis=f"stage{s}_freq_hz")
        vs.add(f"stage{s}_gyy", pyy, unit="FS^2/Hz", description=f"stage {s} Welch Gyy",
               tolerance=g.lin_tol(1e-15, 1e-8), axis=f"stage{s}_freq_hz")
        vs.add(f"stage{s}_gxy", pxy, unit="FS^2/Hz", description=f"stage {s} Welch Gxy",
               tolerance=g.lin_tol(1e-15, 1e-8), axis=f"stage{s}_freq_hz")
    return vs


GENERATORS = [gen_mtw_stage_pipeline_48k]
