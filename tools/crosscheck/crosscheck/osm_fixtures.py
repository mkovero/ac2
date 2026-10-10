"""Synthetic meas/ref WAV pairs with analytic truth, for the OSM stage.

Every path is applied in the frequency domain over the whole file (circular): the file is
then exactly periodic, so a filter or a fractional delay has no start-up transient and its
response at every frequency is the closed form, which is what the analysers are judged
against. Channel 0 is the measurement, channel 1 the reference (OSM's `--stereo` order;
ac2 sees them as inputs 1 and 2)."""
from __future__ import annotations

import math
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from . import wav

FS = 96000


@dataclass
class Case:
    name: str
    seconds: float
    kind: str                       # tf | spectrum
    note: str
    # analytic truth on any frequency grid: complex meas/ref ratio (tf) or None
    delay_samples: float = 0.0
    polarity: int = 1
    biquad: tuple | None = None     # RBJ peaking (f0, gain_db, q)
    snr_db: float | None = None     # uncorrelated noise added to meas, re the reference power
    sine: tuple | None = None       # spectrum: (f_hz, peak amplitude)
    level_dbfs: float = -20.0       # reference noise RMS
    # measured as an operator would: each analyser's own finder result set as its delay
    # first (and the uncompensated result kept as context)
    align: bool = False
    seed: int = 1
    extra: dict = field(default_factory=dict)

    def truth(self, f: np.ndarray) -> np.ndarray:
        """The meas/ref transfer function the fixture was built with (H1's expectation: the
        added noise is uncorrelated with the reference, so it leaves H1 unbiased)."""
        H = self.polarity * np.exp(-2j * np.pi * f * self.delay_samples / FS)
        if self.biquad:
            H = H * peaking_response(f, FS, *self.biquad)
        return H

    def coherence2(self) -> float:
        """True γ² of the fixture: 1 for a noiseless linear path, SNR/(1+SNR) with noise."""
        if self.snr_db is None:
            return 1.0
        s = 10 ** (self.snr_db / 10)
        return s / (1 + s)


def peaking_coefs(f0: float, gain_db: float, q: float, fs: float):
    """RBJ cookbook peaking EQ (b, a), a[0] = 1."""
    A = 10 ** (gain_db / 40)
    w = 2 * math.pi * f0 / fs
    al = math.sin(w) / (2 * q)
    b = np.array([1 + al * A, -2 * math.cos(w), 1 - al * A])
    a = np.array([1 + al / A, -2 * math.cos(w), 1 - al / A])
    return b / a[0], a / a[0]


def peaking_response(f: np.ndarray, fs: float, f0: float, gain_db: float, q: float) -> np.ndarray:
    b, a = peaking_coefs(f0, gain_db, q, fs)
    z = np.exp(-2j * np.pi * np.asarray(f, dtype=float) / fs)
    return (b[0] + b[1] * z + b[2] * z * z) / (a[0] + a[1] * z + a[2] * z * z)


CASES: dict[str, Case] = {c.name: c for c in [
    Case("identity", 20, "tf", "M = R, white noise: 0 dB, 0 deg, coherence 1, delay 0"),
    Case("biquad", 20, "tf", "M = RBJ peaking +6 dB at 1 kHz, Q 2, of R", biquad=(1000.0, 6.0, 2.0), seed=2),
    Case("delay48", 20, "tf", "M = R delayed 48 samples (0.5 ms)", delay_samples=48.0, seed=3, align=True),
    Case("delay10_5", 20, "tf", "M = R delayed 10.5 samples (band-limited fractional delay)",
         delay_samples=10.5, seed=4, align=True),
    Case("polarity", 20, "tf", "M = -R", polarity=-1, seed=5),
    Case("snr20", 20, "tf", "M = R + uncorrelated white noise, SNR 20 dB", snr_db=20.0, seed=6),
    Case("snr10", 20, "tf", "M = R + uncorrelated white noise, SNR 10 dB", snr_db=10.0, seed=7),
    Case("snr0", 20, "tf", "M = R + uncorrelated white noise, SNR 0 dB", snr_db=0.0, seed=8),
    Case("sine1k", 10, "spectrum", "meas: bin-centred sine (FFT 65536) at -20 dBFS peak, R = white noise",
         sine=(683 * FS / 65536, 0.1), seed=9),
]}


def generate(case: Case, path: Path) -> Path:
    n = int(round(case.seconds * FS))
    rng = np.random.default_rng(case.seed)
    ref = rng.standard_normal(n) * 10 ** (case.level_dbfs / 20)
    f = np.fft.rfftfreq(n, 1 / FS)
    if case.kind == "spectrum":
        fs_, a = case.sine
        t = np.arange(n) / FS
        meas = a * np.sin(2 * np.pi * fs_ * t)
    else:
        X = np.fft.rfft(ref)
        H = case.truth(f)
        if n % 2 == 0:
            # the Nyquist bin of a real signal is real: keep its real part only
            H[-1] = H[-1].real
        meas = np.fft.irfft(X * H, n)
        if case.snr_db is not None:
            sig = np.sqrt(np.mean(ref ** 2))
            meas = meas + rng.standard_normal(n) * sig * 10 ** (-case.snr_db / 20)
    x = np.stack([meas, ref], axis=1)
    path.parent.mkdir(parents=True, exist_ok=True)
    wav.write(path, FS, x)
    return path
