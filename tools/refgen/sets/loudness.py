"""Golden vectors for programme loudness (docs/design/programme-meter.md §6.1): ITU-R
BS.1770-5 K-weighting at every supported rate, the EBU Tech 3341 / 3342 cases synthesised
from the documents' text, and an unbinned reference meter for the same signals.

Reference meter (the definitions `ac2_core::loudness` implements, without its histograms):

    y      = K-weighted channel (pre-filter then RLB, scipy.signal.lfilter, f64)
    e_j    = sum over channels and over 10 ms sub-block j of y^2   (weights 1)
    z(J,n) = sum_{i=J-n}^{J-1} e_i / (n N),   N = fs / 100 samples per sub-block
    L(z)   = -0.691 + 10 log10 z    (LUFS)
    momentary after sub-block J: L(z(J, 40));  short-term: L(z(J, 300))
    gating blocks: z(J, 40) for J = R + 40 + 10 k   (R = sub-block of the reset)
    integrated: Ja = {l > -70}; G = L(mean z over Ja) - 10; I = L(mean z over Ja and l > G)
    range values: z(J, 300) for J = R + 300 + 10 k
    LRA: Ja = {S >= -70}; G = L(mean z over Ja) - 20; sorted S over Ja and S >= G;
         P_p = S[floor((n - 1) p / 100 + 0.5)] (zero based); LRA = P95 - P10

Signals ("programmes"): a tone `x[n] = 10^(A(n)/20) sin(2 pi f m / fs)`, m = n mod P,
P = fs / gcd(fs, f), n counted from the programme's first sample, rounded to f32; A is
constant over a segment or ramps linearly in dB (A(n) = a + (b - a) k / len, k from the
segment's start), or the segment is silence. A stereo programme feeds the same samples to
both channels (in phase, as the EBU cases say); a mono one is one channel of weight 1. The
segments are stored as `[seconds, a, b]` (`a = b = null` for silence) and synthesised by the
Rust tests from these parameters, so the files hold no signals.
"""

import math
import pathlib
import sys

import numpy as np
import scipy.signal as sig

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import generate as g  # noqa: E402

RATES = [44100, 48000, 88200, 96000, 176400, 192000]
SUB_S = 0.01
M_SB, S_SB, HOP = 40, 300, 10
OFFSET = -0.691

# BS.1770-5 Annex 1, Tables 1 and 2 (48 kHz): b0, b1, b2, a1, a2.
TABLE_1 = [1.53512485958697, -2.69169618940638, 1.19839281085285,
           -1.69065929318241, 0.73248077421585]
TABLE_2 = [1.0, -2.0, 1.0, -1.99004745483398, 0.99007225036621]

PRE = dict(f0=1681.974450955533, gain_db=3.999843853973347, q=0.7071752369554196,
           vb_exp=0.4996667741545416)
RLB = dict(f0=38.13547087602444, q=0.5003270373238773)

KW_TEXT = (
    "pre-filter: K = tan(pi f0/fs), Vh = 10^(G/20), Vb = Vh^vb_exp, a0 = 1 + K/Q + K^2; "
    "b = [Vh + Vb K/Q + K^2, 2(K^2 - Vh), Vh - Vb K/Q + K^2]/a0, "
    "a = [1, 2(K^2 - 1)/a0, (1 - K/Q + K^2)/a0]. RLB: K = tan(pi f0/fs), a0 = 1 + K/Q + K^2, "
    "b = g [1, -2, 1], g = a0(48 kHz)/a0(fs), a = [1, 2(K^2 - 1)/a0, (1 - K/Q + K^2)/a0]"
)


def kweighting(fs: float):
    """[(b, a) pre, (b, a) rlb], a with a0 = 1."""
    k = math.tan(math.pi * PRE["f0"] / fs)
    vh = 10.0 ** (PRE["gain_db"] / 20.0)
    vb = vh ** PRE["vb_exp"]
    q = PRE["q"]
    a0 = 1.0 + k / q + k * k
    pre = ([(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0],
           [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0])

    def rlb_a0(rate):
        kk = math.tan(math.pi * RLB["f0"] / rate)
        return 1.0 + kk / RLB["q"] + kk * kk

    k = math.tan(math.pi * RLB["f0"] / fs)
    q = RLB["q"]
    a0 = 1.0 + k / q + k * k
    gg = rlb_a0(48000.0) / a0
    rlb = ([gg, -2.0 * gg, gg], [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0])
    return [pre, rlb]


def response_db(fs: float, f: np.ndarray) -> np.ndarray:
    out = np.zeros_like(f)
    for b, a in kweighting(fs):
        _, h = sig.freqz(b, a, worN=f, fs=fs)
        out += 20.0 * np.log10(np.abs(h))
    return out


def f32(x):
    return np.asarray(x, dtype=np.float64).astype(np.float32).astype(np.float64)


def tone(fs: int, freq: int, segments) -> np.ndarray:
    """A programme's samples (see the module doc)."""
    period = fs // math.gcd(fs, freq)
    table = np.sin(2.0 * np.pi * freq * np.arange(period) / fs)
    parts = []
    for secs, a, b in segments:
        ln = int(round(secs * fs))
        if a is None:
            parts.append(np.zeros(ln))
            continue
        if a == b:
            amp = np.full(ln, 10.0 ** (a / 20.0))
        else:
            amp = 10.0 ** ((a + (b - a) * np.arange(ln) / ln) / 20.0)
        parts.append(amp)
    amp = np.concatenate(parts)
    x = amp * table[np.arange(len(amp)) % period]
    return f32(x)


def programme(freq, segments, channels=2):
    return {"freq_hz": freq, "channels": channels,
            "segments": [list(s) for s in segments]}


def sub_block_energy(stream: list, fs: int) -> np.ndarray:
    """e_j over a stream of programmes played back to back (filter state carried)."""
    x = np.concatenate([tone(fs, p["freq_hz"], p["segments"]) for p in stream])
    ch = stream[0]["channels"]
    y = x
    for b, a in kweighting(fs):
        y = sig.lfilter(b, a, y)
    n = fs // 100
    m = len(y) // n
    return ch * (y[: m * n] ** 2).reshape(m, n).sum(axis=1)


def lufs(z):
    with np.errstate(divide="ignore"):
        return OFFSET + 10.0 * np.log10(z)


def windows(e: np.ndarray, n_sb: int, fs: int) -> np.ndarray:
    """z(J, n) for J = n..len(e), at index J - n; each window re-summed, as the meter does."""
    if len(e) < n_sb:
        return np.zeros(0)
    w = np.lib.stride_tricks.sliding_window_view(e, n_sb).sum(axis=1)
    return w / (n_sb * (fs // 100))


class Meter:
    """The unbinned reference over a stream, reset at sub-block `reset`."""

    def __init__(self, stream, fs, reset=0):
        self.fs = fs
        self.e = sub_block_energy(stream, fs)
        self.reset = reset
        self.zm = windows(self.e, M_SB, fs)   # zm[J - 40]
        self.zs = windows(self.e, S_SB, fs)

    def momentary(self):
        return lufs(self.zm)

    def short_term(self):
        return lufs(self.zs)

    def gating_blocks(self, end=None):
        end = len(self.e) if end is None else end
        js = np.arange(self.reset + M_SB, end + 1, HOP)
        return self.zm[js - M_SB]

    def range_values(self, end=None):
        end = len(self.e) if end is None else end
        js = np.arange(self.reset + S_SB, end + 1, HOP)
        return self.zs[js - S_SB]

    def integrated(self, end=None):
        return integrated(self.gating_blocks(end))

    def lra(self, end=None):
        return lra(self.range_values(end))

    def max_momentary_at(self, end):
        """Max momentary over windows wholly in [reset, end] sub-blocks."""
        return lufs(self.zm[self.reset: end - M_SB + 1].max())

    def max_short_term_at(self, end):
        return lufs(self.zs[self.reset: end - S_SB + 1].max())


def integrated(z, include_bin=None):
    l = lufs(z)
    ja = l > -70.0
    if not ja.any():
        return None
    gamma = float(lufs(z[ja].mean()) - 10.0)
    jg = ja & (l > gamma)
    if include_bin is not None and gamma > -70.0:
        b = np.floor((l + 70.0) * 100.0) == math.floor((gamma + 70.0) * 100.0)
        jg = (jg | (b & ja)) if include_bin else (jg & ~b)
    return float(lufs(z[jg].mean())), gamma


def lra(z, include_bin=None):
    s = lufs(z)
    ja = s >= -70.0
    if not ja.any():
        return None
    gamma = float(lufs(z[ja].mean()) - 20.0)
    jr = ja & (s >= gamma)
    if include_bin is not None and gamma >= -70.0:
        b = np.floor((s + 70.0) * 100.0) == math.floor((gamma + 70.0) * 100.0)
        jr = (jr | (b & ja)) if include_bin else (jr & ~b)
    v = np.sort(s[jr])
    n = len(v)
    lo = v[int(math.floor((n - 1) * 10 / 100 + 0.5))]
    hi = v[int(math.floor((n - 1) * 95 / 100 + 0.5))]
    return float(hi - lo), gamma


def sb(secs):
    return int(round(secs / SUB_S))


def r6(x):
    return round(float(x), 6)


def seg(secs, db=None, end=None):
    return [r6(secs), db, db if end is None else end]


REFS_3341 = "EBU Tech 3341 V4 (2023) Table 1, synthesised from its text"
REFS_3342 = "EBU Tech 3342 V4 (2023) Table 1, synthesised from its text"
REF_1770 = "ITU-R BS.1770-5 Annex 1 (K-weighting, gating)"


def gen_loudness_kweighting() -> g.VectorSet:
    freqs = np.geomspace(20.0, 19200.0, 241)
    vs = g.VectorSet(
        name="loudness_kweighting",
        description="BS.1770-5 K-weighting re-derived per rate: biquad coefficients at "
                    "44.1-192 kHz, the cascade's magnitude response from 20 Hz to 19.2 kHz, "
                    "and Tables 1 and 2.",
        function="gen_loudness_kweighting",
        parameters={"rates_hz": RATES, "formulas": KW_TEXT, "pre": PRE, "rlb": RLB,
                    "coefficient_layout": "[rate][stage pre, rlb][b0, b1, b2, a1, a2]"},
        references=[REF_1770],
    )
    coef = np.array([[b + a[1:] for b, a in kweighting(fs)] for fs in RATES])
    vs.add("tables", np.array([TABLE_1, TABLE_2]), unit="1",
           description="BS.1770-5 Tables 1 and 2 as printed (48 kHz)")
    vs.add("rates", np.array(RATES, dtype=float), unit="Hz", description="rates")
    vs.add("coefficients", coef, unit="1", description="per-rate coefficients",
           tolerance=g.lin_tol(1e-12, 0.0))
    vs.add("freqs", freqs, unit="Hz", description="response frequencies")
    vs.add("response_db", np.array([response_db(fs, freqs) for fs in RATES]), unit="dB",
           description="cascade magnitude per rate, [rate][freqs]; freqz's cos-form "
                       "evaluation loses digits near the RLB poles at high rates",
           tolerance=g.lin_tol(1e-6, 0.0))
    return vs


def gen_loudness_ebu3341() -> g.VectorSet:
    cases = {
        "1": ([seg(20, -23)], {"momentary": -23.0, "short_term": -23.0, "integrated": -23.0}),
        "2": ([seg(20, -33)], {"momentary": -33.0, "short_term": -33.0, "integrated": -33.0}),
        "3": ([seg(10, -36), seg(60, -23), seg(10, -36)], {"integrated": -23.0}),
        "4": ([seg(10, -72), seg(10, -36), seg(60, -23), seg(10, -36), seg(10, -72)],
              {"integrated": -23.0}),
        "5": ([seg(20, -26), seg(20.1, -20), seg(20, -26)], {"integrated": -23.0}),
    }
    vs = g.VectorSet(
        name="loudness_ebu3341",
        description="EBU Tech 3341 cases 1-5 (all rates), 9, 12 and the live forms of 11 and "
                    "14 (48 kHz, two reset phases): published readings and the unbinned "
                    "reference meter's values for the same synthesised signals.",
        function="gen_loudness_ebu3341",
        parameters={},
        references=[REFS_3341, REF_1770],
    )
    p = vs.parameters
    p["signal"] = "1000 Hz stereo tone per the module doc of tools/refgen/sets/loudness.py"
    p["tolerance_lu"] = 0.1
    p["rates_hz"] = RATES
    p["cases"] = {}
    for name, (segs, published) in cases.items():
        p["cases"][name] = {"programme": programme(1000, segs), "published": published}
        for fs in RATES:
            m = Meter([programme(1000, segs)], fs)
            vs.scalars[f"c{name}_{fs}_integrated"] = m.integrated()[0]
            vs.scalars[f"c{name}_{fs}_momentary"] = float(m.momentary()[-1])
            vs.scalars[f"c{name}_{fs}_short_term"] = float(m.short_term()[-1])
    p["not_run"] = {
        "6": "5.0 surround: ac2 has no surround source",
        "7, 8": "the EBU's programme recordings, not in the text",
        "10, 13": "file forms of 11 and 14 (twenty files each measured after a reset); the "
                  "live forms test the same windows",
        "15-19": "true peak: tests/it/loudness of ac2-core from the text (analytic)",
    }

    fs = 48000
    c9 = programme(1000, [seg(1.34, -20), seg(1.66, -30)] * 5)
    c12 = programme(1000, [seg(0.18, -20), seg(0.22, -30)] * 25)
    p["case9"] = {"programme": c9, "published_short_term": -23.0,
                  "from_sub_block": S_SB, "note": "every sub-block value after 3 s"}
    p["case12"] = {"programme": c12, "published_momentary": -23.0,
                   "from_sub_block": sb(1.0), "note": "every sub-block value after 1 s"}
    s9 = Meter([c9], fs).short_term()  # index J - 300, J from 300
    vs.add("case9_short_term", s9, unit="LUFS",
           description="short-term after each sub-block J = 300.. (48 kHz)",
           tolerance=g.lin_tol(1e-6, 0.0))
    m12 = Meter([c12], fs).momentary()[sb(1.0) - M_SB:]
    vs.add("case12_momentary", m12, unit="LUFS",
           description="momentary after each sub-block J = 100.. (48 kHz)",
           tolerance=g.lin_tol(1e-6, 0.0))

    lead = programme(1000, [seg(1.0, -30)])
    p["live"] = {
        "lead_in": lead,
        "reset": "reset requested after the lead-in (a sub-block boundary); the signal "
                 "follows at once (phase 0) or after 240 samples of silence (phase 240: the "
                 "reset 5 ms before the signal)",
        "phases_samples": [0, 240],
        "read": "the maximum after the last sample of each segment",
    }
    n = fs // 100
    for case, seg_s, ton_s, quantity in (("11", 6.0, 3.0, "max_short_term"),
                                         ("14", 0.8, 0.4, "max_momentary")):
        step = 0.15 if case == "11" else 0.02
        segs = []
        for i in range(20):
            segs += [seg(step * i), seg(ton_s, -38.0 + i), seg(seg_s - ton_s - step * i)]
        p["live"][f"case{case}"] = {
            "programme": programme(1000, segs),
            "segment_s": seg_s,
            "quantity": quantity,
            "published": [-38.0 + i for i in range(20)],
        }
        rows = []
        for phase in (0, 240):
            pre = [programme(1000, [seg(phase / fs)])] if phase else []
            m = Meter([lead] + pre + [programme(1000, segs)], fs, reset=sb(1.0))
            ends = [sb(1.0) + (phase + int(round(seg_s * (i + 1) * fs))) // n for i in range(20)]
            f = m.max_short_term_at if case == "11" else m.max_momentary_at
            rows.append([f(e) for e in ends])
        vs.add(f"case{case}_maxima", np.array(rows), unit="LUFS",
               description=f"{quantity} at the end of each segment, [phase 0, 240][segment]",
               tolerance=g.lin_tol(1e-6, 0.0))
    return vs


def gen_loudness_ebu3342() -> g.VectorSet:
    l1 = [seg(20, -20), seg(20, -30)]
    cases = {
        "1": (l1, 10.0),
        "2": ([seg(20, -20), seg(20, -15)], 5.0),
        "3": ([seg(20, -40), seg(20, -20)], 20.0),
        "4": ([seg(20, v) for v in (-50, -35, -20, -35, -50)], 15.0),
        "1x2": (l1 + l1, 10.0),
    }
    vs = g.VectorSet(
        name="loudness_ebu3342",
        description="EBU Tech 3342 cases 1-4 and case 1 repeated twice: LRA at the end of "
                    "the signal (no trailing silence), published and the unbinned reference "
                    "at every rate.",
        function="gen_loudness_ebu3342",
        parameters={"signal": "1000 Hz stereo tone per the module doc", "tolerance_lu": 1.0,
                    "rates_hz": RATES, "cases": {}, "not_run": {
                        "5, 6": "the EBU's programme recordings, not in the text"}},
        references=[REFS_3342],
    )
    for name, (segs, want) in cases.items():
        vs.parameters["cases"][name] = {"programme": programme(1000, segs), "published": want}
        for fs in RATES:
            vs.scalars[f"c{name}_{fs}_range"] = Meter([programme(1000, segs)], fs).lra()[0]
    return vs


def gen_loudness_tones() -> g.VectorSet:
    vs = g.VectorSet(
        name="loudness_tones",
        description="Integrated loudness of 40 Hz, 1 kHz and 10 kHz stereo tones at every "
                    "rate; the -18 dBFS stereo calibration tone; mono sources.",
        function="gen_loudness_tones",
        parameters={"rates_hz": RATES, "rate_tolerance_lu": 0.05},
        references=[REF_1770, "EBU Tech 3341 V4 §2.9 (calibration tone)"],
    )
    p = vs.parameters
    p["tones"] = {str(f): programme(f, [seg(5, -20)]) for f in (40, 1000, 10000)}
    for f, prog in p["tones"].items():
        for fs in RATES:
            vs.scalars[f"tone{f}_{fs}_integrated"] = Meter([prog], fs).integrated()[0]
    cal = programme(1000, [seg(10, -18)])
    p["calibration"] = {"programme": cal, "published": -18.0, "tolerance_lu": 0.05}
    m = Meter([cal], 48000)
    vs.scalars["calibration_integrated"] = m.integrated()[0]
    vs.scalars["calibration_momentary"] = float(m.momentary()[-1])
    vs.scalars["calibration_short_term"] = float(m.short_term()[-1])
    p["mono"] = {
        "case1": programme(1000, [seg(20, -23)], channels=1),
        "case3": programme(1000, [seg(10, -36), seg(60, -23), seg(10, -36)], channels=1),
        "note": "one channel of weight 1: 3 LU under the same signal on a pair",
    }
    for name in ("case1", "case3"):
        m = Meter([p["mono"][name]], 48000)
        vs.scalars[f"mono_{name}_integrated"] = m.integrated()[0]
        vs.scalars[f"mono_{name}_momentary"] = float(m.momentary()[-1])
    return vs


def dense_programme(ci, cl):
    return programme(1000, [seg(60, -20), seg(60, ci - 0.5, ci + 0.5),
                            seg(60, cl - 0.5, cl + 0.5)])


def gen_loudness_histogram() -> g.VectorSet:
    fs = 48000
    k1000 = float(response_db(fs, np.array([1000.0]))[0]) + OFFSET  # tone LUFS - dBFS
    ci, cl = -35.0, -45.0
    for _ in range(4):
        m = Meter([dense_programme(ci, cl)], fs)
        ci = round(m.integrated()[1] - k1000, 3)
        cl = round(m.lra()[1] - k1000, 3)
    dense = dense_programme(ci, cl)
    quiet = programme(1000, [seg(10, v) for v in (-58, -62, -66, -69, -75, -82, -64)])
    vs = g.VectorSet(
        name="loudness_histogram",
        description="Histogram bound checks: a programme whose gating blocks and short-term "
                    "values crowd both relative gates (slow 1 dB ramps centred on them), and "
                    "a quiet programme whose relative gates fall below -70 LUFS.",
        function="gen_loudness_histogram",
        parameters={
            "dense": {"programme": dense,
                      "design": "60 s at -20 dBFS, then 60 s ramps of +-0.5 dB centred on the "
                                "integrated and LRA relative gates (iterated four times)",
                      "bound": "binned integrated within [unbinned with the gate's bin "
                               "excluded, included]; LRA likewise widened by 0.01 LU"},
            "quiet": {"programme": quiet, "note": "the absolute gate governs both measures"},
        },
        references=[REF_1770, REFS_3342],
    )
    m = Meter([dense], fs)
    for name, f in (("integrated", integrated), ("range", lra)):
        z = m.gating_blocks() if name == "integrated" else m.range_values()
        exact, gamma = f(z)
        vs.scalars[f"dense_{name}"] = exact
        vs.scalars[f"dense_{name}_gate"] = gamma
        vs.scalars[f"dense_{name}_with_gate_bin"] = f(z, include_bin=True)[0]
        vs.scalars[f"dense_{name}_without_gate_bin"] = f(z, include_bin=False)[0]
        lv = lufs(z)
        vs.scalars[f"dense_{name}_gate_bin_count"] = int(
            (np.floor((lv + 70.0) * 100.0) == math.floor((gamma + 70.0) * 100.0)).sum())
    m = Meter([quiet], fs)
    i, gi = m.integrated()
    r, gr = m.lra()
    vs.scalars.update(quiet_integrated=i, quiet_integrated_gate=gi, quiet_range=r,
                      quiet_range_gate=gr)
    assert gi < -70.0 and gr < -70.0, (gi, gr)
    return vs


TP_FIR_TAPS = 255
TP_CUTOFF = 0.45  # of fs (the output rate)
TP_KAISER_BETA = 8.0
TP_FADE_S = 0.01
TP_LENGTH_S = 0.5
TP_WINDOW = 256
TP_UP = 32


def tp_case(fs: int, offset: int):
    """3341 cases 20-23 at output rate fs (see gen_true_peak_ebu3341)."""
    hi = 4 * fs
    n = int(TP_LENGTH_S * hi)
    splice = (n // 2) // 24 * 24  # fs/6 has period 24 at 4 fs: phase 0 here
    k = np.arange(n)
    x = 0.5 * np.sin(2.0 * np.pi * (k % 24) / 24.0)
    x = np.concatenate([x[:splice], np.sin(2.0 * np.pi * np.arange(16) / 16.0), x[splice:]])
    nf = int(TP_FADE_S * hi)
    fade = 0.5 - 0.5 * np.cos(np.pi * np.arange(nf) / nf)
    x[:nf] *= fade
    x[-nf:] *= fade[::-1]
    h = sig.firwin(TP_FIR_TAPS, TP_CUTOFF * fs, window=("kaiser", TP_KAISER_BETA), fs=hi)
    y = np.convolve(x, h)[TP_FIR_TAPS // 2: TP_FIR_TAPS // 2 + len(x)]
    y = f32(y[offset::4])
    centre = splice // 4
    win = y[centre - TP_WINDOW // 2: centre + TP_WINDOW // 2]
    up = sig.resample_poly(y, TP_UP, 1, window=("kaiser", 10.0))
    return win, 20.0 * np.log10(np.abs(up).max()), 20.0 * np.log10(np.abs(y).max())


def gen_true_peak_ebu3341() -> g.VectorSet:
    fs = 48000
    vs = g.VectorSet(
        name="true_peak_ebu3341",
        description="EBU Tech 3341 cases 20-23: one period of fs/4 (amplitude 1) spliced into "
                    "an fs/6 tone (0.5), made at 4 fs and decimated at offsets 0-3. Holds the "
                    "256 samples around the splice (elsewhere the plain tone, true peak "
                    "-6 dBTP), the published reading and refgen's own x32 true peak.",
        function="gen_true_peak_ebu3341",
        parameters={
            "fs_hz": fs,
            "synthesis": "at 4 fs: 0.5 sin(2 pi k/24) for 0.5 s, one period sin(2 pi k/16) "
                         "inserted at a phase-0 sample nearest the middle (phase continuous "
                         "at both sides), 10 ms raised-cosine fades",
            "decimator": f"scipy.signal.firwin({TP_FIR_TAPS}, {TP_CUTOFF} fs, kaiser beta "
                         f"{TP_KAISER_BETA}) at 4 fs, delay compensated, every 4th sample "
                         "from the offset; rounded to f32",
            "reconstruction": f"scipy.signal.resample_poly(x, {TP_UP}, 1, kaiser beta 10) "
                              "over the whole decimated signal",
            "published_dbtp": 0.0, "tolerance_db": {"above": 0.2, "below": 0.4},
            "window_samples": TP_WINDOW,
        },
        references=[REFS_3341, "ITU-R BS.1770-5 Annex 2"],
    )
    for off in range(4):
        win, tp32, sp = tp_case(fs, off)
        vs.add(f"case{20 + off}_window", win, unit="FS",
               description=f"case {20 + off}: 256 samples around the splice")
        vs.scalars[f"case{20 + off}_true_peak_x32_dbtp"] = float(tp32)
        vs.scalars[f"case{20 + off}_sample_peak_dbfs"] = float(sp)
    return vs


GENERATORS = [gen_loudness_kweighting, gen_loudness_ebu3341, gen_loudness_ebu3342,
              gen_loudness_tones, gen_loudness_histogram, gen_true_peak_ebu3341]
