"""Pure numpy analysis: the independent opinions the suite compares ac2 and REW against.

Nothing here touches files or devices; every function is tested on synthetic signals
(`tests/test_dsp.py`)."""
from __future__ import annotations

import math

import numpy as np

# ---------------------------------------------------------------- frequency-domain bands


def log_centres(f_lo: float, f_hi: float, ppo: int = 48) -> np.ndarray:
    k0, k1 = math.ceil(ppo * math.log2(f_lo / 1000)), math.floor(ppo * math.log2(f_hi / 1000))
    return 1000.0 * 2.0 ** (np.arange(k0, k1 + 1) / ppo)


def band_mean(f: np.ndarray, H: np.ndarray, centres: np.ndarray, frac: float = 1 / 48) -> np.ndarray:
    """Complex mean of H over each band [c·2^(−frac/2), c·2^(frac/2)); the nearest point when
    a band holds none (a linear grid is sparser than 1/48 octave at low frequencies)."""
    lo, hi = centres * 2 ** (-frac / 2), centres * 2 ** (frac / 2)
    a = np.searchsorted(f, lo)
    b = np.searchsorted(f, hi)
    out = np.empty(len(centres), dtype=complex)
    cs = np.concatenate([[0], np.cumsum(H)])
    for i, (x, y) in enumerate(zip(a, b)):
        if y > x:
            out[i] = (cs[y] - cs[x]) / (y - x)
        else:
            out[i] = H[min(np.searchsorted(f, centres[i]), len(f) - 1)]
    return out


def cross_spectrum_bands(meas: np.ndarray, ref: np.ndarray, fs: float, centres: np.ndarray,
                         frac: float = 1 / 48) -> tuple[np.ndarray, np.ndarray]:
    """Direct transfer function meas/ref from one whole recording: in each band
    H = Σ M·R* / Σ |R|², and the band's coherence |Σ M·R*|² / (Σ|M|² Σ|R|²). No window, no
    deconvolution: the truth for a stationary path that both inputs saw."""
    M, R = np.fft.rfft(meas), np.fft.rfft(ref)
    fx = np.fft.rfftfreq(len(meas), 1 / fs)
    lo, hi = centres * 2 ** (-frac / 2), centres * 2 ** (frac / 2)
    a, b = np.searchsorted(fx, lo), np.searchsorted(fx, hi)
    cmr = np.concatenate([[0], np.cumsum(M * np.conj(R))])
    crr = np.concatenate([[0], np.cumsum(np.abs(R) ** 2)])
    cmm = np.concatenate([[0], np.cumsum(np.abs(M) ** 2)])
    H = np.full(len(centres), np.nan + 0j)
    coh = np.full(len(centres), np.nan)
    for i, (x, y) in enumerate(zip(a, b)):
        if y <= x:
            continue
        smr, srr, smm = cmr[y] - cmr[x], crr[y] - crr[x], cmm[y] - cmm[x]
        if srr > 0:
            H[i] = smr / srr
            coh[i] = abs(smr) ** 2 / (srr * smm) if smm > 0 else np.nan
    return H, coh


def db(x) -> np.ndarray:
    return 20 * np.log10(np.maximum(np.abs(x), 1e-300))


def wrap_deg(p) -> np.ndarray:
    return (np.asarray(p) + 180.0) % 360.0 - 180.0


# ---------------------------------------------------------------- delay and impulse response


def delay_from_phase(f: np.ndarray, H: np.ndarray, f_lo: float, f_hi: float) -> float:
    """Pure delay (s) from a least-squares line through the unwrapped phase over [f_lo, f_hi].
    The line keeps its own intercept: the path's filters add a near-constant phase there."""
    m = (f >= f_lo) & (f <= f_hi) & np.isfinite(H)
    ph = np.unwrap(np.angle(H[m]))
    A = np.vstack([2 * np.pi * f[m], np.ones(m.sum())]).T
    return float(-np.linalg.lstsq(A, ph, rcond=None)[0][0])


def parabolic_vertex(y0: float, y1: float, y2: float) -> float:
    d = y0 - 2 * y1 + y2
    return 0.0 if d == 0 else 0.5 * (y0 - y2) / d


def fractional_peak(h: np.ndarray, up: int = 16, half: int = 16) -> tuple[float, float]:
    """Sub-sample position and signed value of |h|'s peak: band-limited interpolation by
    zero-padding the DFT of h[d−half … d+half] ×up, then a parabola on the fine grid (a
    parabola on raw samples is biased by up to ~0.05 sample for a sinc-shaped peak)."""
    d = int(np.argmax(np.abs(h)))
    n = 2 * half + 1
    idx = (d - half + np.arange(n)) % len(h)
    seg = h[idx]
    S = np.fft.fft(seg)
    N = n * up
    P = np.zeros(N, dtype=complex)
    k = n // 2
    P[:k + 1] = S[:k + 1]
    P[-k:] = S[-k:]
    fine = np.real(np.fft.ifft(P)) * up
    j = int(np.argmax(np.abs(fine)))
    a = np.abs(fine)
    v = parabolic_vertex(a[j - 1], a[j], a[(j + 1) % N]) if 0 < j < N - 1 else 0.0
    pos = d - half + (j + v) / up
    return float(pos), float(fine[j])


def bandlimit(h: np.ndarray, fs: float, f_hi: float, slope_oct: float = 1 / 6) -> np.ndarray:
    """Low-pass in the frequency domain with a raised-cosine edge (zero phase)."""
    Hh = np.fft.rfft(h)
    f = np.fft.rfftfreq(len(h), 1 / fs)
    lo = f_hi * 2 ** (-slope_oct)
    x = np.clip(np.log2(np.maximum(f, 1e-9) / lo) / slope_oct, 0, 1)
    return np.fft.irfft(Hh * (0.5 + 0.5 * np.cos(np.pi * x)), len(h))


def envelope(h: np.ndarray) -> np.ndarray:
    n = len(h)
    H = np.fft.fft(h)
    g = np.zeros(n)
    g[0] = 1
    g[1:(n + 1) // 2] = 2
    if n % 2 == 0:
        g[n // 2] = 1
    return np.abs(np.fft.ifft(H * g))


def etc_cells(t: np.ndarray, env: np.ndarray, t0: float, dt: float, n: int) -> np.ndarray:
    """Envelope maximum in each cell [t0 + i·dt, t0 + (i+1)·dt) (ac2's IR block buckets)."""
    out = np.full(n, np.nan)
    i = np.floor((t - t0) / dt).astype(int)
    ok = (i >= 0) & (i < n)
    np.fmax.at(out, i[ok], env[ok])
    return out


# ---------------------------------------------------------------- group delay


def gd_central(f: np.ndarray, phase_rad: np.ndarray) -> np.ndarray:
    """ac2's displayed group delay: −Δφ/Δω between the two neighbouring columns."""
    ph = np.unwrap(phase_rad)
    g = np.full(len(f), np.nan)
    g[1:-1] = -(ph[2:] - ph[:-2]) / (2 * np.pi * (f[2:] - f[:-2]))
    return g


def gd_slope(f: np.ndarray, phase_rad: np.ndarray, fc: np.ndarray, half_oct: float = 1 / 12,
             weights: np.ndarray | None = None, min_points: int = 3) -> np.ndarray:
    """Group delay as the slope of a (weighted) least-squares line through the unwrapped phase
    against frequency over fc·2^(±half_oct): a derivative with a stated bandwidth."""
    ph = np.unwrap(phase_rad)
    w = np.ones_like(f) if weights is None else weights
    out = np.full(len(fc), np.nan)
    for i, c in enumerate(np.atleast_1d(fc)):
        m = (f >= c * 2 ** -half_oct) & (f <= c * 2 ** half_oct) & np.isfinite(ph)
        if m.sum() < min_points:
            continue
        x, y, ww = 2 * np.pi * f[m], ph[m], w[m]
        xm, ym = np.average(x, weights=ww), np.average(y, weights=ww)
        out[i] = -np.sum(ww * (x - xm) * (y - ym)) / np.sum(ww * (x - xm) ** 2)
    return out


# ---------------------------------------------------------------- steady sines


def sine_phasor(x: np.ndarray, f: float, fs: float) -> complex:
    """Least-squares a·cos + b·sin + dc over a whole number of periods; returns the phasor
    of the sine part (phase re cos at sample 0)."""
    per = fs / f
    n = int(np.floor(len(x) / per) * per) or len(x)
    t = np.arange(n) / fs
    A = np.vstack([np.cos(2 * np.pi * f * t), np.sin(2 * np.pi * f * t), np.ones(n)]).T
    a, b, _ = np.linalg.lstsq(A, x[:n], rcond=None)[0]
    return complex(a, -b)


MAINLOBE_BINS = 3  # Blackman: main lobe ±3 bins


def _blackman_spec(x: np.ndarray, fs: float):
    w = np.blackman(len(x))
    X = np.abs(np.fft.rfft(x * w)) ** 2
    return X, fs / len(x), w


def _lobe_power(X: np.ndarray, df: float, f: float) -> float:
    k = int(round(f / df))
    if k + MAINLOBE_BINS >= len(X):
        return np.nan
    return float(X[max(k - MAINLOBE_BINS, 0):k + MAINLOBE_BINS + 1].sum())


def sine_harmonics(x: np.ndarray, f0: float, fs: float, kmax: int = 5) -> dict:
    """Fundamental level (dBFS, sine convention) and H2..Hkmax in dBr: each the power summed
    over the Blackman main lobe (±3 bins) at k·f0, so a tone reads its whole power and noise
    reads its power in the same bandwidth, which is what the floor is compared with."""
    X, df, w = _blackman_spec(x, fs)
    p1 = _lobe_power(X, df, f0)
    # a sine of amplitude A gives a lobe power of (A·Σw/2)² · Σ(window lobe)²/peak² — calibrate
    # on the window itself so the level is exact for any length
    ref = _lobe_power(_blackman_spec(np.cos(2 * np.pi * f0 * np.arange(len(x)) / fs), fs)[0], df, f0)
    level = 10 * np.log10(p1 / ref)  # dB re a full-scale sine
    hs = {k: (10 * np.log10(_lobe_power(X, df, k * f0) / p1) if k * f0 < fs / 2 - 4 * df else np.nan)
          for k in range(2, kmax + 1)}
    return {"f0": f0, "level_dbfs": float(level), "p1": p1, "h_dbr": hs, "bin_hz": df}


def harmonic_floor(noise: np.ndarray, f0: float, fs: float, n: int, p1: float, kmax: int = 5) -> dict:
    """Floor of each harmonic in dBr: the noise recording cut into segments of the tone's
    length n, the same window and lobe as `sine_harmonics`, powers averaged over segments
    (one segment swings several dB at low frequency)."""
    segs = [noise[i:i + n] for i in range(0, len(noise) - n + 1, max(n // 2, 1))]
    if not segs:
        return {k: np.nan for k in range(2, kmax + 1)}
    out = {}
    for k in range(2, kmax + 1):
        ps = []
        for s in segs:
            X, df, _ = _blackman_spec(s, fs)
            ps.append(_lobe_power(X, df, k * f0))
        out[k] = float(10 * np.log10(np.nanmean(ps) / p1))
    return out


def classify(value_db: float, floor_db: float, margin_db: float) -> dict:
    """A reading counts as a value only at floor + margin or above; otherwise it is an upper
    bound, never a value (the noise in the band is as large as what is read)."""
    if not np.isfinite(value_db):
        return {"kind": "none", "value": None, "floor": _f(floor_db), "margin": None}
    if not np.isfinite(floor_db):
        return {"kind": "value", "value": float(value_db), "floor": None, "margin": None}
    m = value_db - floor_db
    if m >= margin_db:
        return {"kind": "value", "value": float(value_db), "floor": float(floor_db), "margin": float(m)}
    return {"kind": "bound", "value": None, "bound": float(max(value_db, floor_db)), "floor": float(floor_db),
            "margin": float(m), "shortfall": float(margin_db - m)}


def _f(x):
    return float(x) if x is not None and np.isfinite(x) else None


def avoid_mains(f: float, seconds: float, mains: float = 50.0, kmax: int = 5, guard_bins: int = 2,
                max_rel: float = 0.06) -> float:
    """Nearest frequency to f whose harmonics 2..kmax all stay clear of every mains multiple
    by the window's main lobe plus `guard_bins` (bins of 1/seconds Hz)."""
    clear = (MAINLOBE_BINS + guard_bins) / seconds
    for step in range(0, int(max_rel / 0.0005) + 1):
        for sgn in (1, -1):
            g = f * (1 + sgn * step * 0.0005)
            ok = all(abs(k * g - mains * round(k * g / mains)) > clear for k in range(2, kmax + 1))
            if ok:
                return round(g, 3)
    return f


def mains_lines(x: np.ndarray, fs: float, mains: float = 50.0, n_max: int = 40, thr_db: float = 10.0) -> list[dict]:
    """Mains-family lines in a noise recording: multiples of `mains` whose bin stands
    `thr_db` above the median of ±5 % around it."""
    X = np.abs(np.fft.rfft(x * np.hanning(len(x)))) ** 2
    df = fs / len(x)
    out = []
    for n in range(1, n_max + 1):
        f = n * mains
        k = int(round(f / df))
        if k + 3 >= len(X):
            break
        lo, hi = int(k * 0.95), int(k * 1.05) + 1
        med = np.median(X[lo:hi])
        pk = X[k - 2:k + 3].max()
        if med > 0 and 10 * np.log10(pk / med) >= thr_db:
            out.append({"hz": f, "above_db": float(10 * np.log10(pk / med))})
    return out


# ---------------------------------------------------------------- sweeps (an ac2 replica)


def deconvolve(ref: np.ndarray, meas: np.ndarray, eps_rel: float = 1e-6) -> np.ndarray:
    """h = IFFT(M·R* / (|R|² + ε)), circular over 2× the length (harmonic IRs land before t=0)."""
    n = 2 * (1 << int(np.ceil(np.log2(len(ref)))))
    R, M = np.fft.rfft(ref, n), np.fft.rfft(meas, n)
    eps = eps_rel * np.max(np.abs(R) ** 2)
    return np.fft.irfft(M * np.conj(R) / (np.abs(R) ** 2 + eps), n)


def _taper(n, rise, fall):
    i = np.arange(n)
    w = np.ones(n)
    r = i < rise
    w[r] *= 0.5 - 0.5 * np.cos(np.pi * (i[r] + 0.5) / rise)
    fe = n - 1 - i
    q = fe < fall
    w[q] *= 0.5 - 0.5 * np.cos(np.pi * (fe[q] + 0.5) / fall)
    return w


def _band_power(s, bin_hz, f, octv, min_hz):
    half = 2 ** (octv / 2)
    lo, hi = f / half, f * half
    if hi - lo < min_hz:
        c = 0.5 * (lo + hi)
        lo, hi = max(c - 0.5 * min_hz, 0), c + 0.5 * min_hz
    a, b = int(np.ceil(lo / bin_hz)), min(int(np.floor(hi / bin_hz)), len(s) - 1)
    return s[a:b + 1].mean() if b >= a else np.nan


def sweep_harmonics(h: np.ndarray, d: int, fs: float, L: float, freqs: np.ndarray, kmax: int = 5,
                    pre: float = 0.0083229, post: float = 0.0916771, noise_h: np.ndarray | None = None,
                    noise_offsets: int = 8) -> dict:
    """ac2's harmonic windows on a deconvolved exponential sweep (Farina): harmonic k sits
    L·ln k before the linear IR. Returns Hk in dBr per frequency and, given the deconvolved
    noise-only recording `noise_h`, the floor from `noise_offsets` windows power-averaged —
    the cross-check of ac2's single-window floor."""
    pre_n, post_n = max(round(pre * fs), 1), max(round(post * fs), 1)
    wl = pre_n + post_n
    w = _taper(wl, pre_n, max(post_n // 5, 1))
    nw = 4 * (1 << int(np.ceil(np.log2(wl))))
    bw = fs / nw
    N = len(h)
    min_hz = 3 / (wl / fs)

    def spec(x, start):
        return np.abs(np.fft.rfft(x[(start + np.arange(wl)) % len(x)] * w, nw)) ** 2

    sk = {k: spec(h, d - round(L * np.log(k) * fs) - pre_n) for k in range(1, kmax + 1)}
    p1 = np.array([_band_power(sk[1], bw, f, 1 / 24, min_hz) for f in freqs])
    res = {"p1": p1, "h": {}, "floor": {}}
    for k in range(2, kmax + 1):
        pk = np.array([_band_power(sk[k], bw, k * f, 1 / 24, min_hz) for f in freqs])
        res["h"][k] = 10 * np.log10(pk / p1)
        if noise_h is not None:
            Nn = len(noise_h)
            starts = [int(Nn * (j + 0.5) / noise_offsets) for j in range(noise_offsets)]
            pn = np.mean([[_band_power(spec(noise_h, s0), bw, k * f, 1 / 24, min_hz) for f in freqs]
                          for s0 in starts], axis=0)
            res["floor"][k] = 10 * np.log10(pn / p1)
    del N
    return res


# ---------------------------------------------------------------- levels, weighting, bands


def weighting_db(f: np.ndarray, kind: str) -> np.ndarray:
    """IEC 61672-1 A and C weightings (analytic), Z = 0."""
    f = np.maximum(np.asarray(f, float), 1e-6)
    f2 = f * f
    c1, c2, c3, c4 = 20.598997 ** 2, 107.65265 ** 2, 737.86223 ** 2, 12194.217 ** 2
    if kind.upper() == "Z":
        return np.zeros_like(f)
    if kind.upper() == "A":
        ra = c4 * f2 * f2 / ((f2 + c1) * np.sqrt((f2 + c2) * (f2 + c3)) * (f2 + c4))
        return 20 * np.log10(ra) + 2.0
    if kind.upper() == "C":
        rc = c4 * f2 / ((f2 + c1) * (f2 + c4))
        return 20 * np.log10(rc) + 0.062
    raise ValueError(kind)


def mic_correction_db(curve: list[list[float]] | None, f: np.ndarray, f_norm: float = 1000.0) -> np.ndarray:
    """ac2's mic-curve correction (q7 §5): the file's response interpolated linearly in
    log-frequency, flat outside its range, shifted to 0 dB at f_norm; the correction is its
    negative."""
    if not curve:
        return np.zeros_like(np.asarray(f, float))
    c = np.asarray(curve, float)
    lf = np.log(c[:, 0])

    def at(x):
        return np.interp(np.log(np.maximum(x, 1e-9)), lf, c[:, 1], left=c[0, 1], right=c[-1, 1])

    return -(at(np.asarray(f, float)) - at(f_norm))


def leq_spectrum(x: np.ndarray, fs: float, sensitivity_db: float, weighting: str = "Z",
                 curve: list | None = None, f_lo: float = 0.0) -> float:
    """Leq over the whole recording in dB SPL: power spectrum of the segment, weighted, mic
    curve corrected, summed. dBFS in the full-scale-sine convention (rms·√2), plus the
    calibration's dB SPL at 0 dBFS."""
    X = np.fft.rfft(x - np.mean(x))
    f = np.fft.rfftfreq(len(x), 1 / fs)
    P = np.abs(X) ** 2
    P[1:-1] *= 2
    g = 10 ** ((weighting_db(f, weighting) + mic_correction_db(curve, f)) / 10)
    g[f < f_lo] = 0
    ms = np.sum(P * g) / len(x) ** 2
    return float(10 * np.log10(max(ms, 1e-300) * 2) + sensitivity_db)


def third_octave_levels(x: np.ndarray, fs: float, sensitivity_db: float, curve: list | None = None,
                        f_lo: float = 20.0, f_hi: float = 20000.0) -> tuple[np.ndarray, np.ndarray]:
    """Band levels in dB SPL over base-10 third-octave bands (ideal edges), from one FFT of
    the whole segment."""
    X = np.fft.rfft(x - np.mean(x))
    f = np.fft.rfftfreq(len(x), 1 / fs)
    P = np.abs(X) ** 2
    P[1:-1] *= 2
    P = P * 10 ** (mic_correction_db(curve, f) / 10)
    n = np.arange(round(10 * np.log10(f_lo)), round(10 * np.log10(f_hi)) + 1)
    fc = 10 ** (n / 10)
    out = []
    for c in fc:
        m = (f >= c * 10 ** -0.05) & (f < c * 10 ** 0.05)
        ms = P[m].sum() / len(x) ** 2
        out.append(10 * np.log10(max(ms, 1e-300) * 2) + sensitivity_db)
    return fc, np.array(out)


# ---------------------------------------------------------------- room parameters (third opinion)


def _band_filter(h, fs, fc, frac):
    H = np.fft.rfft(h, 2 * len(h))
    f = np.fft.rfftfreq(2 * len(h), 1 / fs)
    lo, hi = fc * 2 ** (-frac / 2), fc * 2 ** (frac / 2)
    # raised-cosine skirts a sixth of the band wide: approximate class-1 filters, stated so
    sk = frac / 6
    x = np.log2(np.maximum(f, 1e-9))
    g = np.clip((x - np.log2(lo) + sk / 2) / sk, 0, 1) * np.clip((np.log2(hi) + sk / 2 - x) / sk, 0, 1)
    g = 0.5 - 0.5 * np.cos(np.pi * g)
    return np.fft.irfft(H * g)[:len(h)]


def room_parameters(h: np.ndarray, fs: float, onset: int, fc: float | None = None, frac: float = 1.0) -> dict:
    """ISO 3382-1 style EDT, T20, T30, C50, C80, D50 from one IR: Schroeder backward
    integral after subtracting the noise power estimated from the last tenth, truncated where
    the envelope meets the noise (a simple Lundeby-like point)."""
    x = h if fc is None else _band_filter(h, fs, fc, frac)
    e = x[onset:] ** 2
    if len(e) < fs * 0.05:
        return {}
    tail = e[int(len(e) * 0.9):]
    noise = tail.mean()
    sm = np.convolve(e, np.ones(int(0.01 * fs)) / int(0.01 * fs), mode="same")
    above = np.where(sm > 3 * noise)[0]
    trunc = above[-1] if len(above) else len(e) - 1
    ee = np.clip(e[:trunc] - noise, 0, None)
    sch = np.cumsum(ee[::-1])[::-1]
    if sch[0] <= 0:
        return {}
    L = 10 * np.log10(np.maximum(sch / sch[0], 1e-30))
    t = np.arange(len(L)) / fs

    def fit(a, b):
        m = (L <= a) & (L >= b)
        if m.sum() < 10 or L.min() > b:
            return None
        p = np.polyfit(t[m], L[m], 1)
        return float(-60 / p[0]) if p[0] < 0 else None

    def clar(ms):
        n = int(ms * fs / 1000)
        early, late = e[:n].sum(), e[n:trunc].sum()
        return float(10 * np.log10(early / late)) if late > 0 else None

    n50 = int(0.05 * fs)
    return {"edt": fit(0, -10), "t20": fit(-5, -25), "t30": fit(-5, -35), "c50": clar(50), "c80": clar(80),
            "d50": float(e[:n50].sum() / e[:trunc].sum()), "decay_range": float(-L[-1]) if len(L) else None,
            "truncation_s": trunc / fs}
