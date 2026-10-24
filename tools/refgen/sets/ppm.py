"""Golden vectors for the programme meters (docs/design/programme-meter.md §3): IEC 60268-10
quasi-peak Type I and Type II tone-burst sequences, the IEC 60268-17 VU step, and BS.1770-5
Annex 2 true peak.

Quasi-peak model (identical in `ac2_core::ppm`):

    h    = max(|x|, h·rel)                     peak-held rectifier, released at the return rate
    y_i  = y_i + α_i (h − y_i)   if h > y_i    two first-order charges toward the held peak
         = y_i · rel             otherwise      otherwise the return, linear in dB
    out  = a·y1 + (1 − a)·y2
    α_i  = 1 − exp(−1 / (fs τ_i)),  rel = 10^(−return_db_per_s / (20 fs))

The hold exists because the rectified sine falls to zero twice a period: charging the states
straight from |x| makes a steady 5 kHz tone read 0.2 dB under its peak (and the fit for
Type I degenerates to a single charge). A real meter's rectifier bridges the troughs with a
reservoir; so does the hold, and the steady tone then reads its peak within 0.01 dB.

`a, τ1, τ2` per type are least-squares fits (in dB over the table rows, `python ppm.py
--fit`) to the burst table in the design note; they are frozen here and in `ppm.rs`.
"""

import pathlib
import sys

import numpy as np
import scipy.signal as sig

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import generate as g  # noqa: E402

FS = 48000.0
BURST_HZ = 5000.0

# {type: (burst ms, reading dB re steady tone, return dB, return s)}
TABLE = {
    "type1": ([100.0, 10.0, 5.0, 3.0, 0.4], [0.0, -1.0, -2.0, -4.0, -15.0], 20.0, 1.7),
    "type2": ([100.0, 10.0, 5.0, 1.5, 0.5], [0.0, -2.0, -4.0, -9.0, -17.0], 24.0, 2.8),
}

# Frozen fit (--fit): {type: (a, tau1_s, tau2_s)}; residuals in dB per table row:
#   type1: +0.00, +0.24, −0.30, +0.15, −0.01
#   type2: −0.00, −0.06, +0.11, −0.14, +0.08
FIT = {
    "type1": (0.360601, 0.000794, 0.004907),
    "type2": (0.433480, 0.001484, 0.010061),
}

VU_TAU_S = 0.3 / np.log(100.0)  # 99 % of a steady tone in 300 ms
VU_SINE_SCALE = np.pi / (2.0 * np.sqrt(2.0))  # mean|sin| = 2/π of the peak → RMS

# BS.1770-5 Annex 2, order-48 4-phase interpolating FIR; column p row m = h[4m + p].
TRUE_PEAK_PHASES = np.array([
    [0.0017089843750, -0.0291748046875, -0.0189208984375, -0.0083007812500],
    [0.0109863281250, 0.0292968750000, 0.0330810546875, 0.0148925781250],
    [-0.0196533203125, -0.0517578125000, -0.0582275390625, -0.0266113281250],
    [0.0332031250000, 0.0891113281250, 0.1015625000000, 0.0476074218750],
    [-0.0594482421875, -0.1665039062500, -0.2003173828125, -0.1022949218750],
    [0.1373291015625, 0.4650878906250, 0.7797851562500, 0.9721679687500],
    [0.9721679687500, 0.7797851562500, 0.4650878906250, 0.1373291015625],
    [-0.1022949218750, -0.2003173828125, -0.1665039062500, -0.0594482421875],
    [0.0476074218750, 0.1015625000000, 0.0891113281250, 0.0332031250000],
    [-0.0266113281250, -0.0582275390625, -0.0517578125000, -0.0196533203125],
    [0.0148925781250, 0.0330810546875, 0.0292968750000, 0.0109863281250],
    [-0.0083007812500, -0.0189208984375, -0.0291748046875, 0.0017089843750],
])
TRUE_PEAK_TAPS = TRUE_PEAK_PHASES.reshape(-1)  # h[n], n = 4m + p


def f32(x: np.ndarray) -> np.ndarray:
    """Round to f32 so the Rust side (f32 blocks) sees exactly these inputs."""
    return np.asarray(x, dtype=np.float64).astype(np.float32).astype(np.float64)


def burst(ms: float, fs: float = FS, f0: float = BURST_HZ) -> np.ndarray:
    nb = int(round(ms * 1e-3 * fs))
    return f32(np.sin(2.0 * np.pi * f0 * np.arange(nb) / fs))


def quasi_peak(x: np.ndarray, a: float, tau1: float, tau2: float, return_db: float,
               return_s: float, fs: float = FS, block: int = 1):
    """Block maxima of the detector output (what `QuasiPeak::process` returns per block)."""
    al1 = 1.0 - np.exp(-1.0 / (fs * tau1))
    al2 = 1.0 - np.exp(-1.0 / (fs * tau2))
    rel = 10.0 ** (-(return_db / return_s) / (20.0 * fs))
    b = 1.0 - a
    h = y1 = y2 = 0.0
    out = []
    vals = x.tolist()
    for start in range(0, len(vals), block):
        m = 0.0
        for v in vals[start:start + block]:
            u = v if v >= 0.0 else -v
            h = u if u > h else h * rel
            y1 = y1 + al1 * (h - y1) if h > y1 else y1 * rel
            y2 = y2 + al2 * (h - y2) if h > y2 else y2 * rel
            o = a * y1 + b * y2
            if o > m:
                m = o
        out.append(m)
    return np.array(out)


def table_readings(kind: str, a: float, tau1: float, tau2: float):
    rows, _, ret_db, ret_s = TABLE[kind]
    steady = quasi_peak(burst(400.0), a, tau1, tau2, ret_db, ret_s).max()
    got = [quasi_peak(burst(ms), a, tau1, tau2, ret_db, ret_s).max() for ms in rows]
    return 20.0 * np.log10(np.array(got) / steady), steady


def fit(kind: str):
    from scipy.optimize import least_squares

    rows, target, _, _ = TABLE[kind]
    target = np.asarray(target)

    def residual(p):
        return table_readings(kind, *p)[0] - target

    best = None
    for a0 in (0.3, 0.6):
        for t10 in (0.8e-3, 2e-3):
            for t20 in (5e-3, 20e-3):
                r = least_squares(residual, [a0, t10, t20],
                                  bounds=([0.01, 1e-4, 1e-3], [0.99, 20e-3, 1.0]))
                if best is None or r.cost < best.cost:
                    best = r
    return best.x, residual(best.x)


def gen_ppm_quasi_peak() -> g.VectorSet:
    gap = 0.5
    vs = g.VectorSet(
        name="ppm_quasi_peak",
        description=(
            "IEC 60268-10 Type I and Type II quasi-peak detector: block maxima every 1 ms over "
            "a 5 kHz tone-burst sequence (the table bursts, then a 3 s return), plus the "
            "table readings of each burst relative to the steady tone."
        ),
        function="gen_ppm_quasi_peak",
        parameters={
            "fs_hz": FS,
            "burst_hz": BURST_HZ,
            "burst": "x = sin(2 pi f0 k / fs), k = 0..round(T fs)-1, rounded to f32",
            "sequence_s": "silence 0.05, bursts from the table each followed by a 0.5 s gap, "
                          "the last gap 3 s",
            "detector": __doc__.split("Quasi-peak model")[1].split("The hold")[0].strip(),
            "block_samples": 48,
            "fit": {k: {"a": v[0], "tau1_s": v[1], "tau2_s": v[2]} for k, v in FIT.items()},
            "return": {k: {"db": v[2], "s": v[3]} for k, v in TABLE.items()},
            "steady_reference": "max detector output over a 400 ms burst",
        },
        references=[
            "IEC 60268-10 (Type I, Type II burst tables and return times) as tabulated in "
            "docs/design/programme-meter.md §3",
        ],
    )
    vs.add("burst_100ms", burst(100.0), unit="FS",
           description="the 100 ms burst; every table burst is a prefix of it")
    for kind, (rows, target, ret_db, ret_s) in TABLE.items():
        a, t1, t2 = FIT[kind]
        pieces = [np.zeros(int(0.05 * FS))]
        segments = []
        for i, ms in enumerate(rows):
            b = burst(ms)
            segments.append([sum(len(q) for q in pieces), len(b)])
            pieces.append(b)
            pieces.append(np.zeros(int((3.0 if i == len(rows) - 1 else gap) * FS)))
        x = np.concatenate(pieces)
        x = x[: len(x) // 48 * 48]
        out = quasi_peak(x, a, t1, t2, ret_db, ret_s, block=48)
        rel_db, steady = table_readings(kind, a, t1, t2)
        vs.scalars[f"{kind}_steady_reading"] = float(steady)
        vs.scalars[f"{kind}_input_samples"] = len(x)
        vs.add(f"{kind}_segments", np.array(segments, dtype=float), unit="samples",
               description=f"{kind} bursts in the sequence, [start, length]; zeros elsewhere")
        vs.add(f"{kind}_block_max", out, unit="FS",
               description=f"{kind} detector block maxima, one per 48 samples",
               tolerance=g.db_tol(0.05, -80.0))
        vs.add(f"{kind}_burst_ms", np.array(rows), unit="ms", description="table bursts")
        vs.add(f"{kind}_table_db", np.array(target), unit="dB",
               description="table reading re steady tone", axis=f"{kind}_burst_ms")
        vs.add(f"{kind}_reading_db", rel_db, unit="dB",
               description="fitted detector reading re steady tone",
               tolerance=g.lin_tol(0.05, 0.0), axis=f"{kind}_burst_ms")
    return vs


def gen_ppm_vu_step() -> g.VectorSet:
    fs = FS
    amp = 0.5
    n = int(1.0 * fs)
    x = f32(amp * np.sin(2.0 * np.pi * 1000.0 * np.arange(n) / fs))
    alpha = 1.0 - np.exp(-1.0 / (fs * VU_TAU_S))
    y = sig.lfilter([alpha], [1.0, -(1.0 - alpha)], np.abs(x)) * VU_SINE_SCALE
    block = int(0.01 * fs)
    sampled = y[block - 1::block]
    vs = g.VectorSet(
        name="ppm_vu_step",
        description="IEC 60268-17 VU: first-order mean of |x| (99 % in 300 ms) scaled so a "
                    "steady sine reads its RMS; a 1 kHz tone switched on at t = 0, the state "
                    "after every 10 ms block for 1 s.",
        function="gen_ppm_vu_step",
        parameters={
            "fs_hz": fs, "tone_hz": 1000.0, "amplitude_fs": amp,
            "tau_s": float(VU_TAU_S), "tau": "0.3 / ln 100",
            "detector": "y[n] = y[n-1] + alpha (|x[n]| - y[n-1]), alpha = 1 - exp(-1/(fs tau)); "
                        "reading = y * pi / (2 sqrt 2)",
            "block_samples": block,
        },
        references=["IEC 60268-17 (300 ms to 99 %)"],
    )
    vs.scalars["rms"] = amp / np.sqrt(2.0)
    vs.add("period", x[:48], unit="FS", description="one period of the tone (48 samples)")
    vs.add("reading", sampled, unit="FS", description="VU reading after each 10 ms block",
           tolerance=g.db_tol(0.05, -80.0))
    return vs


def true_peak_blocks(x: np.ndarray, block: int) -> np.ndarray:
    """Per-block max |4x interpolated sample| with the Annex 2 filter, zero history."""
    y = sig.upfirdn(TRUE_PEAK_TAPS, x, up=4)[: 4 * len(x)].reshape(len(x), 4)
    peaks = np.abs(y).max(axis=1)
    return peaks[: len(x) // block * block].reshape(-1, block).max(axis=1)


def gen_ppm_true_peak() -> g.VectorSet:
    rng = np.random.default_rng(0x5EED_7B)
    vs = g.VectorSet(
        name="ppm_true_peak",
        description="BS.1770-5 Annex 2 true peak (order-48, 4-phase FIR, 4x at every rate): "
                    "per-block maxima for an fs/4 sine sampled 45 deg off its peak, an "
                    "11.025 kHz sine at 44.1 kHz, and a pink-noise burst.",
        function="gen_ppm_true_peak",
        parameters={
            "taps": "h[4m + p] = Annex 2 table column p (phase) row m; y[4n+p] = "
                    "sum_m h[4m+p] x[n-m]; scipy.signal.upfirdn(h, x, up=4)",
            "block_samples": 480,
            "sine_fs4": "sin(pi n / 2 + pi / 4) at 48 kHz: sample peak -3.01 dBFS, true peak 0 dBTP",
            "sine_11025": "sin(pi n / 2) at 44.1 kHz",
            "pink": "white Gaussian shaped 1/sqrt(f) in the FFT domain, scaled to peak 0.5, "
                    "seed 0x5EED7B",
        },
        references=["ITU-R BS.1770-5 Annex 2 (over-sampled peak)"],
    )
    vs.add("phases", TRUE_PEAK_PHASES, unit="1",
           description="filter coefficients, [row m][phase p]")
    n = 4800
    x1 = f32(np.sin(np.pi * np.arange(n) / 2.0 + np.pi / 4.0))
    x2 = f32(np.sin(np.pi * np.arange(n) / 2.0))
    w = rng.standard_normal(n)
    spec = np.fft.rfft(w)
    f = np.fft.rfftfreq(n, 1.0 / 48000.0)
    f[0] = f[1]
    pink = np.fft.irfft(spec / np.sqrt(f), n)
    pink = f32(0.5 * pink / np.abs(pink).max())
    for name, x in (("sine_fs4", x1), ("sine_11025", x2), ("pink", pink)):
        tp = true_peak_blocks(x, 480)
        vs.add(f"{name}_input", x, unit="FS", description=f"{name} input")
        vs.add(f"{name}_block_true_peak", tp, unit="FS",
               description=f"{name} true peak per 480-sample block",
               tolerance=g.lin_tol(1e-4, 0.0))
    vs.scalars["sine_fs4_sample_peak"] = float(np.abs(x1).max())
    vs.scalars["sine_fs4_true_peak"] = float(true_peak_blocks(x1, 480)[1:].max())
    vs.scalars["sine_11025_true_peak"] = float(true_peak_blocks(x2, 480)[1:].max())
    return vs


GENERATORS = [gen_ppm_quasi_peak, gen_ppm_vu_step, gen_ppm_true_peak]


if __name__ == "__main__" and "--fit" in sys.argv:
    for kind in TABLE:
        p, resid = fit(kind)
        steady = table_readings(kind, *p)[1]
        print(f"{kind}: a={p[0]:.6f} tau1_s={p[1]:.6f} tau2_s={p[2]:.6f} "
              f"steady/peak={20 * np.log10(steady):+.4f} dB")
        for ms, r in zip(TABLE[kind][0], resid):
            print(f"  {ms:6.1f} ms residual {r:+.2f} dB")
