"""Power spectral density of the narrowband spectra (fixed and multi-resolution).

Density is tone power over the noise bandwidth: per bin 2 F_k / (fs S2) (F_k = c_k |X_k|^2,
S2 = sum w^2), i.e. tone power 2 F_k / S1^2 divided by ENBW * fs / N. It keeps the dBFS
reference of tone power (a full-scale sine's mean square), so white noise of standard
deviation sigma reads 4 sigma^2 / fs per hertz: integrated over 0 .. fs/2 that is 2 sigma^2,
the noise's level in dBFS.

A multi-resolution column takes the mean of the bins it covers in each band (the bin under
its centre when it covers none), blends the bands in power with the weights of
sets/multires.py and divides by the column's noise bandwidth
B(f) = sum_b w_b * ENBW * fs / N_b.

Each set holds two signals: white noise plus a bin-centred sine, and pink noise synthesised
in the frequency domain with an exact 1/f density, with their analytic densities.
"""

import pathlib
import sys

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE))
import generate as g  # noqa: E402
import multires as mr  # noqa: E402

SIGMA = 0.1
F_REF = 1000.0


def hann_enbw(n: int) -> float:
    w = g.periodic_hann(n)
    return n * np.sum(w * w) / np.sum(w) ** 2


def pink(length: int, fs: float, rng) -> np.ndarray:
    """White noise of deviation SIGMA shaped by sqrt(F_REF / f) over the whole (circular)
    signal: one-sided density 4 SIGMA^2 / fs * F_REF / f in ac2's dBFS/Hz."""
    w = SIGMA * rng.standard_normal(length)
    spec = np.fft.rfft(w)
    f = np.fft.rfftfreq(length, 1.0 / fs)
    shape = np.zeros_like(f)
    shape[1:] = np.sqrt(F_REF / f[1:])
    return np.fft.irfft(spec * shape, n=length)


def white_density_db(fs: float) -> float:
    return float(10.0 * np.log10(4.0 * SIGMA ** 2 / fs))


def pink_density_db(f: np.ndarray, fs: float) -> np.ndarray:
    return 10.0 * np.log10(4.0 * SIGMA ** 2 / fs * F_REF / f)


def band_density(x: np.ndarray, n: int, hop: int, frames: int, fs: float) -> np.ndarray:
    """Mean over the last `frames` frames of 2 c_k |X_k|^2 / (fs S2), periodic Hann."""
    w = g.periodic_hann(n)
    s2 = np.sum(w * w)
    fold = g.one_sided_factor(n)
    ends = [n + j * hop for j in range((len(x) - n) // hop + 1)][-frames:]
    p = [fold * np.abs(np.fft.rfft(w * x[e - n:e])) ** 2 for e in ends]
    return np.mean(p, axis=0) * 2.0 / (fs * s2)


def gen_spectrum_density() -> g.VectorSet:
    fs = 48000.0
    n = 1024
    hop = 512
    frames = 32
    length = n + (frames - 1) * hop
    rng = np.random.default_rng(0x0AC2_D351)
    k_sine = 96
    f_sine = k_sine * fs / n
    a_sine = 0.5
    t = np.arange(length) / fs
    x_a = SIGMA * rng.standard_normal(length) + a_sine * np.sin(2.0 * np.pi * f_sine * t)
    x_pink = pink(length, fs, rng)
    freqs = np.arange(n // 2 + 1) * fs / n
    enbw_hz = hann_enbw(n) * fs / n
    d_a = band_density(x_a, n, hop, frames, fs)
    d_pink = band_density(x_pink, n, hop, frames, fs)
    sine_level_db = 10.0 * np.log10(a_sine ** 2)

    vs = g.VectorSet(
        name="spectrum_density",
        description=(
            "Power spectral density of a 1024-point Hann spectrum at 48 kHz, FIFO of 32 "
            "frames (hop 512): white noise plus a bin-centred 4.5 kHz sine, and pink noise; "
            "with the analytic densities."
        ),
        function="gen_spectrum_density",
        parameters={
            "fs_hz": fs,
            "n": n,
            "hop": hop,
            "fifo_frames": frames,
            "signal_a": f"{SIGMA} N(0,1) + {a_sine} sin(2 pi {f_sine} t)",
            "signal_pink": f"{SIGMA} N(0,1) shaped by sqrt({F_REF}/f) over the whole signal",
            "seed": "numpy default_rng(0x0AC2D351), white first, then the pink's white",
            "density": "2/(fs S2) * mean over the last 32 frames of c_k |X_k|^2",
            "white_density": "4 sigma^2 / fs",
            "pink_density": "4 sigma^2 / fs * 1000 / f",
            "sine": "density at its bin = a^2/2 * 2 / (ENBW fs/N): level - 10 lg(ENBW fs/N)",
        },
        references=["crates/ac2-core/src/spectrum.rs", "docs/design/multires-spectrum.md"],
        scalars={
            "fs_hz": fs,
            "n": n,
            "hop": hop,
            "fifo_frames": frames,
            "sine_bin": k_sine,
            "sine_level_db": float(sine_level_db),
            "enbw_hz": float(enbw_hz),
            "white_density_db": white_density_db(fs),
            "pink_density_1khz_db": float(pink_density_db(np.array([F_REF]), fs)[0]),
        },
    )
    vs.add("x_a", x_a, unit="FS", description="white noise + bin-centred sine")
    vs.add("x_pink", x_pink, unit="FS", description="pink noise")
    vs.add("freq_hz", freqs, unit="Hz", description="bin centres",
           tolerance=g.lin_tol(1e-9, 1e-12))
    vs.add("density_a_db", 10.0 * np.log10(d_a), unit="dBFS/Hz",
           description="density of x_a per bin", axis="freq_hz",
           tolerance=g.lin_tol(1e-6, 0.0))
    vs.add("density_pink_db", 10.0 * np.log10(np.maximum(d_pink, 1e-300)), unit="dBFS/Hz",
           description="density of x_pink per bin", axis="freq_hz",
           tolerance=g.db_tol(1e-6, -250.0))
    return vs


def gen_multires_density() -> g.VectorSet:
    fs = 12000.0
    ppo = 24
    fifo = 16
    n_max = mr.longest_window(fs)
    ns = [n_max >> b for b in range(mr.BANDS)]
    hops = [mr.display_hop(n, fs) for n in ns]
    length = ns[0] + (fifo - 1) * hops[0] + 500
    rng = np.random.default_rng(0x0AC2_D352)
    # Bin-centred in band 0 and below its anchor, where a column reads one bin.
    k_sine = 36
    f_sine = k_sine * fs / ns[0]
    a_sine = 0.5
    t = np.arange(length) / fs
    x_a = SIGMA * rng.standard_normal(length) + a_sine * np.sin(2.0 * np.pi * f_sine * t)
    x_pink = pink(length, fs, rng)

    k = mr.kappa(mr.DESIGN_PPO)
    anchors = np.array([2.0 * k * fs / n for n in ns])
    enbw = hann_enbw(ns[0])

    k_min = -7 * ppo
    k_max = int(np.floor(ppo * np.log2(fs / 2.0 / 1000.0) - 0.5))
    ks = np.arange(k_min, k_max + 1)
    freqs = 1000.0 * 2.0 ** (ks / ppo)
    half = 2.0 ** (0.5 / ppo)

    def weights(f: float) -> list[tuple[int, float]]:
        if f <= anchors[0]:
            return [(0, 1.0)]
        if f >= anchors[-1]:
            return [(mr.BANDS - 1, 1.0)]
        b = int(np.max(np.nonzero(anchors < f)[0]))
        wt = min(max(f / anchors[b] - 1.0, 0.0), 1.0)
        return [(b, 1.0 - wt), (b + 1, wt)]

    def columns(x: np.ndarray) -> np.ndarray:
        tone = []
        for n, hop in zip(ns, hops):
            w = g.periodic_hann(n)
            s1 = np.sum(w)
            fold = g.one_sided_factor(n)
            ends = [n + j * hop for j in range((len(x) - n) // hop + 1)][-fifo:]
            p = [fold * np.abs(np.fft.rfft(w * x[e - n:e])) ** 2 for e in ends]
            tone.append(np.mean(p, axis=0) * 2.0 / (s1 * s1))
        out = []
        for f in freqs:
            p = 0.0
            bw = 0.0
            for b, wt in weights(f):
                n = ns[b]
                df = fs / n
                bins = n // 2 + 1
                lo, hi = f / half, f * half
                first = min(int(np.ceil(lo / df)), bins)
                end = min(int(np.ceil(hi / df)), bins)
                if end <= first:
                    kk = min(int(np.round(f / df)), bins - 1)
                    first, end = kk, kk + 1
                p += wt * float(np.mean(tone[b][first:end]))
                bw += wt * enbw * df
            out.append(p / bw)
        return np.array(out)

    bw = np.array([sum(wt * enbw * fs / ns[b] for b, wt in weights(f)) for f in freqs])
    d_a = columns(x_a)
    d_pink = columns(x_pink)

    vs = g.VectorSet(
        name="multires_density",
        description=(
            "Multi-resolution power spectral density at 12 kHz: six full-rate Hann bands "
            "(8192 ... 256 points), FIFO of 16 frames per band, 1/24-octave columns, each "
            "band's mean bin over the column blended in power and divided by the column's "
            "noise bandwidth: white noise plus a 52.7 Hz sine, and pink noise."
        ),
        function="gen_multires_density",
        parameters={
            "fs_hz": fs,
            "ppo": ppo,
            "fifo_frames": fifo,
            "windows": ns,
            "hops": hops,
            "signal_a": f"{SIGMA} N(0,1) + {a_sine} sin(2 pi {f_sine} t)",
            "signal_pink": f"{SIGMA} N(0,1) shaped by sqrt({F_REF}/f) over the whole signal",
            "seed": "numpy default_rng(0x0AC2D352), white first, then the pink's white",
            "anchor": "A_b = 2 kappa(48) fs / N_b, kappa(p) = 1/(2^(1/2p) - 2^(-1/2p))",
            "blend": "A_b < f < 2A_b: (1-w) P_b + w P_{b+1}, w = f/A_b - 1",
            "column": "mean tone power over bins k with ceil(lo/df) <= k < ceil(hi/df); "
                      "none: k = round(f/df)",
            "density": "blend / B(f), B(f) = sum_b w_b ENBW fs / N_b",
            "white_density": "4 sigma^2 / fs",
            "pink_density": "4 sigma^2 / fs * 1000 / f",
        },
        references=["docs/design/multires-spectrum.md", "crates/ac2-core/src/multires.rs"],
        scalars={
            "fs_hz": fs,
            "ppo": ppo,
            "fifo_frames": fifo,
            "sine_hz": f_sine,
            "sine_level_db": float(10.0 * np.log10(a_sine ** 2)),
            "white_density_db": white_density_db(fs),
        },
    )
    vs.add("x_a", x_a, unit="FS", description="white noise + sine")
    vs.add("x_pink", x_pink, unit="FS", description="pink noise")
    vs.add("hops", np.array(hops, dtype=float), unit="samples",
           description="hop of each band, longest window first")
    vs.add("freq_hz", freqs, unit="Hz", description="column centres",
           tolerance=g.lin_tol(1e-9, 1e-12))
    vs.add("noise_bandwidth_hz", bw, unit="Hz", description="noise bandwidth per column",
           axis="freq_hz", tolerance=g.lin_tol(0.0, 1e-12))
    vs.add("density_a_db", 10.0 * np.log10(d_a), unit="dBFS/Hz",
           description="density of x_a per column", axis="freq_hz",
           tolerance=g.lin_tol(1e-6, 0.0))
    vs.add("density_pink_db", 10.0 * np.log10(d_pink), unit="dBFS/Hz",
           description="density of x_pink per column", axis="freq_hz",
           tolerance=g.lin_tol(1e-6, 0.0))
    vs.add("pink_expected_db", pink_density_db(freqs, fs), unit="dBFS/Hz",
           description="analytic density of the pink noise", axis="freq_hz")
    return vs


GENERATORS = [gen_spectrum_density, gen_multires_density]
