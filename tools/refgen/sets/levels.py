"""Golden vectors for level measurement: spectrum scalloping, FFT banding, IEC 61260-1
Butterworth band-pass responses and SPL exponential time weighting.

Conventions follow docs/design/q4-level-normalisation.md. Expected values come from numpy,
scipy and analytic formulas only.
"""

import pathlib
import sys

import numpy as np
import scipy.signal as sig

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import generate as g  # noqa: E402

G10 = 10.0 ** 0.3  # IEC 61260-1 base-10 octave ratio


def iec_centres(b: int, f_lo: float, f_hi: float) -> np.ndarray:
    """Exact IEC 61260-1 mid-band frequencies in [f_lo, f_hi] (Formulas 2 and 3)."""
    x = np.arange(-200, 200)
    if b % 2 == 1:
        f = 1000.0 * G10 ** (x / b)
    else:
        f = 1000.0 * G10 ** ((2 * x + 1) / (2 * b))
    return f[(f >= f_lo) & (f <= f_hi)]


def iec_edges(fm, b: int):
    half = G10 ** (1.0 / (2 * b))
    return fm / half, fm * half


def hft95(n: int) -> np.ndarray:
    """HFT95 flat-top, periodic (Heinzel, Rüdiger, Schilling 2002, Table/Appendix D)."""
    a = [1.0, 1.9383379, 1.3045202, 0.4028270, 0.0350665]
    z = 2.0 * np.pi * np.arange(n) / n
    return sum(((-1) ** k) * a[k] * np.cos(k * z) for k in range(len(a)))


# --------------------------------------------------------------------------------------
# Off-bin tone: scalloping per window
# --------------------------------------------------------------------------------------


def gen_spectrum_offbin_windows() -> g.VectorSet:
    fs = 48000.0
    n = 4096
    tone_bin = 1000.5  # half a bin off: worst-case scalloping
    f0 = tone_bin * fs / n
    t = np.arange(n) / fs
    x = np.sin(2.0 * np.pi * f0 * t)  # full-scale sine: 0 dBFS
    fold = g.one_sided_factor(n)

    windows = {
        "hann": g.periodic_hann(n),
        "blackmanharris4": sig.get_window("blackmanharris", n, fftbins=True),
        "flattop_hft95": hft95(n),
        "rectangular": np.ones(n),
    }
    bh = windows["blackmanharris4"]
    i = np.arange(n)
    z = 2 * np.pi * i / n
    bh_explicit = 0.35875 - 0.48829 * np.cos(z) + 0.14128 * np.cos(2 * z) - 0.01168 * np.cos(3 * z)
    assert np.allclose(bh, bh_explicit, rtol=0, atol=1e-15)

    scalars = {"fs_hz": fs, "n": n, "tone_bin": tone_bin, "tone_freq_hz": f0}
    vs = g.VectorSet(
        name="spectrum_offbin_windows",
        description=(
            "Full-scale sine half a bin off-centre, one frame, amplitude spectrum in dBFS for "
            "Hann, 4-term Blackman-Harris, HFT95 flat-top and rectangular periodic windows; "
            "the peak bin's shortfall from 0 dBFS is the window's scalloping loss."
        ),
        function="gen_spectrum_offbin_windows",
        parameters={
            "fs_hz": fs,
            "n": n,
            "tone": "x = sin(2 pi f0 t), f0 = 1000.5 * fs / n (peak 1.0 = 0 dBFS)",
            "windows": {
                "hann": "periodic, 0.5 - 0.5 cos(2 pi i/N)",
                "blackmanharris4": "scipy get_window('blackmanharris', fftbins=True)",
                "flattop_hft95": "periodic cosine sum 1, 1.9383379, 1.3045202, 0.4028270, "
                                 "0.0350665 (alternating signs)",
                "rectangular": "ones",
            },
            "amplitude_dbfs": "20 log10(sqrt(c_k) |X_k| / sum(w) * sqrt 2)",
            "scalloping_db": "-max_k amplitude_dbfs (includes the negligible image leakage)",
            "kernel_loss_db": "-20 log10(|sum_j w_j exp(-i pi j / N)| / sum(w))",
        },
        references=[
            "docs/design/q4-level-normalisation.md (scalloping and windows, decision 4c)",
            "G. Heinzel, A. Ruediger, R. Schilling, Spectrum and spectral density estimation "
            "by the DFT (2002), HFT95",
        ],
        scalars=scalars,
    )
    vs.add("x", x, unit="FS", description="input frame")
    for key, w in windows.items():
        s1 = np.sum(w)
        amp = np.sqrt(fold) * np.abs(np.fft.rfft(w * x)) / s1
        db = 20.0 * np.log10(amp * np.sqrt(2.0))
        kernel = np.abs(np.sum(w * np.exp(-1j * np.pi * i / n))) / s1
        scalars[f"{key}_scalloping_db"] = float(-np.max(db))
        scalars[f"{key}_kernel_loss_db"] = float(-20.0 * np.log10(kernel))
        assert abs(scalars[f"{key}_scalloping_db"] - scalars[f"{key}_kernel_loss_db"]) < 2e-3
        vs.add(f"{key}_amplitude_dbfs", db, unit="dBFS",
               description=f"amplitude spectrum, {key} window",
               tolerance=g.db_tol(1e-4, -200.0))
    return vs


# --------------------------------------------------------------------------------------
# FFT banding: fractional-bin band power and Parseval
# --------------------------------------------------------------------------------------


def gen_fft_banding_noise() -> g.VectorSet:
    fs = 48000.0
    n = 8192
    sigma = 0.1
    tone_f, tone_peak = 1234.5, 0.2
    seed = 0x0AC2_0011
    rng = np.random.default_rng(seed)
    t = np.arange(n) / fs
    x = sigma * rng.standard_normal(n) + tone_peak * np.sin(2 * np.pi * tone_f * t)

    w = g.periodic_hann(n)
    s2 = np.sum(w * w)
    fold = g.one_sided_factor(n)
    spec = np.fft.rfft(w * x)
    df = fs / n
    # Power carried by each bin (PSD_k * df).
    p_bin = fold * np.abs(spec) ** 2 / (n * s2)
    windowed_ms = float(np.sum((w * x) ** 2) / s2)
    assert abs(np.sum(p_bin) / windowed_ms - 1.0) < 1e-12  # Parseval

    # Cumulative power vs frequency, each bin's power spread uniformly over its interval
    # [(k - 1/2) df, (k + 1/2) df] clipped to [0, fs/2]; band power = C(hi) - C(lo).
    bounds = np.concatenate(([0.0], (np.arange(len(p_bin) - 1) + 0.5) * df, [fs / 2]))
    cum = np.concatenate(([0.0], np.cumsum(p_bin)))
    assert abs(cum[-1] - windowed_ms) < 1e-12 * windowed_ms

    def band_power(lo, hi):
        return float(np.interp(hi, bounds, cum) - np.interp(lo, bounds, cum))

    vs = g.VectorSet(
        name="fft_banding_noise",
        description=(
            "White noise plus an off-bin sine, one Hann frame; IEC base-10 1/1- and "
            "1/3-octave band power by fractional-bin integration of the PSD, with band status "
            "for bands under two bins wide."
        ),
        function="gen_fft_banding_noise",
        parameters={
            "fs_hz": fs,
            "n": n,
            "x": "sigma * default_rng(seed).standard_normal(n) + 0.2 sin(2 pi 1234.5 t)",
            "sigma": sigma,
            "seed": seed,
            "window": "hann, periodic",
            "bin_interval": "[(k - 1/2) df, (k + 1/2) df] clipped to [0, fs/2], df = fs/n",
            "band_power": ("integral of the piecewise-constant density: each bin's power "
                           "c_k |X_k|^2 / (n sum w^2) spread uniformly over its interval"),
            "band_range": "exact mid-band frequencies in [15, 21000] Hz",
            "edges": "f_m * G^(-+1/(2b)), G = 10^0.3",
            "status": "0 valid, 1 width < 2 df (power stored as 0), 2 upper edge > fs/2",
        },
        references=[
            "docs/design/q4-level-normalisation.md (FFT banding, Parseval test)",
            "IEC 61260-1:2014 5.2-5.4 (base-10 mid-band frequencies and band edges)",
        ],
        scalars={"fs_hz": fs, "n": n, "windowed_mean_square": windowed_ms},
    )
    vs.add("x", x, unit="FS", description="input frame")
    for key, b in (("oct", 1), ("third", 3)):
        fm = iec_centres(b, 15.0, 21000.0)
        lo, hi = iec_edges(fm, b)
        status = np.where(hi > fs / 2 * (1 + 1e-12), 2.0, np.where(hi - lo < 2 * df, 1.0, 0.0))
        power = np.array([band_power(a, c) if s == 0 else 0.0
                          for a, c, s in zip(lo, hi, status)])
        vs.add(f"{key}_centre_hz", fm, unit="Hz", description=f"1/{b}-octave mid-band",
               tolerance=g.lin_tol(0.0, 1e-12))
        vs.add(f"{key}_status", status, unit="1", description="band status code",
               tolerance=g.lin_tol(0.0, 0.0), axis=f"{key}_centre_hz")
        vs.add(f"{key}_band_power", power, unit="FS^2",
               description="band power (0 where status != 0)",
               tolerance=g.lin_tol(1e-18, 1e-9), axis=f"{key}_centre_hz")
    return vs


# --------------------------------------------------------------------------------------
# IEC 61260-1 Butterworth band-pass responses
# --------------------------------------------------------------------------------------

BANDS = [
    # (fs, b, centre selector (f_lo, f_hi) giving exactly one band, order)
    (48000.0, 1, (30.0, 33.0), 3),
    (48000.0, 1, (999.0, 1001.0), 3),
    (48000.0, 1, (7900.0, 8000.0), 3),
    (48000.0, 1, (15000.0, 16500.0), 4),
    (48000.0, 3, (19.0, 21.0), 3),
    (48000.0, 3, (999.0, 1001.0), 3),
    (48000.0, 3, (19000.0, 21000.0), 4),
    (44100.0, 3, (999.0, 1001.0), 3),
    (44100.0, 3, (15000.0, 16500.0), 4),
    (96000.0, 3, (19000.0, 21000.0), 3),
    (48000.0, 6, (1000.0, 1100.0), 3),
    (96000.0, 24, (19.5, 20.0), 3),
]


def gen_rta_butterworth_bands() -> g.VectorSet:
    spec_rows = []
    arrays = []
    for fs, b, (f_lo, f_hi), order in BANDS:
        fm = iec_centres(b, f_lo, f_hi)
        assert len(fm) == 1, (fs, b, fm)
        fm = float(fm[0])
        lo, hi = iec_edges(fm, b)
        sos = sig.butter(order, [lo, hi], btype="bandpass", fs=fs, output="sos")
        f = fm * G10 ** (np.linspace(-3.0, 3.0, 121) / b)
        f = f[f < 0.499 * fs]
        _, h = sig.sosfreqz(sos, worN=f, fs=fs)
        db = 20.0 * np.log10(np.abs(h))
        # -3 dB at the pre-warped edges; unit gain at the digital image of the analog centre.
        _, he = sig.sosfreqz(sos, worN=[lo, hi], fs=fs)
        assert np.allclose(20 * np.log10(np.abs(he)), -10 * np.log10(2), atol=1e-6)
        spec_rows.append([fs, b, lo, hi, order])
        arrays.append((f, db, fs, b, fm, order))

    vs = g.VectorSet(
        name="rta_butterworth_bands",
        description=(
            "Magnitude responses of digital Butterworth band-pass filters on IEC 61260-1 "
            "base-10 band edges (scipy.signal.butter, bilinear with pre-warped edges)."
        ),
        function="gen_rta_butterworth_bands",
        parameters={
            "design": ("scipy.signal.butter(order, [f_lower, f_upper], btype='bandpass', "
                       "fs=fs, output='sos')"),
            "edges": "f_m * G^(-+1/(2b)), G = 10^0.3",
            "frequencies": "f_m * G^(x/b), x = linspace(-3, 3, 121), kept below 0.499 fs",
            "band_spec_columns": ["fs_hz", "b", "f_lower_hz", "f_upper_hz", "order"],
        },
        references=[
            "IEC 61260-1:2014 (base-10 band edges, relative attenuation)",
            "scipy.signal.butter / sosfreqz",
        ],
    )
    vs.add("band_spec", np.array(spec_rows), unit="mixed",
           description="rows of (fs_hz, b, f_lower_hz, f_upper_hz, order)")
    for i, (f, db, fs, b, fm, order) in enumerate(arrays):
        vs.add(f"band{i}_freq_hz", f, unit="Hz",
               description=f"frequencies, fs={fs} 1/{b} fm={fm:.3f} order {order}")
        vs.add(f"band{i}_mag_db", db, unit="dB", description="20 log10 |H|",
               tolerance=g.db_tol(1e-6, -250.0), axis=f"band{i}_freq_hz")
    return vs


# --------------------------------------------------------------------------------------
# SPL: exponential time weighting toneburst responses
# --------------------------------------------------------------------------------------

TB_F_MS = [1000, 500, 200, 100, 50, 20, 10, 5, 2, 1, 0.5, 0.25]
TB_S_MS = [1000, 500, 200, 100, 50, 20, 10, 5, 2]


def gen_spl_toneburst() -> g.VectorSet:
    fs = 48000.0
    f0 = 4000.0
    amp = 0.5
    pre = int(0.01 * fs)
    steady_ms = amp * amp / 2.0

    def burst_response(tau, tb_ms):
        nb = int(round(tb_ms * 1e-3 * fs))
        assert abs(nb - tb_ms * 1e-3 * fs) < 1e-9
        post = int(5 * tau * fs)
        x = np.zeros(pre + nb + post)
        k = np.arange(nb)
        x[pre:pre + nb] = amp * np.sin(2 * np.pi * f0 * k / fs)
        alpha = 1.0 - np.exp(-1.0 / (fs * tau))
        ms = sig.lfilter([alpha], [1.0, -(1.0 - alpha)], x * x)
        return 10.0 * np.log10(np.max(ms) / steady_ms)

    resp_f = np.array([burst_response(0.125, tb) for tb in TB_F_MS])
    resp_s = np.array([burst_response(1.0, tb) for tb in TB_S_MS])
    eq7_f = 10 * np.log10(1 - np.exp(-np.array(TB_F_MS) * 1e-3 / 0.125))
    eq7_s = 10 * np.log10(1 - np.exp(-np.array(TB_S_MS) * 1e-3 / 1.0))

    vs = g.VectorSet(
        name="spl_toneburst",
        description=(
            "Maximum F and S time-weighted level of isolated 4 kHz tonebursts relative to the "
            "steady level, from a one-pole exponential average of the squared signal "
            "(Z weighting), plus IEC 61672-1 Formula 7."
        ),
        function="gen_spl_toneburst",
        parameters={
            "fs_hz": fs,
            "tone_hz": f0,
            "amplitude_fs": amp,
            "burst": ("x = amp sin(2 pi f0 k / fs), k = 0..round(Tb fs)-1, starting at a zero "
                      "crossing after 10 ms of silence; zeros afterwards"),
            "detector": ("ms[n] = ms[n-1] + alpha (x[n]^2 - ms[n-1]), "
                         "alpha = 1 - exp(-1/(fs tau)), ms[-1] = 0 "
                         "(scipy.signal.lfilter([alpha], [1, alpha - 1], x^2))"),
            "tau_s": {"F": 0.125, "S": 1.0},
            "response_db": "10 log10(max ms / (amp^2/2))",
            "eq7": "10 log10(1 - exp(-Tb / tau))",
        },
        references=[
            "IEC 61672-1:2013 5.8, 5.9, Table 4 and Formula 7",
        ],
    )
    vs.add("tb_f_ms", np.array(TB_F_MS, dtype=float), unit="ms",
           description="toneburst durations, F")
    vs.add("response_f_db", resp_f, unit="dB", description="LFmax - L, Z weighting",
           tolerance=g.lin_tol(1e-9, 0.0), axis="tb_f_ms")
    vs.add("eq7_f_db", eq7_f, unit="dB", description="Formula 7, tau = 0.125 s",
           axis="tb_f_ms")
    vs.add("tb_s_ms", np.array(TB_S_MS, dtype=float), unit="ms",
           description="toneburst durations, S")
    vs.add("response_s_db", resp_s, unit="dB", description="LSmax - L, Z weighting",
           tolerance=g.lin_tol(1e-9, 0.0), axis="tb_s_ms")
    vs.add("eq7_s_db", eq7_s, unit="dB", description="Formula 7, tau = 1 s",
           axis="tb_s_ms")
    return vs


TB_I_MS = [1000, 500, 200, 100, 50, 20, 10, 5, 2]
STEP_MS = [10, 35, 70, 125, 250, 500, 1000, 2000, 3000, 5000]
DECAY_MS = [100, 250, 500, 1000, 1500, 2000, 3000]
# Time constants: F and S average with one time constant; I averages with 35 ms and holds
# the average's peaks, falling with 1.5 s.
TAU = {"F": (0.125, 0.125), "S": (1.0, 1.0), "I": (0.035, 1.5)}


def time_weighted(x2: np.ndarray, fs: float, rise: float, fall: float) -> np.ndarray:
    """The detector on x^2: a one-pole average with `rise`; when `fall` differs, a peak
    follower after it that takes a rising average at once and otherwise falls with `fall`."""
    alpha = 1.0 - np.exp(-1.0 / (fs * rise))
    avg = sig.lfilter([alpha], [1.0, -(1.0 - alpha)], x2)
    if fall == rise:
        return avg
    d = np.exp(-1.0 / (fs * fall))
    out = np.empty_like(avg)
    ms = 0.0
    for n, a in enumerate(avg):
        ms = a if a >= ms else max(ms * d, a)
        out[n] = ms
    return out


def gen_spl_time_weighting() -> g.VectorSet:
    fs = 48000.0
    f0 = 4000.0
    amp = 0.5
    steady = amp * amp / 2.0
    pre = int(0.01 * fs)

    def burst_i(tb_ms):
        nb = int(round(tb_ms * 1e-3 * fs))
        post = int(0.2 * fs)
        x = np.zeros(pre + nb + post)
        k = np.arange(nb)
        x[pre:pre + nb] = amp * np.sin(2 * np.pi * f0 * k / fs)
        rise, fall = TAU["I"]
        return 10.0 * np.log10(np.max(time_weighted(x * x, fs, rise, fall)) / steady)

    # A step of the mean square from 0 to 1 (x = 1 from sample 0): the level `t` after the
    # step is read at sample round(t fs) - 1, the last of the first t seconds.
    n_step = int(STEP_MS[-1] * 1e-3 * fs)
    step_idx = np.array([int(round(t * 1e-3 * fs)) - 1 for t in STEP_MS])
    # The fall: 10 s of x = 1, then silence; read `t` after the last sample of the step.
    n_on = int(10 * fs)
    n_off = int(DECAY_MS[-1] * 1e-3 * fs)
    decay_idx = np.array([n_on - 1 + int(round(t * 1e-3 * fs)) for t in DECAY_MS])
    resp = {}
    for name, (rise, fall) in TAU.items():
        up = time_weighted(np.ones(n_step), fs, rise, fall)
        resp[f"step_{name.lower()}_db"] = 10.0 * np.log10(up[step_idx])
        x2 = np.concatenate([np.ones(n_on), np.zeros(n_off)])
        down = time_weighted(x2, fs, rise, fall)
        resp[f"decay_{name.lower()}_db"] = 10.0 * np.log10(down[decay_idx] / down[n_on - 1])

    vs = g.VectorSet(
        name="spl_time_weighting",
        description=(
            "Time weightings F, S and I of a sound level meter on the squared signal: the "
            "level after a step of the mean square, its fall after the signal stops, and the "
            "maximum I-weighted level of isolated 4 kHz tonebursts relative to the steady "
            "level (Z weighting)."
        ),
        function="gen_spl_time_weighting",
        parameters={
            "fs_hz": fs,
            "tone_hz": f0,
            "amplitude_fs": amp,
            "tau_s": {k: {"rise": r, "fall": f} for k, (r, f) in TAU.items()},
            "detector": ("avg[n] = avg[n-1] + alpha (x[n]^2 - avg[n-1]), "
                         "alpha = 1 - exp(-1/(fs rise)); F, S: ms = avg; I: ms[n] = avg[n] "
                         "if avg[n] >= ms[n-1], else max(ms[n-1] exp(-1/(fs fall)), avg[n])"),
            "step": ("x = 1 from sample 0; level 10 log10(ms) at sample round(t fs) - 1 "
                     "(analytically 10 log10(1 - exp(-t / rise)))"),
            "decay": ("x = 1 for 10 s, then 0; level at sample 10 fs - 1 + round(t fs) "
                      "relative to the level at the stop (analytically -10 lg(e) t / fall "
                      "= -4.343 t / fall dB)"),
            "burst": ("x = amp sin(2 pi f0 k / fs), k = 0..round(Tb fs)-1, after 10 ms of "
                      "silence, then 200 ms of silence"),
        },
        references=[
            "IEC 61672-1:2013 5.8 (F, S: 125 ms, 1 s; decay 34.7 and 4.3 dB/s)",
            "IEC 60651:1979 (I: 35 ms rise, 1.5 s fall; single-burst responses "
            "-3.6, -8.8, -12.6 dB at 20, 5, 2 ms)",
        ],
    )
    vs.add("step_ms", np.array(STEP_MS, dtype=float), unit="ms",
           description="time after the step")
    for name in ("f", "s", "i"):
        vs.add(f"step_{name}_db", resp[f"step_{name}_db"], unit="dB",
               description=f"{name.upper()} level after a step to 0 dB",
               tolerance=g.lin_tol(1e-9, 0.0), axis="step_ms")
    vs.add("decay_ms", np.array(DECAY_MS, dtype=float), unit="ms",
           description="time after the signal stops")
    for name in ("f", "s", "i"):
        vs.add(f"decay_{name}_db", resp[f"decay_{name}_db"], unit="dB",
               description=f"{name.upper()} level after the stop, relative to the stop",
               tolerance=g.lin_tol(1e-9, 0.0), axis="decay_ms")
    vs.add("tb_i_ms", np.array(TB_I_MS, dtype=float), unit="ms",
           description="toneburst durations, I")
    vs.add("response_i_db", np.array([burst_i(tb) for tb in TB_I_MS]), unit="dB",
           description="LImax - L, Z weighting",
           tolerance=g.lin_tol(1e-9, 0.0), axis="tb_i_ms")
    return vs


GENERATORS = [
    gen_spectrum_offbin_windows,
    gen_fft_banding_noise,
    gen_rta_butterworth_bands,
    gen_spl_toneburst,
    gen_spl_time_weighting,
]
