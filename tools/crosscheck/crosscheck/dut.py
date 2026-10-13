"""The digital DUT: a software device (`ac2-jack-dut`) with harmonics known exactly, so the
analysers' harmonic readings can be judged against an analytic truth instead of a hardware
path whose own distortion sits at its converters' floor.

    ref_out = ref_in + noise
    dut_out = post(poly(pre(dut_in))) + noise,   poly(u) = Σ c_k u^k

pre and post are cascades of biquads (direct form II transposed, a0 = 1,
y = b0·x + b1·x₁ + b2·x₂ − a1·y₁ − a2·y₂), applied in the order given.

Truth for a steady sine x = A·cos(2πft): the pre-filter turns it into a sine of amplitude
a = A·|G_pre(f)| (its phase does not matter to a static polynomial), poly(a·cos θ) has
harmonic amplitudes h_k that one period's FFT gives exactly (a polynomial of degree d has
no content above d, so N > 2d points do not alias), and the post-filter weights each by
|G_post(k·f)|. Hk in dBr is then 20·log10(h_k·|G_post(kf)| / (h_1·|G_post(f)|)).

Wiener versus Hammerstein: with a pre-filter ahead of the nonlinearity (a Wiener model) the
truth above is exact only for a steady sine. A swept sine reaches the polynomial through the
pre-filter's transient response: the sweep's harmonic at instantaneous frequency f follows
|G_pre(f)| only as far as the filter is quasi-static over the time the sweep spends near f
(the instantaneous-frequency approximation). Near the pre-filter's corner, and for fast
sweeps, a sweep may read a different Hk than the steady sine without either analyser being
wrong; the post-filter (Hammerstein part) has no such caveat. `sweep_analytic` is the sweep's
own truth: the first-order chirp term of the pre-filter and ac2's band-mean reading; the
report still marks the columns where the pre-filter is not flat.

numpy only: it runs on the rig (the run designs the coefficients and writes the truth table)
and in the analysis."""
from __future__ import annotations

import math

import numpy as np

KMAX = 5


# ---------------------------------------------------------------- filter design


def design(spec: dict, fs: float) -> list[float]:
    """[b0, b1, b2, a1, a2] (a0 = 1) for one spec: {"type": hp1|lp1|hp2|lp2, "hz": f, "q": Q}.

    First order: bilinear transform prewarped at the corner, so |H(f_c)| = 1/√2 exactly.
    Second order: the RBJ cookbook biquads, whose corner gain is Q (−3.01 dB at Q = 1/√2)."""
    kind, fc = spec["type"], float(spec["hz"])
    if not 0 < fc < fs / 2:
        raise ValueError(f"{kind} at {fc:g} Hz: outside 0 … fs/2 = {fs / 2:g}")
    if kind in ("hp1", "lp1"):
        k = math.tan(math.pi * fc / fs)
        a1 = (k - 1) / (k + 1)
        if kind == "lp1":
            g = k / (1 + k)
            return [g, g, 0.0, a1, 0.0]
        g = 1 / (1 + k)
        return [g, -g, 0.0, a1, 0.0]
    if kind in ("hp2", "lp2"):
        q = float(spec.get("q", 1 / math.sqrt(2)))
        w0 = 2 * math.pi * fc / fs
        cw, al = math.cos(w0), math.sin(w0) / (2 * q)
        a0 = 1 + al
        if kind == "lp2":
            b = [(1 - cw) / 2, 1 - cw, (1 - cw) / 2]
        else:
            b = [(1 + cw) / 2, -(1 + cw), (1 + cw) / 2]
        return [b[0] / a0, b[1] / a0, b[2] / a0, -2 * cw / a0, (1 - al) / a0]
    raise ValueError(f"unknown filter type {kind!r} (hp1, lp1, hp2, lp2)")


def design_chain(specs: list[dict] | None, fs: float) -> list[list[float]]:
    return [design(s, fs) for s in (specs or [])]


def response(chain: list[list[float]], f, fs: float) -> np.ndarray:
    """Complex response of a biquad cascade at f (Hz)."""
    f = np.asarray(f, dtype=float)
    z1 = np.exp(-2j * np.pi * f / fs)
    h = np.ones_like(z1)
    for b0, b1, b2, a1, a2 in chain:
        h = h * (b0 + b1 * z1 + b2 * z1 ** 2) / (1 + a1 * z1 + a2 * z1 ** 2)
    return h


def biquad(chain: list[list[float]], x: np.ndarray) -> np.ndarray:
    """The cascade on a signal, DF2T per sample exactly as the binary runs it (reference
    simulation for the tests: slow, a per-sample loop)."""
    y = np.asarray(x, dtype=float)
    for b0, b1, b2, a1, a2 in chain:
        out = np.empty_like(y)
        s1 = s2 = 0.0
        for i, v in enumerate(y):
            o = b0 * v + s1
            s1 = b1 * v - a1 * o + s2
            s2 = b2 * v - a2 * o
            out[i] = o
        y = out
    return y


def poly(c: list[float], u: np.ndarray) -> np.ndarray:
    return np.polynomial.polynomial.polyval(u, np.asarray(c, dtype=float))


# ---------------------------------------------------------------- the truth


def poly_harmonics(c: list[float], a, kmax: int = KMAX) -> np.ndarray:
    """|h_k| of poly(a·cos θ) for k = 0..kmax (h_0 the DC term, h_k peak amplitudes); for an
    array of amplitudes, one row per amplitude."""
    n = max(64, 4 * (len(c) + kmax))
    th = 2 * np.pi * np.arange(n) / n
    y = np.fft.rfft(poly(c, np.multiply.outer(a, np.cos(th))), axis=-1) / n
    h = np.abs(y[..., :kmax + 1]) * 2
    h[..., 0] /= 2
    return h


def analytic(f: float, amp: float, cfg: dict, fs: float, kmax: int = KMAX) -> dict[int, float]:
    """{k: Hk in dBr} for k = 2..kmax relative to the fundamental at the output, for a steady
    sine of peak amplitude `amp` at f into dut_in. `cfg` holds `poly`, `pre`, `post` as
    coefficient lists (`coefficients(...)`). A harmonic at or above fs/2 is left out: the
    binary aliases it, and no analyser reads it at k·f."""
    pre, post = cfg.get("pre", []), cfg.get("post", [])
    a = amp * abs(response(pre, [f], fs)[0])
    h = poly_harmonics(cfg["poly"], a, kmax)
    g = np.abs(response(post, f * np.arange(1, kmax + 1), fs))
    fund = h[1] * g[0]
    out = {}
    for k in range(2, kmax + 1):
        if k * f >= fs / 2:
            continue
        v = h[k] * g[k - 1]
        out[k] = float(20 * np.log10(v / fund)) if v > 0 else float("-inf")
    return out


def sweep_amplitude(f, amp: float, pre: list[list[float]], fs: float, rate_s: float) -> np.ndarray:
    """The amplitude an exponential sweep reaches the polynomial with at instantaneous
    frequency f, to first order in the sweep rate.

    A chirp x = Re e^{jθ(t)}, θ' = ω(t), through a filter G: expanding θ(t − τ) to second
    order in τ gives u = Re e^{jθ}·[G(ω) − (j/2)·ω'·G''(ω)] (G'' the second derivative in ω).
    An exponential sweep has ω' = ω/L (L the rate, s per neper of frequency), so in hertz the
    bracket is G(f) − j·f·G_ff(f)/(4π·L). The correction falls as 1/L: a sweep twice as slow
    sees half of it, a steady sine (L → ∞) none."""
    f = np.asarray(f, dtype=float)
    if not pre:
        return np.full_like(f, amp)
    h = 1e-3 * f
    g = response(pre, f, fs)
    gff = (response(pre, f + h, fs) - 2 * g + response(pre, f - h, fs)) / h ** 2
    return amp * np.abs(g - 1j * f * gff / (4 * np.pi * rate_s))


def sweep_analytic(f: float, amp: float, cfg: dict, fs: float, rate_s: float, window_s: float | None = None,
                   own_windows: dict[int, float] | None = None, kmax: int = KMAX) -> dict[int, float]:
    """{k: Hk in dBr} that an exponential sweep of rate `rate_s` reads at fundamental f, as
    ac2 defines the reading. Where the pre-filter is flat this is `analytic`.

    Two terms separate it from the steady sine's truth:
    - the sweep drives the polynomial with `sweep_amplitude`, not A·|G_pre(f)| (the linear
      response, deconvolved, is the exact G and has no such term);
    - given `window_s` (the harmonic window, s), the reading is ac2's band ratio: the mean
      power of harmonic k over k·f ± 1/48 octave, widened to 3 cells of the window
      (3/window_s Hz), over the fundamental's mean power across that band ÷ k. Where Hk
      changes fast with f, the band mean is not the value at f. `own_windows` {k: s} are the
      orders read in their own window where the shared band was widened (LF harmonics fine).

    Both matter where Hk is very sensitive to the polynomial's input amplitude: a polynomial
    whose orders cancel on one harmonic (T_5 leaking into H3 at r = a/a_design < 1 against
    T_3) turns the chirp term's hundredths of a dB into tenths."""
    pre, post = cfg.get("pre", []), cfg.get("post", [])
    out = {}
    for k in range(2, kmax + 1):
        if k * f >= fs / 2:
            continue
        if window_s is None:
            fund = np.array([f])
        else:
            half = 2 ** (1 / 48)
            lo, hi = k * f / half, k * f * half
            ws = window_s
            if own_windows and k in own_windows and 3 / window_s > hi - lo:
                ws = own_windows[k]
            if hi - lo < 3 / ws:
                c = 0.5 * (lo + hi)
                lo, hi = max(c - 1.5 / ws, 0.0), c + 1.5 / ws
            fund = np.linspace(lo, hi, 33) / k
        hk = poly_harmonics(cfg["poly"], sweep_amplitude(fund, amp, pre, fs, rate_s))[:, k]
        h1 = poly_harmonics(cfg["poly"], amp * np.abs(response(pre, fund, fs)))[:, 1]
        pk = np.mean((hk * np.abs(response(post, k * fund, fs))) ** 2)
        p1 = np.mean((h1 * np.abs(response(post, fund, fs))) ** 2)
        out[k] = float(10 * np.log10(pk / p1)) if pk > 0 else float("-inf")
    return out


def chebyshev_poly(h_dbr, amp: float) -> list[float]:
    """poly(u) = u + Σ r_k·a·T_k(u/a), r_k = 10^(Hk/20), for H2, H3, … given in dBr (None:
    absent): at input amplitude a exactly, T_k(cos θ) = cos kθ puts each term on its own
    harmonic, so the fundamental stays a and Hk is the target. Away from a the terms leak
    into lower harmonics of the same parity (T_k(r·cos θ) for r ≠ 1); `analytic` stays exact
    there, the targets just no longer hold."""
    cheb = np.polynomial.chebyshev
    t = np.zeros(len(h_dbr) + 2)
    t[1] = 1.0  # T_1(u/a)·a = u
    for k, h in enumerate(h_dbr, start=2):
        if h is not None:
            t[k] = 10 ** (float(h) / 20)
    c = cheb.cheb2poly(t) * amp  # a·Σ t_k T_k(x), x = u/a
    # The even T_k carry a constant: DC alone, no part of any h_k (k ≥ 1). Dropping it keeps
    # the DUT's output at rest at zero, as a noise-only recording expects.
    c[0] = 0.0
    return [float(v / amp ** j) for j, v in enumerate(c)]


def target_dbr(dut_cfg: dict, level_dbfs: float | None) -> list[float] | None:
    """A designed DUT's harmonic targets in dBr at the run's level: `harmonics_dbr` as given,
    or `harmonics_dbfs` (each harmonic's own level at the output, dBFS like the run's level)
    less the level, which keeps each harmonic at one distance above the DUT's fixed noise
    floor at every level."""
    if "harmonics_dbr" in dut_cfg:
        return [None if h is None else float(h) for h in dut_cfg["harmonics_dbr"]]
    if "harmonics_dbfs" in dut_cfg:
        if level_dbfs is None:
            raise ValueError("harmonics_dbfs needs the run's level")
        return [None if h is None else float(h) - float(level_dbfs) for h in dut_cfg["harmonics_dbfs"]]
    return None


def coefficients(dut_cfg: dict, fs: float, level_dbfs: float | None = None) -> dict:
    """The rig's [dut] table → the coefficient lists the binary is given. The polynomial is
    `poly` as written, or designed (`chebyshev_poly`) from `harmonics_dbr` / `harmonics_dbfs`
    at the peak amplitude of `level_dbfs`, the level the run plays into dut_in."""
    given = [k for k in ("poly", "harmonics_dbr", "harmonics_dbfs") if k in dut_cfg]
    if len(given) != 1:
        raise ValueError(f"[dut] needs exactly one of poly, harmonics_dbr, harmonics_dbfs (has {given or 'none'})")
    h = target_dbr(dut_cfg, level_dbfs)
    if h is None:
        poly_c = [float(x) for x in dut_cfg["poly"]]
    else:
        if level_dbfs is None:
            raise ValueError("a designed polynomial needs the run's level")
        poly_c = chebyshev_poly(h, 10 ** (float(level_dbfs) / 20))  # full-scale-sine dBFS: peak
    return {"poly": poly_c, "pre": design_chain(dut_cfg.get("pre"), fs),
            "post": design_chain(dut_cfg.get("post"), fs), "noise_dbfs": float(dut_cfg.get("noise_dbfs", -120.0))}


def _num(x: float) -> str:
    return repr(float(x))  # shortest round-trip form: the binary parses the same double


def command(dut_cfg: dict, coef: dict, binary: str) -> list[str]:
    """argv for ac2-jack-dut. `--opt=value` so a negative first coefficient is not taken
    for an option."""
    cmd = [binary, f"--name={dut_cfg.get('client', 'ac2-dut')}", "--poly=" + ",".join(map(_num, coef["poly"]))]
    for key in ("pre", "post"):
        for bq in coef[key]:
            cmd.append(f"--{key}=" + ",".join(map(_num, bq)))
    cmd.append(f"--noise-dbfs={_num(coef['noise_dbfs'])}")
    return cmd


def truth_table(freqs, amp: float, coef: dict, fs: float) -> list[dict]:
    return [{"f": float(f), "h_dbr": {str(k): v for k, v in analytic(float(f), amp, coef, fs).items()}}
            for f in freqs]


def peak_out(amp: float, coef: dict, fs: float, f: float = 1000.0) -> float:
    """Largest |output| for a steady sine of amplitude amp at f (one period, post-filter
    ignored: it passes the band the harmonics fall in nearly flat)."""
    a = amp * abs(response(coef["pre"], [f], fs)[0])
    th = np.linspace(0, 2 * np.pi, 4096, endpoint=False)
    return float(np.max(np.abs(poly(coef["poly"], a * np.cos(th)))))
