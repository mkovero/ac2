"""Multi-resolution single-channel spectrum (docs/design/multires-spectrum.md).

An independent numpy implementation of the estimator in crates/ac2-core/src/multires.rs:
six full-rate bands of halving window length, per-band FIFO averaging of folded power,
tone scaling 2/S1^2, per-band column = max over the bins whose centres the column covers
(the bin under the centre when none), and between anchors A_b < f < 2 A_b a power blend
with the shorter band weighted f/A_b - 1.
"""

import pathlib
import sys

import numpy as np

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import generate as g  # noqa: E402

LF_BIN_HZ = 96000.0 / 65536.0
BANDS = 6
DESIGN_PPO = 48


def kappa(ppo: int) -> float:
    h = 0.5 / ppo
    return 1.0 / (2.0 ** h - 2.0 ** (-h))


def display_hop(n: int, fs: float) -> int:
    target = max(int(fs // 30), 64)
    pow2 = 1 << (target.bit_length() - 1)
    return int(min(max(pow2, max(n // 8, 1)), max(n // 2, 1)))


def longest_window(fs: float) -> int:
    need = int(np.ceil(fs / LF_BIN_HZ * (1.0 - 1e-12)))
    return max(1 << (need - 1).bit_length(), 1 << BANDS)


def gen_multires_spectrum() -> g.VectorSet:
    fs = 12000.0
    ppo = 24
    fifo = 4
    n_max = longest_window(fs)
    ns = [n_max >> b for b in range(BANDS)]
    hops = [display_hop(n, fs) for n in ns]
    length = ns[0] + (fifo - 1) * hops[0] + 500
    rng = np.random.default_rng(0x0AC2_3511)
    t = np.arange(length) / fs
    tones = [(50.0, 0.5), (700.0, 0.25), (3000.0, 0.1)]
    x = 0.01 * rng.standard_normal(length)
    for f, a in tones:
        x = x + a * np.sin(2.0 * np.pi * f * t)

    k = kappa(DESIGN_PPO)
    anchors = np.array([2.0 * k * fs / n for n in ns])

    band_tone = []
    for n, hop in zip(ns, hops):
        w = g.periodic_hann(n)
        s1 = np.sum(w)
        fold = g.one_sided_factor(n)
        ends = [n + j * hop for j in range((length - n) // hop + 1)]
        ends = ends[-fifo:]
        frames = [fold * np.abs(np.fft.rfft(w * x[e - n:e])) ** 2 for e in ends]
        band_tone.append(np.mean(frames, axis=0) * 2.0 / (s1 * s1))

    k_min = -7 * ppo
    k_max = int(np.floor(ppo * np.log2(fs / 2.0 / 1000.0) - 0.5))
    ks = np.arange(k_min, k_max + 1)
    freqs = 1000.0 * 2.0 ** (ks / ppo)
    half = 2.0 ** (0.5 / ppo)

    def band_value(b: int, f: float) -> float:
        n = ns[b]
        df = fs / n
        bins = n // 2 + 1
        lo, hi = f / half, f * half
        first = min(int(np.ceil(lo / df)), bins)
        end = min(int(np.ceil(hi / df)), bins)
        if end <= first:
            kk = min(int(np.round(f / df)), bins - 1)
            first, end = kk, kk + 1
        return float(np.max(band_tone[b][first:end]))

    level = []
    for f in freqs:
        if f <= anchors[0]:
            p = band_value(0, f)
        elif f >= anchors[-1]:
            p = band_value(BANDS - 1, f)
        else:
            b = int(np.max(np.nonzero(anchors < f)[0]))
            wt = min(max(f / anchors[b] - 1.0, 0.0), 1.0)
            p = (1.0 - wt) * band_value(b, f) + wt * band_value(b + 1, f)
        level.append(10.0 * np.log10(p))

    vs = g.VectorSet(
        name="multires_spectrum",
        description=(
            "Multi-resolution spectrum of three sines plus white noise at 12 kHz: six "
            "full-rate Hann bands (8192 ... 256 points), FIFO of 4 frames per band, tone "
            "level per 1/24-octave column, bands blended in power between anchors."
        ),
        function="gen_multires_spectrum",
        parameters={
            "fs_hz": fs,
            "ppo": ppo,
            "fifo_frames": fifo,
            "windows": ns,
            "hops": hops,
            "signal": "0.01 N(0,1) + sum a sin(2 pi f t), (f, a) = " + str(tones),
            "seed": "numpy default_rng(0x0AC23511)",
            "anchor": "A_b = 2 kappa(48) fs / N_b, kappa(p) = 1/(2^(1/2p) - 2^(-1/2p))",
            "blend": "A_b < f < 2A_b: (1-w) P_b + w P_{b+1}, w = f/A_b - 1",
            "column": "max tone power over bins k with ceil(lo/df) <= k < ceil(hi/df); "
                      "none: k = round(f/df)",
            "tone": "2/S1^2 * mean over the last 4 frames of c_k |X_k|^2",
        },
        references=["docs/design/multires-spectrum.md", "crates/ac2-core/src/multires.rs"],
        scalars={"fs_hz": fs, "ppo": ppo, "fifo_frames": fifo},
    )
    vs.add("x", x, unit="FS", description="input")
    vs.add("hops", np.array(hops, dtype=float), unit="samples",
           description="hop of each band, longest window first")
    vs.add("anchor_hz", anchors, unit="Hz", description="band anchors",
           tolerance=g.lin_tol(1e-9, 1e-12))
    vs.add("freq_hz", freqs, unit="Hz", description="column centres",
           tolerance=g.lin_tol(1e-9, 1e-12))
    vs.add("level_db", np.array(level), unit="dB re tone power of a full-scale sine",
           description="tone level per column", axis="freq_hz",
           tolerance=g.lin_tol(1e-6, 0.0))
    return vs


GENERATORS = [gen_multires_spectrum]
