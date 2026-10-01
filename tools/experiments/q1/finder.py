"""Reference prototype of the Q1 delay finder (PLAN.md §5.2, docs/design/q1-delay-finder.md).

The design note is normative; this file is the executable evidence that its numbers work.
Plain numpy, written for clarity rather than speed.

Sign convention: positive delay = measurement late, meas[i] ~ (h * ref)[i - D]. Delays are
absolute: D = (meas stream index) - (ref stream index) of the same source sample.

Two passes over the raw, unaligned pair:

1. Acquisition: tiled regularised H1 over the whole signed search range. Each tile
   pre-shifts the reference by an integer D0 (from sample indices), accumulates Welch
   spectra of Hann segments zero-padded to 2*Nseg, forms H = Gxy / (Gxx + eps*mean_band Gxx),
   band-limits, and inverts to an analytic IR whose lags |delta| <= Nseg/8 are kept.
2. Refinement: one tile at the strongest acquisition peak with Nseg2 = 2*Nseg and 75 %
   overlap. With the dominant arrival explained, its floor is set by noise rather than by
   unexplained signal, so candidate levels, the first-arrival decision and the confidence
   are taken from it.
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field

import numpy as np


# --------------------------------------------------------------------------------------
# Configuration
# --------------------------------------------------------------------------------------


@dataclass(frozen=True)
class Band:
    name: str
    f_lo: float  # -6 dB edge, Hz
    f_hi: float  # -6 dB edge, Hz (clipped so the taper ends at or below Nyquist)
    taper_oct: float  # raised-cosine width (octaves) centred on each edge
    seg_s: float  # acquisition segment (seconds; power of two in samples)
    obs_s: float  # default measurement observation (seconds)
    tol_s: float  # accuracy target when accepted (seconds)
    min_excited_frac: float  # fraction of the band's octave span that must be excited
    merge_max: float  # lobe misfit above which the pick is a merged lobe


BANDS = {
    "full": Band("full", 2000.0, 16000.0, 1.0, 4096 / 48000, 0.25, 1 / 48000, 0.5, 0.10),
    "mid": Band("mid", 300.0, 3000.0, 1.0, 8192 / 48000, 0.5, 5e-5, 0.5, 0.05),
    "sub": Band("sub", 20.0, 120.0, 1.0, 32768 / 48000, 4.0, 1e-4, 0.5, 0.03),
}
# Periodicity is a property of the excitation, not of the analysis band: it is checked over
# the whole audio band, where a repeat is seen with the most independent samples.
PERIOD_BAND = Band("period", 40.0, 16000.0, 1.0, 4096 / 48000, 0.0, 0.0, 0.0, 0.0)


@dataclass(frozen=True)
class Params:
    threshold_db: float = -12.0  # first significant arrival (decision 1a)
    list_depth_db: float = -20.0  # candidates are tracked down to this level
    eps: float = 0.01  # regularisation, fraction of the in-band mean Gxx
    p_fa: float = 1e-3  # false-peak probability per window for the detection floor
    psr_min_db: float = 15.0  # refinement: strongest over detection floor (|T| + 3 dB)
    psr_acq_min_db: float = 10.0  # acquisition: strongest over its detection floor
    borderline_db: float = 2.0  # +- band around the threshold that makes a pick ambiguous
    mismatch_noise_k: float = 3.0  # ... or k x the floor-to-peak ratio, whichever is larger
    close_k: float = 2.0  # another candidate within close_k pulse widths of the pick ...
    close_depth_db: float = -20.0  # ... and at least this strong -> ambiguous
    sidelobe_margin_db: float = 6.0  # a candidate must beat the pulse-model skirt by this
    band_snr_min_db: float = -10.0  # coherent/incoherent in band at the strongest alignment
    periodic_db: float = -10.0  # whitened ref autocorrelation peak that marks periodicity
    tail_s: float = 1.0  # T: response tail allowance for periodic excitation (PLAN §5.4)
    estimator: str = "h1"  # "h1" (regularised) or "phat"
    max_listed: int = 3
    edge_guard: int = 2  # samples: a pick this close to a search edge is refused
    refine: bool = True
    shape_test: bool = True
    floor_excl_k: float = 3.0  # lags this many pulse widths around a peak are not floor
    deblend_k: float = 6.0  # neighbours within this many pulse widths are deblended
    u_coef: float = 0.35  # sigma_tau = u_coef * width * sigma_noise / peak
    u_k: float = 2.5  # refuse when u_k * sigma_tau exceeds the band tolerance


@dataclass
class Candidate:
    delay: float  # fractional absolute delay, samples
    delay_int: int
    level_db: float  # relative to the strongest
    phase_deg: float  # analytic phase at the peak; 0 = positive polarity, 180 = inverted
    mismatch: float  # lobe shape misfit vs the pulse model (relative RMS)
    refined: bool  # inside the refinement window
    uncertainty: float = float("nan")  # 1-sigma timing uncertainty, samples
    noise_ratio: float = float("nan")  # sigma_n / |A|


@dataclass
class Result:
    status: str  # "accepted" | "ambiguous" | "no_estimate"
    reasons: list[str] = field(default_factory=list)
    delay: float | None = None  # first significant arrival (fractional samples)
    delay_int: int | None = None
    strongest: Candidate | None = None
    pick: Candidate | None = None
    listed: list[Candidate] = field(default_factory=list)
    candidates: list[Candidate] = field(default_factory=list)
    psr_db: float = float("nan")
    psr_acq_db: float = float("nan")
    band_snr_db: float = float("nan")
    excited_frac: float = float("nan")
    pulse_width: float = float("nan")
    nominal_width: float = float("nan")
    period: int | None = None
    window: tuple[int, int] | None = None  # first refinement window
    estimator: str = "h1"
    lags: np.ndarray | None = None
    env: np.ndarray | None = None


# --------------------------------------------------------------------------------------
# Helpers
# --------------------------------------------------------------------------------------


def seg_len(band: Band, fs: float) -> int:
    return 1 << int(round(math.log2(band.seg_s * fs)))


def band_hi(band: Band, fs: float) -> float:
    return min(band.f_hi, 0.5 * fs * 2.0 ** (-band.taper_oct / 2.0))


def band_weights(freqs: np.ndarray, band: Band, fs: float) -> np.ndarray:
    """Zero-phase band weighting: 1 inside, raised cosine in log2(f) across each -6 dB edge."""
    f_lo, f_hi, w = band.f_lo, band_hi(band, fs), band.taper_oct
    out = np.zeros_like(freqs)
    pos = freqs > 0
    lf = np.log2(np.where(pos, freqs, 1.0))

    def ramp(x):
        u = np.clip(x / w + 0.5, 0.0, 1.0)
        return 0.5 - 0.5 * np.cos(np.pi * u)

    v = ramp(lf - math.log2(f_lo)) * ramp(math.log2(f_hi) - lf)
    out[pos] = v[pos]
    return out


def periodic_hann(n: int) -> np.ndarray:
    return 0.5 - 0.5 * np.cos(2.0 * np.pi * np.arange(n) / n)


def window_overlap(w: np.ndarray, delta) -> np.ndarray:
    """rho(delta) = sum w[n] w[n+delta] / sum w^2."""
    n = len(w)
    ww = np.fft.irfft(np.abs(np.fft.rfft(w, 2 * n)) ** 2, 2 * n)
    return ww[np.asarray(delta) % (2 * n)] / ww[0]


def analytic_ir(hk: np.ndarray, nfft: int) -> np.ndarray:
    """Complex analytic IR from one-sided bins hk (len nfft/2+1)."""
    a = np.zeros(nfft, dtype=np.complex128)
    a[0] = hk[0]
    a[1 : nfft // 2] = 2.0 * hk[1 : nfft // 2]
    a[nfft // 2] = hk[nfft // 2]
    return np.fft.ifft(a)


def ir_norm(B: np.ndarray, nfft: int) -> float:
    """Envelope peak of a unit pure delay through the band weighting."""
    return float((B[0] + 2.0 * np.sum(B[1 : nfft // 2]) + B[nfft // 2]) / nfft)


def halfwidth(env: np.ndarray, i: int, direction: int, level: float) -> float:
    n = len(env)
    j = i
    while 0 <= j + direction < n and env[j + direction] >= level:
        j += direction
    k = j + direction
    if not (0 <= k < n):
        return float(abs(j - i))
    frac = (env[j] - level) / (env[j] - env[k]) if env[j] != env[k] else 0.0
    return abs(j - i) + frac


def parabolic(y0: float, y1: float, y2: float) -> float:
    den = y0 - 2.0 * y1 + y2
    if den >= 0.0:
        return 0.0
    return float(np.clip(0.5 * (y0 - y2) / den, -0.5, 0.5))


def segment_starts(n_avail: int, nseg: int, hop: int) -> list[int]:
    if n_avail < nseg:
        return []
    k = (n_avail - nseg) // hop + 1
    used = (k - 1) * hop + nseg
    off = (n_avail - used) // 2  # centre the grid in the block
    return [off + i * hop for i in range(k)]


# --------------------------------------------------------------------------------------
# Regularised H1 tile
# --------------------------------------------------------------------------------------


@dataclass
class Grid:
    nseg: int
    hop: int
    nfft: int
    freqs: np.ndarray
    B: np.ndarray
    w: np.ndarray
    starts: list[int]
    Y: list[np.ndarray]
    norm: float


def make_grid(meas, fs, band: Band, nseg: int, hop: int) -> Grid | None:
    nfft = 2 * nseg  # zero padding: lags up to +-Nseg are linear, never circular
    w = periodic_hann(nseg)
    freqs = np.fft.rfftfreq(nfft, 1.0 / fs)
    B = band_weights(freqs, band, fs)
    starts = segment_starts(len(meas), nseg, hop)
    if not starts:
        return None
    Y = [np.fft.rfft(w * meas[s : s + nseg], nfft) for s in starts]
    return Grid(nseg, hop, nfft, freqs, B, w, starts, Y, ir_norm(B, nfft))


def tile(g: Grid, ref, ref_start, meas_start, d0: int, p: Params):
    """Accumulate Welch spectra with the ref pre-shifted by d0; return (analytic IR, gxx, K)."""
    gxy = np.zeros(len(g.freqs), dtype=np.complex128)
    gxx = np.zeros(len(g.freqs))
    k = 0
    for s, Y in zip(g.starts, g.Y):
        r0 = meas_start + s - d0 - ref_start
        if r0 < 0 or r0 + g.nseg > len(ref):
            continue  # the ref block does not cover this segment at this lag
        X = np.fft.rfft(g.w * ref[r0 : r0 + g.nseg], g.nfft)
        gxy += np.conj(X) * Y
        gxx += np.abs(X) ** 2
        k += 1
    if k == 0:
        return None, gxx, 0
    if p.estimator == "h1":
        mean_b = np.sum(g.B * gxx) / np.sum(g.B)
        H = gxy / (gxx + p.eps * mean_b)
    elif p.estimator == "phat":
        mag = np.abs(gxy)
        H = gxy / np.where(mag > 0, mag, 1.0)
    else:
        raise ValueError(p.estimator)
    return analytic_ir(g.B * H, g.nfft) / g.norm, gxx, k


def ir_over(g: Grid, ref, ref_start, meas_start, d0: int, deltas: np.ndarray, p: Params):
    a, gxx, k = tile(g, ref, ref_start, meas_start, d0, p)
    if a is None:
        return None, gxx, 0
    return a[deltas % g.nfft] / window_overlap(g.w, deltas), gxx, k


def pulse_model(g: Grid, gxx: np.ndarray, p: Params, flat: bool = False):
    """Spectrum B*W of the expected single-arrival pulse, its envelope (peak 1 at index c)
    and its -6 dB width. W = Gxx/(Gxx + eps*mean) is the regularisation shrink."""
    B = g.B
    if flat or p.estimator == "phat":
        W = np.ones_like(B)
    else:
        mean_b = np.sum(B * gxx) / np.sum(B)
        W = gxx / (gxx + p.eps * mean_b)
    a = analytic_ir(B * W, g.nfft)
    e = np.abs(np.fft.fftshift(a))
    e /= e.max()
    c = g.nfft // 2
    width = halfwidth(e, c, -1, 0.5) + halfwidth(e, c, +1, 0.5)
    return B * W, e, c, width, W


def excited_fraction(g: Grid, W: np.ndarray, band: Band, fs: float) -> float:
    """Fraction of the band's octave span (between -6 dB edges) where W >= 0.5."""
    f = g.freqs
    hi = band_hi(band, fs)
    inside = (f >= band.f_lo) & (f <= hi) & (f > 0)
    if not np.any(inside):
        return 0.0
    oct_per_bin = (f[1] - f[0]) / (np.where(f > 0, f, 1.0) * math.log(2.0))
    return float(np.sum(oct_per_bin[inside & (W >= 0.5)]) / math.log2(hi / band.f_lo))


OS = 16  # oversampling of the pulse model for the lobe fit


def pulse_oversampled(pw: np.ndarray, nfft: int, half_span: float):
    """Complex analytic pulse model on a grid of 1/OS samples over +-half_span, peak 1
    (spectrum zero-padded OS-fold, one inverse FFT)."""
    big = nfft * OS
    a = np.zeros(big, dtype=np.complex128)
    a[0] = pw[0]
    a[1 : nfft // 2] = 2.0 * pw[1 : nfft // 2]
    full = np.fft.ifft(a)
    half = int(math.ceil(half_span * OS))
    n = np.arange(-half, half + 1)
    m = full[n % big]
    return n / OS, m / np.abs(m).max()


def model_at(model, x):
    t, m = model
    return (np.interp(x, t, m.real, left=0.0, right=0.0)
            + 1j * np.interp(x, t, m.imag, left=0.0, right=0.0))


def lobe_fit(ev: np.ndarray, k: np.ndarray, model, reach: float) -> tuple[float, float]:
    """Fit the pulse-model envelope (shift tau + amplitude) to envelope samples ev at
    integer offsets k around a candidate. Returns (tau, relative RMS misfit).

    The fit uses the whole lobe down to -20 dB of the model, a far better timing estimate
    than a 3-point vertex when the lobe is hundreds of samples wide."""
    if not np.all(np.isfinite(ev)):
        return 0.0, float("inf")
    t, m = model
    mabs = np.abs(m)

    def misfit(tau):
        mv = np.interp(k - tau, t, mabs, left=0.0, right=0.0)
        sel = mv >= 0.1
        if np.count_nonzero(sel) < 3:
            return float("inf")
        e_, m_ = ev[sel], mv[sel]
        amp = np.dot(e_, m_) / np.dot(m_, m_)
        return float(np.linalg.norm(e_ - amp * m_) / (amp * np.linalg.norm(m_)))

    # coarse grid over +-reach, then a fine grid around the best point, then a vertex
    centre, step = 0.0, reach / 12.0
    for _ in range(3):
        taus = centre + step * np.arange(-12, 13)
        f = np.array([misfit(x) for x in taus])
        i = int(np.argmin(f))
        centre = float(taus[i])
        if step <= 1.0 / 64:
            break
        step = max(step / 8.0, 1.0 / 64)
    if 0 < i < len(taus) - 1 and np.all(np.isfinite(f[i - 1 : i + 2])):
        tau = taus[i] + parabolic(-f[i - 1], -f[i], -f[i + 1]) * (taus[1] - taus[0])
        return float(tau), misfit(tau)
    return float(taus[i]), float(f[i])


def deblend(hcx: np.ndarray, js: list, taus: list, model, width: float, reach: float,
            iters: int = 2):
    """Refine candidate times and complex amplitudes jointly (index coordinates).

    Each candidate's lobe is fitted after subtracting the modelled pulses (complex, so
    polarity and interference are represented) of every other candidate within `reach`,
    because a neighbour's skirt otherwise pulls a weaker arrival's envelope peak."""
    half = max(2, int(math.ceil(1.5 * width)))
    k = np.arange(-half, half + 1)
    taus = list(taus)
    amps = [complex(hcx[j] / model_at(model, j - t)) for j, t in zip(js, taus)]
    mism = [0.0] * len(js)
    for _ in range(iters):
        for a, j in enumerate(js):
            ok = (j + k >= 0) & (j + k < len(hcx))
            kk = k[ok]
            r = hcx[j + kk].copy()
            for b in range(len(js)):
                if b != a and abs(taus[b] - taus[a]) < reach:
                    r -= amps[b] * model_at(model, j + kk - taus[b])
            reach_fit = max(1.5, 0.05 * width)
            tau, mm = lobe_fit(np.abs(r), kk, model, reach_fit)
            if abs(tau) < reach_fit:
                taus[a] = j + tau
            i0 = int(np.argmin(np.abs(kk - (taus[a] - j))))
            amps[a] = complex(r[i0] / model_at(model, j + kk[i0] - taus[a]))
            mism[a] = mm
    return taus, amps, mism


def detect_period(ref, fs, p: Params, min_lag: int = 256):
    """Whitened, band-limited autocorrelation of the ref block -> (period or None, level dB)."""
    n = len(ref)
    nfft = 1 << int(math.ceil(math.log2(2 * n)))
    R = np.fft.rfft(ref, nfft)
    freqs = np.fft.rfftfreq(nfft, 1.0 / fs)
    B = band_weights(freqs, PERIOD_BAND, fs)
    g = np.abs(R) ** 2
    k = max(1, nfft // 4096)  # smooth |R|^2 to the excitation shape
    gs = np.convolve(g, np.ones(2 * k + 1) / (2 * k + 1), mode="same")
    mean_b = np.sum(B * gs) / np.sum(B)
    ac = np.abs(analytic_ir(B * g / (gs + p.eps * mean_b), nfft))
    if ac[0] <= 0:
        return None, -np.inf
    lags = np.arange(nfft)
    max_lag = n - max(2048, n // 8)  # a repeat needs this much overlap to be seen
    sel = (lags >= min_lag) & (lags <= max_lag)
    if not np.any(sel):
        return None, -np.inf
    norm = ac[sel] / ac[0] * n / (n - lags[sel])
    i = int(np.argmax(norm))
    lvl = 20 * math.log10(max(norm[i], 1e-300))
    return (int(lags[sel][i]) if lvl >= p.periodic_db else None), lvl


def band_snr(ref, ref_start, meas, meas_start, delay: int, fs, band: Band, nseg: int):
    """Coherent-to-incoherent power ratio in the band with the pair aligned at `delay`."""
    w = periodic_hann(nseg)
    freqs = np.fft.rfftfreq(nseg, 1.0 / fs)
    B = band_weights(freqs, band, fs)
    sxx = np.zeros(len(freqs))
    syy = np.zeros(len(freqs))
    sxy = np.zeros(len(freqs), dtype=np.complex128)
    k = 0
    for s in segment_starts(len(meas), nseg, nseg // 2):
        r0 = meas_start + s - delay - ref_start
        if r0 < 0 or r0 + nseg > len(ref):
            continue
        X = np.fft.rfft(w * ref[r0 : r0 + nseg])
        Y = np.fft.rfft(w * meas[s : s + nseg])
        sxx += np.abs(X) ** 2
        syy += np.abs(Y) ** 2
        sxy += np.conj(X) * Y
        k += 1
    if k < 2:
        return float("nan")
    coh = np.abs(sxy) ** 2 / np.maximum(sxx * syy, 1e-300)
    num = np.sum(B * syy * coh)
    den = np.sum(B * syy * (1.0 - coh))
    if num <= 0:
        return -np.inf
    return 10.0 * math.log10(num / max(den, 1e-300 * num))


def detection_floor(env: np.ndarray, n_lags: int, b_eff: float, fs: float, p: Params,
                    width: float | None = None):
    """Median envelope and the level that noise maxima exceed with probability p_fa.

    For a Rayleigh envelope, median = sigma*sqrt(2 ln 2) and P(e > a) = exp(-a^2/2sigma^2),
    so over n independent lags the p_fa level is median * sqrt(ln(n/p_fa) / ln 2).
    With `width`, lags within 1.5 pulse widths of anything above that level are excluded
    and the median is taken once more, so wide lobes (sub band) do not inflate the floor."""
    n_indep = max(n_lags / fs * b_eff, 1.0)
    k = math.sqrt(math.log(n_indep / p.p_fa) / math.log(2.0))
    med = float(np.median(env))
    if width:
        h = int(math.ceil(p.floor_excl_k * width))
        sig = np.convolve((env > med * k).astype(float), np.ones(2 * h + 1), mode="same") > 0
        if np.count_nonzero(~sig) >= len(env) // 4:
            med = float(np.median(env[~sig]))
    return med, med * k


def local_maxima(e: np.ndarray, level: float) -> np.ndarray:
    m = np.zeros(len(e), dtype=bool)
    if len(e) >= 3:
        m[1:-1] = (e[1:-1] >= e[:-2]) & (e[1:-1] > e[2:])
    if len(e) >= 2:
        m[0] = e[0] > e[1]
        m[-1] = e[-1] > e[-2]
    return np.flatnonzero(m & (e >= level))


# --------------------------------------------------------------------------------------
# Finder
# --------------------------------------------------------------------------------------


def find_delay(ref, ref_start: int, meas, meas_start: int, fs: float, band: Band | str,
               search: tuple[int, int], p: Params = Params(),
               excitation_period: int | None = None, keep_env: bool = False) -> Result:
    band = BANDS[band] if isinstance(band, str) else band
    ref = np.asarray(ref, dtype=np.float64)
    meas = np.asarray(meas, dtype=np.float64)
    d_min, d_max = int(search[0]), int(search[1])
    res = Result(status="no_estimate", estimator=p.estimator)

    if not np.any(ref):
        res.reasons.append("no_reference")
    if not np.any(meas):
        res.reasons.append("no_signal")
    if res.reasons:
        return res

    nseg1 = seg_len(band, fs)
    nseg2 = 2 * nseg1 if p.refine else nseg1
    if len(meas) < nseg2:
        res.reasons.append("observation_too_short")
        return res

    # ---- pass 1: acquisition over the whole search range --------------------------------
    g1 = make_grid(meas, fs, band, nseg1, nseg1 // 2)
    step = nseg1 // 4
    half = step // 2
    lags = np.arange(d_min, d_max + 1)
    hcx = np.full(len(lags), np.nan + 0j)
    kseg = np.zeros(len(lags), dtype=int)
    gxx1 = np.zeros(len(g1.freqs))
    for d0 in range(d_min + half, d_max + step, step):
        dl = np.arange(-half, step - half)
        dd = d0 + dl
        sel = (dd >= d_min) & (dd <= d_max)
        a, gxx, k = ir_over(g1, ref, ref_start, meas_start, d0, dl[sel], p)
        if a is None:
            continue
        hcx[dd[sel] - d_min] = a
        kseg[dd[sel] - d_min] = k
        gxx1 += gxx
    # lags seen by fewer than half the segments are outside the usable search range
    valid = kseg >= max(1, (kseg.max() + 1) // 2)
    if not np.any(valid):
        res.reasons.append("insufficient_overlap")
        return res
    env = np.where(valid, np.abs(hcx), 0.0)

    pw1, ep1, c1, width1, W1 = pulse_model(g1, gxx1, p)
    b_eff1 = np.sum(pw1) ** 2 / np.sum(pw1 ** 2) * g1.freqs[1]
    med1, det1 = detection_floor(env[valid], int(np.count_nonzero(valid)), b_eff1, fs, p,
                                 width1)
    i_s1 = int(np.argmax(env))
    res.psr_acq_db = 20.0 * math.log10(env[i_s1] / det1) if det1 > 0 else float("inf")
    if not env[i_s1] > 0:
        res.reasons.append("no_signal")
        return res

    # ---- pass 2: refinement tiles ---------------------------------------------------------
    # First at the strongest acquisition peak; then, if the rule pick lies outside every
    # refinement window, once more at the pick, so the decisive arrival always has
    # refinement-quality evidence.
    det = np.full(len(lags), det1)
    med = np.full(len(lags), med1)
    in_win = np.zeros(len(lags), dtype=bool) if p.refine else valid.copy()
    model_src = {"pw": pw1, "ep": ep1, "cp": c1, "width": width1, "W": W1, "g": g1}
    windows: list[tuple[int, int]] = [] if p.refine else [(d_min, d_max)]
    g2 = make_grid(meas, fs, band, nseg2, nseg2 // 4) if p.refine else None

    def refine_at(d0: int) -> None:
        r2 = nseg2 // 8
        dl = np.arange(-r2, r2 + 1)
        sel = (d0 + dl >= d_min) & (d0 + dl <= d_max)
        a2, gxx2, _ = ir_over(g2, ref, ref_start, meas_start, d0, dl[sel], p)
        if a2 is None:
            return
        idx = d0 + dl[sel] - d_min
        hcx[idx] = a2
        env[idx] = np.abs(a2)
        if not windows:  # the pulse model comes from the first refinement tile
            pw_, ep_, cp_, width_, W_ = pulse_model(g2, gxx2, p)
            model_src.update(pw=pw_, ep=ep_, cp=cp_, width=width_, W=W_, g=g2)
        pw_ = model_src["pw"]
        b_eff2 = np.sum(pw_) ** 2 / np.sum(pw_ ** 2) * g2.freqs[1]
        med2, det2 = detection_floor(env[idx], len(idx), b_eff2, fs, p, model_src["width"])
        det[idx] = det2
        med[idx] = med2
        in_win[idx] = True
        windows.append((int(lags[idx[0]]), int(lags[idx[-1]])))

    if p.refine:
        refine_at(int(lags[i_s1]))
    pw, ep, cp, width, W, gq = (model_src[k] for k in ("pw", "ep", "cp", "width", "W", "g"))
    res.window = windows[0] if windows else None
    res.pulse_width = width
    res.nominal_width = pulse_model(gq, np.ones_like(gq.freqs), p, flat=True)[3]
    res.excited_frac = excited_fraction(gq, W, band, fs)

    # ---- checks that refuse regardless of the IR -----------------------------------------
    if res.excited_frac < band.min_excited_frac:
        res.reasons.append("insufficient_excitation")
    per = excitation_period
    if per is None:
        per, _ = detect_period(ref, fs, p)
    res.period = per
    if per is not None and per <= (d_max - d_min) + p.tail_s * fs:
        res.reasons.append("periodic_excitation_too_short")

    # ---- candidates -----------------------------------------------------------------------
    reach = p.deblend_k * width
    model = pulse_oversampled(pw, gq.nfft, reach + 1.5 * width + 3)
    margin = 10 ** (p.sidelobe_margin_db / 20.0)

    def candidates():
        e_max = float(env.max())
        floor_lvl = np.maximum(det, e_max * 10 ** (p.list_depth_db / 20.0))
        idx = [j for j in local_maxima(env, 0.0) if env[j] >= floor_lvl[j] and valid[j]]
        kept: list[int] = []
        for j in sorted(idx, key=lambda j: -env[j]):
            ok = True
            for s_ in kept:
                dl = int(j - s_)
                if abs(dl) < width:  # inside a stronger candidate's main lobe
                    ok = False
                    break
                k = cp + dl
                side = ep[k] if 0 <= k < len(ep) else 0.0
                if env[j] <= env[s_] * side * margin:  # explained by its pulse skirt
                    ok = False
                    break
            if ok:
                kept.append(int(j))
        taus = []
        for j in kept:
            frac = 0.0
            if 0 < j < len(env) - 1 and env[j - 1] > 0 and env[j + 1] > 0:
                frac = parabolic(math.log(env[j - 1]), math.log(env[j]), math.log(env[j + 1]))
            taus.append(j + frac)  # index coordinates; lag = d_min + index
        amps = [complex(hcx[j]) for j in kept]
        mism = [0.0] * len(kept)
        win_js = [a_ for a_, j in enumerate(kept) if in_win[j]]
        if p.shape_test and win_js:
            t2, a2, m2 = deblend(hcx, [kept[a_] for a_ in win_js], [taus[a_] for a_ in win_js],
                                 model, width, reach)
            for a_, t_, am, mmv in zip(win_js, t2, a2, m2):
                taus[a_], amps[a_], mism[a_] = t_, am, mmv
        lv = [abs(amps[a_]) if (p.shape_test and in_win[j]) else float(env[j])
              for a_, j in enumerate(kept)]
        l_max = max(lv) if lv else e_max
        out = []
        for a_, j in enumerate(kept):
            out.append(Candidate(
                delay=float(d_min + taus[a_]), delay_int=int(round(d_min + taus[a_])),
                level_db=20.0 * math.log10(max(lv[a_], 1e-300) / l_max),
                phase_deg=float(np.degrees(np.angle(amps[a_]))),
                mismatch=mism[a_], refined=bool(in_win[j]),
                # Rayleigh envelope: median = 1.177 sigma
                # interpolation bias (~0.1 sample) bounds it from below at high SNR
                uncertainty=max(p.u_coef * width * (med[j] / 1.1774) / max(lv[a_], 1e-300),
                                0.1),
                noise_ratio=(med[j] / 1.1774) / max(lv[a_], 1e-300),
            ))
        out.sort(key=lambda c: c.delay)
        return out

    cands = candidates()
    if p.refine and cands:
        first = [c for c in cands if c.level_db >= p.threshold_db][0]
        if not first.refined:
            refine_at(int(round(first.delay)))
            cands = candidates()
    if keep_env:
        res.lags, res.env = lags, env.copy()

    e_max = float(env.max())
    i_max = int(np.argmax(env))
    det_s = float(det[i_max])
    res.psr_db = 20.0 * math.log10(e_max / det_s) if det_s > 0 else float("inf")
    if res.psr_acq_db < p.psr_acq_min_db or res.psr_db < p.psr_min_db:
        res.reasons.append("low_psr")
    res.candidates = cands
    if not cands:
        if "low_psr" not in res.reasons:
            res.reasons.append("low_psr")
        return res
    strongest = max(cands, key=lambda c: c.level_db)
    res.strongest = strongest
    pick = [c for c in cands if c.level_db >= p.threshold_db][0]
    res.pick = pick
    res.delay, res.delay_int = pick.delay, pick.delay_int

    if p.u_k * pick.uncertainty > band.tol_s * fs:
        res.reasons.append("low_precision")
    if pick.delay_int - d_min < p.edge_guard or d_max - strongest.delay_int < p.edge_guard:
        res.reasons.append("peak_at_search_edge")
    snr = band_snr(ref, ref_start, meas, meas_start, strongest.delay_int, fs, band, nseg1)
    res.band_snr_db = snr
    if not (snr >= p.band_snr_min_db):
        res.reasons.append("low_band_snr")
    if res.reasons:
        return res

    # ---- ambiguity --------------------------------------------------------------------------
    amb = []
    clear = [c for c in cands if c.level_db >= p.threshold_db + p.borderline_db]
    pick_clear = clear[0]
    unsure = [c for c in cands
              if p.threshold_db - p.borderline_db <= c.level_db < p.threshold_db + p.borderline_db
              and c.delay < pick_clear.delay]
    if unsure:
        amb.append("borderline_level")
    close = [c for c in cands if c is not pick and abs(c.delay - pick.delay) < p.close_k * width
             and c.level_db >= p.close_depth_db]
    if close:
        amb.append("close_arrivals")
    if pick.mismatch > max(band.merge_max, p.mismatch_noise_k * pick.noise_ratio):
        amb.append("merged_lobe")
    if not pick.refined:
        amb.append("outside_refinement")
    res.reasons = amb
    res.status = "ambiguous" if amb else "accepted"

    listed = [pick]
    for c in [pick_clear, strongest] + sorted(unsure + close, key=lambda c: -c.level_db):
        if c not in listed:
            listed.append(c)
    res.listed = listed[: p.max_listed]
    return res


# --------------------------------------------------------------------------------------
# Tracking
# --------------------------------------------------------------------------------------


@dataclass
class TrackState:
    held: int | None = None
    pending: tuple[int, int] | None = None  # (delay_int, end index of its meas window)


def track_step(state: TrackState, res: Result, win_start: int, win_end: int,
               agree: int = 1) -> int | None:
    """Move the held delay only when an accepted result from a window sharing no samples
    with the pending one agrees with it within `agree` samples. Ambiguous or refused
    results clear the pending candidate. Returns the new held delay when it moves."""
    if res.status != "accepted":
        state.pending = None
        return None
    d = res.delay_int
    if state.pending is None:
        state.pending = (d, win_end)
        return None
    pd, pend = state.pending
    if win_start < pend:
        return None  # overlapping windows are the same audio read twice
    if abs(d - pd) <= agree:
        state.pending = None
        if state.held != d:
            state.held = d
            return d
        return None
    state.pending = (d, win_end)
    return None
