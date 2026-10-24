"""Golden vectors for the stereo correlator (docs/design/programme-meter.md §4) and a
numpy check of the goniometer rotation (§5).

Correlator (identical in `ac2_core::stereo::Correlator`): exponential averages of l·r, l²
and r² with α = 1 − exp(−1/(fs τ)), τ the window; r = <lr> / sqrt(<l²><r²>) clamped to
[−1, 1]. Gated per side: a side whose mean square is below the gate (dBFS, 0 dBFS = the mean
square of a full-scale sine) names itself quiet and r is not reported.
"""

import pathlib
import sys

import numpy as np
import scipy.signal as sig

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import generate as g  # noqa: E402

FS = 48000.0
WINDOW_S = 0.3
GATE_DBFS = -60.0
BLOCK = 480
QUIET = {"none": 0.0, "left": 1.0, "right": 2.0}


def f32(x):
    return np.asarray(x, dtype=np.float64).astype(np.float32).astype(np.float64)


def pink(rng, n, fs):
    spec = np.fft.rfft(rng.standard_normal(n))
    f = np.fft.rfftfreq(n, 1.0 / fs)
    f[0] = f[1]
    x = np.fft.irfft(spec / np.sqrt(f), n)
    return x / np.sqrt(np.mean(x * x))


def correlate(l, r, fs, window_s, gate_dbfs, block):
    alpha = 1.0 - np.exp(-1.0 / (fs * window_s))
    ema = lambda v: sig.lfilter([alpha], [1.0, -(1.0 - alpha)], v)  # noqa: E731
    slr, sll, srr = ema(l * r), ema(l * l), ema(r * r)
    idx = np.arange(block - 1, len(l), block)
    gate_ms = 10.0 ** (gate_dbfs / 10.0) / 2.0
    quiet = np.where(sll[idx] < gate_ms, QUIET["left"],
                     np.where(srr[idx] < gate_ms, QUIET["right"], QUIET["none"]))
    with np.errstate(divide="ignore", invalid="ignore"):
        r_ = np.clip(slr[idx] / np.sqrt(sll[idx] * srr[idx]), -1.0, 1.0)
    return np.where(quiet == QUIET["none"], np.nan_to_num(r_), 0.0), quiet


def gen_stereo_correlation() -> g.VectorSet:
    rng = np.random.default_rng(0x5EED_C0)
    n = int(1.0 * FS)
    p = pink(rng, n, FS) * 0.1
    q = pink(rng, n, FS) * 0.1
    delayed = np.concatenate([np.zeros(48), p[:-48]])
    cases = {
        "mono": (p, p),
        "left_only": (p, np.zeros(n)),
        "right_only": (np.zeros(n), p),
        "reversed": (p, -p),
        "independent": (p, q),
        "delayed_1ms": (p, delayed),
        "gated_left": (p * 10.0 ** (-50.0 / 20.0), q),
    }
    vs = g.VectorSet(
        name="stereo_correlation",
        description="Zero-lag normalised cross-correlation over an exponential 0.3 s window, "
                    "one reading per 480-sample block, gated per side at -60 dBFS; pink noise "
                    "pairs: mono, one side only, reversed, independent, 1 ms delayed copy, "
                    "and a left side 50 dB down (gated).",
        function="gen_stereo_correlation",
        parameters={
            "fs_hz": FS, "window_s": WINDOW_S, "gate_dbfs": GATE_DBFS, "block_samples": BLOCK,
            "detector": __doc__.split("Correlator")[1].split("Gated")[0].strip(),
            "gate": "mean square < 10^(gate/10) / 2 (0 dBFS = full-scale sine); left checked "
                    "first; r = 0 where gated",
            "quiet_codes": QUIET,
            "cases": "l, r from p, q (rounded to f32 after composition): mono (p, p); "
                     "left_only (p, 0); right_only (0, p); reversed (p, -p); independent "
                     "(p, q); delayed_1ms (p, p delayed 48 samples, zeros first); "
                     "gated_left (p * 10^(-50/20), q)",
            "pink": "white Gaussian shaped 1/sqrt(f) in the FFT domain, unit RMS, scaled to "
                    "-20 dBFS RMS (0.1 FS); seed 0x5EEDC0; delayed copy: 48 samples",
        },
        references=["docs/design/programme-meter.md §4"],
    )
    vs.add("p", f32(p), unit="FS", description="pink noise p, -20 dBFS RMS")
    vs.add("q", f32(q), unit="FS", description="pink noise q, independent of p")
    for name, (l, r) in cases.items():
        l, r = f32(l), f32(r)
        rr, quiet = correlate(l, r, FS, WINDOW_S, GATE_DBFS, BLOCK)
        vs.add(f"{name}_r", rr, unit="1", description=f"{name} correlation per block",
               tolerance=g.lin_tol(1e-4, 0.0))
        vs.add(f"{name}_quiet", quiet, unit="code", description=f"{name} quiet side code")
    # Goniometer rotation check (no vector: the pick is random): left only leans left.
    l, r = p[:BLOCK], np.zeros(BLOCK)
    x, y = (r - l) / np.sqrt(2.0), (l + r) / np.sqrt(2.0)
    assert np.all(np.sign(x) == -np.sign(y)) and np.all(np.abs(x) == np.abs(y))
    vs.scalars["gonio_left_only_x_times_y_max"] = float(np.max(x * y))
    return vs


GENERATORS = [gen_stereo_correlation]
