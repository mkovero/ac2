"""The OSM stage: Open Sound Meter's own DSP as a standing reference for ac2's live math.

Offline and file based. Each case is a meas/ref WAV pair: generated with analytic truth
(`osm_fixtures`) or cut from an earlier rig run. The pair goes through
- the external `osm-harness` (GPL, outside this repository: only run, its JSON read), and
- ac2 itself: a private ac2d (fake backend, own HOME, no autosave), the WAV imported as a
  recording and replayed `--fast`, with a transfer and a spectrum measurement on it.
The results are compared definition-aware (README, "OSM stage") and judged with
`[osm]` in tolerances.toml. Nothing is played: the replay backend has no outputs."""
from __future__ import annotations

import json
import math
import os
import shutil
import subprocess
import tempfile
import time
import tomllib
from contextlib import contextmanager
from dataclasses import asdict
from pathlib import Path

import numpy as np

from . import dsp, osm_fixtures, wav
from .ac2 import Ac2, Ac2Error
from .analyse import TOLERANCES, Check, _tol, judge
from .formats import read_ac2_csv

HERE = Path(__file__).resolve().parent.parent
CONFIG = HERE / "osm.toml"

# Below this many dB re its strongest bin the reference meets OSM's float32 floor (its
# "DC removal" subtracts the windowed block's sum in float32, which sets a spectrum-wide
# rounding floor near -80 dB re the strongest bin): a ratio there is OSM's rounding.
REF_FLOOR_DB = 70.0
# Columns are judged where the analysers say the data are trustworthy.
GATE_G2 = 0.95
# OSM's coherence is a running sum over a fixed number of ticks, whatever the averaging.
OSM_COHERENCE_TICKS = 21
BAND = (50.0, 20000.0)        # magnitude / phase / coherence columns
SLOPE_BAND = (1000.0, 20000.0)  # phase-slope delay
BIAS_BAND = (1000.0, 20000.0)   # OSM's magnitude bias and coherence model: many bins, flat noise


class Skip(Exception):
    pass


# ---------------------------------------------------------------- tools


def find_tools(cfg: dict) -> tuple[Path, Path]:
    """The harness and the ac2 bin directory, or Skip saying what is missing."""
    h = os.environ.get("OSM_HARNESS") or cfg.get("tools", {}).get("harness") or ""
    if not h:
        raise Skip("no osm-harness: set OSM_HARNESS (or [tools].harness in osm.toml) to the built binary; "
                   "README, 'OSM stage', says how to build it")
    hp = Path(os.path.expanduser(h))
    if not (hp.is_file() and os.access(hp, os.X_OK)):
        raise Skip(f"osm-harness {hp} is missing or not executable")
    d = os.environ.get("AC2_BIN_DIR") or cfg.get("tools", {}).get("ac2_bin_dir") or ""
    if d:
        bd = Path(os.path.expanduser(d))
    else:
        w = shutil.which("ac2d")
        bd = Path(w).parent if w else Path("/nonexistent")
    for b in ("ac2", "ac2d"):
        if not os.access(bd / b, os.X_OK):
            raise Skip(f"no {b} in {bd}: set AC2_BIN_DIR (or [tools].ac2_bin_dir) to the directory holding ac2 and ac2d")
    return hp, bd


def run_harness(harness: Path, wav_path: Path, out: Path, s: dict) -> dict:
    argv = [str(harness), "--stereo", str(wav_path), "--fft", str(s["fft"]), "--window", s["window"],
            "--averageType", "fifo", "--average", str(s["osm_average"]), "--out", str(out)]
    if s.get("osm_delay"):
        argv += ["--delay", str(int(s["osm_delay"]))]
    p = subprocess.run(argv, capture_output=True, text=True, timeout=600)
    if p.returncode != 0:
        raise RuntimeError(f"osm-harness exit {p.returncode}: {p.stderr.strip()[-500:]}")
    return json.loads(out.read_text())


@contextmanager
def private_daemon(bin_dir: Path, root: Path):
    """An ac2d of its own: fake backend (it opens no device), its own HOME, config, runtime
    and recordings, no autosave. The replay sessions it opens have no outputs."""
    home = root / "home"
    run = root / "run"
    rec = root / "recordings"
    for d in (home, run, rec):
        d.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / ".config"),
               XDG_DATA_HOME=str(home / ".local/share"), AC2_CONFIG_DIR=str(home / ".config/ac2"),
               AC2_RUNTIME_DIR=str(run))
    log = open(root / "ac2d.log", "w")
    p = subprocess.Popen([str(bin_dir / "ac2d"), "--backend", "fake", "--no-autosave", "--recordings", str(rec)],
                         stdout=log, stderr=subprocess.STDOUT, env=env)
    try:
        t0 = time.monotonic()
        while not (run / "ctrl.sock").exists():
            if p.poll() is not None or time.monotonic() - t0 > 10:
                raise RuntimeError(f"ac2d did not start (see {root / 'ac2d.log'})")
            time.sleep(0.05)
        yield Ac2([str(bin_dir / "ac2")], timeout="5s", log=root / "ac2.log", env=env), rec
    finally:
        p.terminate()
        try:
            p.wait(10)
        except subprocess.TimeoutExpired:
            p.kill()
        log.close()


def ac2_spectrum_fifo(s: dict, fs: float) -> int:
    """ac2 spectrum FIFO frames spanning the audio OSM's FIFO spans: OSM's ticks are
    round(0.08 fs) apart, a long ac2 FFT's frames N/8."""
    n = 2 ** int(s["fft"])
    hop_osm = round(0.08 * fs)
    return max(1, round((int(s["osm_average"]) - 1) * hop_osm / (n // 8)) + 1)


def run_ac2(ac2: Ac2, recdir: Path, name: str, wav_path: Path, frames: int, s: dict, d: Path, align: bool) -> dict:
    """Replays the pair through ac2 and stores its TF and spectrum captures and its delay
    finding in `d`. With `align`, the finder's first arrival is inserted and the file
    replayed again, so the TF is averaged on aligned windows."""
    ac2.run("rec", "import", str(wav_path), "--dir", str(recdir), "--name", name)
    for m in ("osm-tf", "osm-sp"):
        ac2.meas_rm(m)
    ac2.run("meas", "new", "tf", "--name", "osm-tf", "--ref", 2, "--meas", 1, "--blocks", s["ac2_tf_blocks"], "--start")
    fifo = ac2_spectrum_fifo(s, osm_fixtures.FS)
    ac2.run("meas", "new", "spectrum", "--name", "osm-sp", "--input", 1, "--fft", f"{2 ** int(s['fft'])}samples",
            "--window", s["window"], "--average", f"fifo:{fifo}", "--start")

    def replay():
        sess = ac2.run("session", "replay", name, "--fast") or {}
        epoch = sess.get("epoch")
        # The replay has ended once a capture is of the file's last sample.
        t0 = time.monotonic()
        while True:
            # refused until the new session's first result exists
            c = ac2.run("trace", "capture", "osm-tf", "--name", "osm-probe", check=False)
            c = c if isinstance(c, dict) else {}
            ac2.run("trace", "rm", "osm-probe", json_out=False, check=False)
            src = c.get("source") or {}
            # a result of the previous replay (same length) is not this one's end
            if src.get("epoch") == epoch and int(src.get("at_sample", -1)) + 1 >= frames:
                return
            if time.monotonic() - t0 > 300:
                raise Ac2Error(f"{name}: replay did not reach sample {frames} in 300 s")
            time.sleep(0.3)

    replay()
    found = ac2.run("delay", "find", "osm-tf")
    inserted = None
    if align:
        # the uncompensated TF too, as context: what the operator sees before inserting
        ac2.run("trace", "rm", "osm-tf-raw", json_out=False, check=False)
        ac2.capture("osm-tf", "osm-tf-raw")
        ac2.export_trace("osm-tf-raw", d / "tf-raw.csv")
        ac2.run("trace", "rm", "osm-tf-raw", json_out=False, check=False)
        inserted = ac2.run("delay", "insert", "osm-tf")
        replay()
    for meas, trace, csv in (("osm-tf", "osm-tf-cap", "tf.csv"), ("osm-sp", "osm-sp-cap", "spectrum.csv")):
        ac2.run("trace", "rm", trace, json_out=False, check=False)
        ac2.capture(meas, trace)
        ac2.export_trace(trace, d / csv)
        ac2.run("trace", "rm", trace, json_out=False, check=False)
    info = {"delay_find": found, "inserted": inserted is not None, "spectrum_fifo": fifo}
    (d / "ac2.json").write_text(json.dumps(info, indent=1))
    return info


# ---------------------------------------------------------------- definitions and models


def first_arrival(found) -> float | None:
    """The delay finder's first arrival (samples), whatever its outcome type."""
    f = (found or {}).get("finding") if isinstance(found, dict) else None
    if not f:
        return None
    o = f.get("outcome") or {}
    for k in ("first",):
        if isinstance(o.get(k), dict):
            return float(o[k]["delay_samples"])
    r = o.get("ranked") or f.get("candidates") or []
    return float(r[0]["delay_samples"]) if r else None


def welch_neff(n: int, hop: int, k: int) -> float:
    """Equivalent independent averages of k Hann frames of n samples, hop apart (Welch's
    overlap correction: each pair at lag j counts by its windows' squared correlation)."""
    w = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(n) / n)
    s = float(np.sum(w * w))
    t = 0.0
    for j in range(1, k):
        if j * hop >= n:
            break
        r = float(np.sum(w[: n - j * hop] * w[j * hop:])) / s
        t += (1 - j / k) * r * r
    return k / (1 + 2 * t)


def expected_g2(g2: float, n: float) -> float:
    """E[estimated γ²] from n independent averages at true γ² (Carter, Knapp & Nuttall 1973):
    1/n + (n−1)/(n+1)·γ²·₂F₁(1, 1; n+2; γ²). Few averages read high at low coherence."""
    term, s = 1.0, 1.0
    for k in range(5000):
        term *= (1 + k) * (1 + k) / ((n + 2 + k) * (k + 1)) * g2
        s += term
        if term < 1e-15:
            break
    return 1 / n + (n - 1) / (n + 1) * g2 * s


def expected_ratio_mean(snr_db: float) -> float:
    """E|M|/|R| per bin for M = R + N, N uncorrelated with |N|²/|R|² = 1/SNR on average:
    the expectation of OSM's magnitude (a mean of ratios; averaging overlapped ticks keeps
    the expectation). With Z = N/R, |Z| = s·q where q = |g1|/|g2| of two complex normals
    has density 2q/(1+q²)², and ∠Z is uniform, so E|1+Z| = ∫ A(s·q)·2q/(1+q²)² dq with
    A(ρ) the mean of |1 + ρ·e^{jθ}| over θ."""
    s = 10 ** (-snr_db / 20)
    # q on a log grid: the density has a q^-3 tail and A grows like ρ, so the integrand
    # falls like q^-2 and the tail past q_max is ~2s/q_max
    lq = np.linspace(-8, 8, 40001)
    q = np.exp(lq)
    th = np.linspace(0, 2 * np.pi, 721)[:-1]
    rho = s * q
    # A(ρ) on the grid in chunks (memory)
    A = np.empty_like(rho)
    for i in range(0, len(rho), 4000):
        r = rho[i:i + 4000, None]
        A[i:i + 4000] = np.mean(np.abs(1 + r * np.exp(1j * th)[None, :]), axis=1)
    p = 2 * q / (1 + q * q) ** 2
    integrand = A * p * q  # dq = q d(ln q)
    return float(np.trapezoid(integrand, lq) + 2 * s / q[-1])


def rayleigh_mean_db() -> float:
    """20·log10 of a Rayleigh amplitude's mean over its RMS, √(π/4): what a linear mean of
    noise amplitudes (OSM's RTA module) reads below a power mean (ac2's spectrum)."""
    return 20 * math.log10(math.sqrt(math.pi / 4))


# ---------------------------------------------------------------- reading both sides


def osm_arrays(j: dict):
    a = np.array([[np.nan if v is None else v for v in r] for r in j["ftdata"]], dtype=float)
    f, module, mag, ph, coh = a.T
    nan_phase = int(np.sum(~np.isfinite(ph[1:])))
    H = mag * np.exp(1j * ph)
    return f, module, H, coh, nan_phase


def ref_level_db(x_ref: np.ndarray, n: int) -> np.ndarray:
    """The reference's level per bin over the last n samples (the frame OSM's last tick
    took), dB re its strongest bin; for the float32-floor mask."""
    w = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(n) / n)
    R = np.abs(np.fft.rfft(x_ref[-n:] * w))[: n // 2]
    return 20 * np.log10(np.maximum(R, 1e-300) / R.max())


def osm_on_columns(f, H, coh, ok, centres, tau_s):
    """OSM's native bins in ac2's 1/48-octave columns: the delay is taken out per bin and
    put back at the column centre (a complex sum across a column would turn with it), the
    magnitude is the column's power mean, the coherence its mean γ²."""
    Hd = np.where(ok, H * np.exp(2j * np.pi * f * tau_s), np.nan)
    keep = np.isfinite(Hd)
    Hc = dsp.band_mean(f[keep], Hd[keep], centres) * np.exp(-2j * np.pi * centres * tau_s)
    g2 = np.where(keep, coh ** 2, np.nan)
    lo, hi = centres * 2 ** (-1 / 96), centres * 2 ** (1 / 96)
    gc = np.full(len(centres), np.nan)
    for i, (x, y) in enumerate(zip(np.searchsorted(f, lo), np.searchsorted(f, hi))):
        if y > x and np.isfinite(g2[x:y]).any():
            gc[i] = np.nanmean(g2[x:y])
        else:
            k = min(np.searchsorted(f, centres[i]), len(f) - 1)
            gc[i] = g2[k]
    return Hc, gc


def phase_diff_deg(a, b):
    return dsp.wrap_deg(np.rad2deg(np.angle(a / b)))


# ---------------------------------------------------------------- the comparisons


def load_pair(tf_csv: Path, osm: dict, x: np.ndarray, settings: dict, osm_delay: float) -> dict:
    """Both analysers' TF in one phase reference (meas ÷ ref with the path's delay in it:
    the delay each took out goes back into its phase), OSM's bins also on ac2's columns."""
    fs = float(osm["harness"]["sampleRate"])
    f, module, H, coh, nan_phase = osm_arrays(osm)
    n = 2 ** int(settings["fft"])
    floor = ref_level_db(x[:, 1], n) < -REF_FLOOR_DB
    ok = np.isfinite(H) & ~floor
    ok[0] = False  # DC: OSM subtracts the block sum, bin 0 means nothing
    tr = read_ac2_csv(tf_csv)
    b = tr.freq
    fc = b["freq_hz"]
    Ha = 10 ** (b["mag_db"] / 20) * np.exp(1j * np.deg2rad(b["phase_deg"]))
    g2a = b["coherence"]
    applied_ms = float(tr.meta.get("delay_ms") or 0)
    applied = applied_ms * 1e-3
    # Slopes are fitted to the phase each analyser showed (its own delay out), then that
    # delay is added: a flight time of hundreds of samples turns the phase by more than a
    # half turn between 1/48-octave columns, and an unwrap across that guesses wrong.
    sla = (fc >= SLOPE_BAND[0]) & (fc <= min(SLOPE_BAND[1], 0.45 * fs)) & np.isfinite(Ha) & (g2a >= GATE_G2)
    tau_a = (dsp.delay_from_phase(fc[sla], Ha[sla], *SLOPE_BAND) * fs + applied * fs) if sla.sum() > 8 else None
    sl = ok & (f >= SLOPE_BAND[0]) & (f <= SLOPE_BAND[1]) & (coh ** 2 >= GATE_G2)
    tau_os = dsp.delay_from_phase(f[sl], H[sl], *SLOPE_BAND) if sl.sum() > 8 else 0.0  # s, OSM's delay out
    Ha = Ha * np.exp(-2j * np.pi * fc * applied)
    H = H * np.exp(-2j * np.pi * f * osm_delay / fs)
    tau_osm = tau_os + osm_delay / fs
    Ho, g2o = osm_on_columns(f, H, coh, ok, fc, tau_osm)
    return {"fs": fs, "f": f, "H": H, "coh": coh, "ok": ok, "floor": floor, "nan_phase": nan_phase, "fc": fc, "Ha": Ha,
            "g2a": g2a, "Ho": Ho, "g2o": g2o, "tau_osm": tau_osm, "sl": sl, "tau_a": tau_a, "applied_ms": applied_ms,
            "osm_delay": osm_delay, "n": n}


class OsmAnalysis:
    def __init__(self, tol: dict):
        self.tol = tol
        self.checks: list[Check] = []
        self.tables: dict[str, dict] = {}
        self.notes: list[str] = []
        self.series: dict = {}
        self.rows: dict[str, list] = {"tf": [], "masked": [], "settings": [], "delay": [], "noise": [], "spectrum": []}

    def t(self, key):
        v = self.tol["osm"][key]
        return _tol(v)

    def add(self, case, title, value, unit, key, meaning, detail=None, status=None):
        tol = self.t(key) if key else None
        st = status or judge(value, tol)
        self.checks.append(Check(id=f"osm.{case}.{_slug(title)}", group=f"osm {case}", path=case, title=title, value=value, unit=unit, tol=tol,
                                 status=st, meaning=meaning, detail=detail or {}))
        return st

    # ------------------------------------------------------------ one TF case
    def tf_case(self, name: str, d: Path, x: np.ndarray, osm: dict, ac2info: dict, case: osm_fixtures.Case | None,
                settings: dict):
        L = load_pair(d / "tf.csv", osm, x, settings, float(settings.get("osm_delay") or 0))
        fs, f, H, coh, ok, floor, nan_phase = L["fs"], L["f"], L["H"], L["coh"], L["ok"], L["floor"], L["nan_phase"]
        fc, Ha, g2a, Ho, g2o, tau_osm, sl = L["fc"], L["Ha"], L["g2a"], L["Ho"], L["g2o"], L["tau_osm"], L["sl"]
        applied_ms, osm_delay, n = L["applied_ms"], L["osm_delay"], L["n"]
        inband = (fc >= BAND[0]) & (fc <= min(BAND[1], 0.45 * fs))
        have = inband & np.isfinite(Ha) & np.isfinite(Ho)
        ga = have & (g2a >= GATE_G2) & (g2o >= GATE_G2)
        self.rows["masked"].append([name, int(floor[1:].sum()), nan_phase, int((inband & ~np.isfinite(Ha)).sum()),
                                    int((have & ~ga).sum()), int(ga.sum())])
        truth = case.truth(fc) if case is not None and case.kind == "tf" else None
        snr = case.snr_db if case is not None else None

        def maxabs(v):
            v = v[np.isfinite(v)]
            return float(np.max(np.abs(v))) if len(v) else None

        def med(v):
            v = v[np.isfinite(v)]
            return float(np.median(v)) if len(v) else None

        row = [name]
        # ---- magnitude and phase, coherence-gated
        if snr is None:
            if truth is not None:
                gt = have & (g2a >= GATE_G2)
                va = maxabs(dsp.db(Ha[gt]) - dsp.db(truth[gt]))
                self.add(name, "ac2 TF |H| vs analytic, max over γ²≥0.95 columns", va, "dB", "mag_truth_db",
                         f"ac2's live TF (MTW, {settings['ac2_tf_blocks']} blocks) against the fixture's closed form, "
                         f"{int(gt.sum())} columns {BAND[0]:g} Hz–{BAND[1] / 1000:g} kHz")
                go = have & (g2o >= GATE_G2)
                vo = maxabs(dsp.db(Ho[go]) - dsp.db(truth[go]))
                self.add(name, "OSM |M|/|R| vs analytic, max over γ²≥0.95 columns", vo, "dB", "mag_truth_db",
                         "OSM FFT bins power-averaged into ac2's columns, against the closed form")
                pa = maxabs(phase_diff_deg(Ha[gt], truth[gt]))
                po = maxabs(phase_diff_deg(Ho[go], truth[go]))
                self.add(name, "ac2 TF phase vs analytic, max", pa, "°", "phase_truth_deg",
                         "meas ÷ ref with the path's delay in it")
                self.add(name, "OSM phase vs analytic, max", po, "°", "phase_truth_deg",
                         "∠ of OSM's averaged unit phasor, its bins' complex mean per column")
                row += [va, vo, pa, po]
            else:
                row += [None] * 4
            vd = maxabs(dsp.db(Ha[ga]) - dsp.db(Ho[ga]))
            pd = maxabs(phase_diff_deg(Ha[ga], Ho[ga]))
            if case is not None:
                self.add(name, "ac2 vs OSM |H|, max over columns both at γ²≥0.95", vd, "dB", "mag_osm_db",
                         f"{int(ga.sum())} columns; OSM's mean of |M|/|R| and ac2's H1 agree where coherence is high")
                self.add(name, "ac2 vs OSM phase, max", pd, "°", "phase_osm_deg", "same columns")
            else:
                # A room read through a 0.68 s window (OSM FFT16) and through ac2's MTW ladder
                # (short windows at HF) differ column by column by design: the reflections
                # inside one window and not the other. The median is the analysers' agreement.
                vm = med(np.abs(dsp.db(Ha[ga]) - dsp.db(Ho[ga])))
                pm = med(np.abs(phase_diff_deg(Ha[ga], Ho[ga])))
                self.add(name, "ac2 vs OSM |H|, median |difference| over columns both at γ²≥0.95", vm, "dB",
                         "mag_osm_rig_db", f"{int(ga.sum())} columns; max {_fmt(vd)} dB (windows see a room differently)")
                self.add(name, "ac2 vs OSM phase, median |difference|", pm, "°", "phase_osm_rig_deg",
                         f"same columns; max {_fmt(pd, 1)}°")
            row += [vd, pd]
        else:
            row += [None] * 6
        # ---- coherence
        g2true = case.coherence2() if case is not None else None
        bias_band = have & (fc >= BIAS_BAND[0]) & (fc <= BIAS_BAND[1])
        mean_g2a = float(np.nanmean(g2a[bias_band])) if bias_band.any() else None
        okb = ok & (f >= BIAS_BAND[0]) & (f <= BIAS_BAND[1])
        mean_g2o = float(np.nanmean(coh[okb] ** 2)) if okb.any() else None
        neff = welch_neff(n, round(0.08 * fs), OSM_COHERENCE_TICKS)
        if g2true is not None:
            self.add(name, "ac2 γ² vs true γ², mean 1–20 kHz", None if mean_g2a is None else mean_g2a - g2true, "",
                     "coh_ac2", f"ac2 shows γ² (true γ² = {g2true:.4f}: SNR/(1+SNR) of the fixture)")
            model = expected_g2(g2true, neff)
            self.add(name, "OSM γ² vs its expected value, mean 1–20 kHz", None if mean_g2o is None else mean_g2o - model,
                     "", "coh_osm_model",
                     f"OSM shows γ: squared here. Its 21 overlapped ticks are ≈{neff:.1f} independent averages "
                     f"(Welch), so E[γ̂²] = {model:.4f} at true {g2true:.4f} (Carter); OSM read {mean_g2o:.4f}")
            row += [g2true, mean_g2a, mean_g2o, model]
        else:
            row += [None, mean_g2a, mean_g2o, None]
        # ---- magnitude in noise: H1 unbiased, OSM's mean of ratios biased high
        if snr is not None:
            ma = med(dsp.db(Ha[bias_band]))
            self.add(name, "ac2 H1 |H| vs analytic 0 dB, median 1–20 kHz", ma, "dB", "mag_h1_noise_db",
                     "noise uncorrelated with the reference leaves H1 unbiased")
            mo = float(np.mean(np.abs(H[okb])))
            em = expected_ratio_mean(snr)
            self.add(name, "OSM mean |M|/|R| vs its expected bias", 20 * math.log10(mo) - 20 * math.log10(em), "dB",
                     "mag_osm_bias_db",
                     f"OSM averages |M|/|R|: E = {20 * math.log10(em):+.2f} dB at SNR {snr:g} dB (not 0 dB); "
                     f"OSM read {20 * math.log10(mo):+.2f} dB (linear mean of its bins 1–20 kHz)")
            self.rows["noise"].append([name, snr, ma, 20 * math.log10(mo), 20 * math.log10(em), g2true,
                                       mean_g2a, mean_g2o, expected_g2(g2true, neff)])
        self.rows["tf"].append(row)
        if (d / "tf-raw.csv").exists() and (d / "osm-raw.json").exists():
            self.uncompensated(name, d, x, settings, truth is not None and case or None)
        # ---- delay
        fa = first_arrival(ac2info.get("delay_find"))
        est = osm.get("estimated")
        tau_a = L["tau_a"]
        tau_o = tau_osm * fs if sl.sum() > 8 else None
        true_d = case.delay_samples if (case is not None and case.biquad is None and case.kind == "tf") else None
        if true_d is not None and snr is None:
            self.add(name, "OSM delay finder vs analytic", None if est is None else float(est) - true_d, "samples",
                     "delay_osm_samples", "OSM: integer argmax of IFFT(M/R) (whole samples); within ±0.5 is exact")
            self.add(name, "ac2 delay finder vs analytic", None if fa is None else fa - true_d, "samples",
                     "delay_ac2_samples", "ac2's first arrival, sub-sample")
            self.add(name, "OSM phase slope vs analytic", None if tau_o is None else tau_o - true_d, "samples",
                     "slope_samples", f"line through OSM's phase {SLOPE_BAND[0] / 1000:g}–{SLOPE_BAND[1] / 1000:g} kHz")
            self.add(name, "ac2 phase slope vs analytic", None if tau_a is None else tau_a - true_d, "samples",
                     "slope_samples", "line through ac2's columns, inserted delay put back")
        if fa is not None and est is not None:
            self.add(name, "ac2 delay finder vs OSM's", fa - float(est), "samples", "delay_vs_osm_samples",
                     "ac2 sub-sample vs OSM whole samples: within ±0.5 they agree"
                     + ("" if case is not None else " (a room's first arrival vs OSM's strongest peak)"))
        self.rows["delay"].append([name, true_d, est, fa, tau_o, tau_a, applied_ms * fs / 1000, osm_delay])
        self.series[name] = {"fc": fc, "ac2": Ha, "osm": Ho, "g2a": g2a, "g2o": g2o, "truth": truth}

    def uncompensated(self, name, d, x, settings, case):
        """Context, not judged: both TFs before any delay was set. ac2's MTW windows at HF
        are a few ms, so a flight time decorrelates their ends (γ² falls, the estimates
        scatter with the few averages there); OSM's 0.68 s window barely notices 0.5 ms
        in its phase, but its mean of |M|/|R| scatters per bin the same way."""
        osm_raw = json.loads((d / "osm-raw.json").read_text())
        L = load_pair(d / "tf-raw.csv", osm_raw, x, settings, 0.0)
        fs, fc, Ha, Ho, g2a = L["fs"], L["fc"], L["Ha"], L["Ho"], L["g2a"]
        inb = (fc >= BAND[0]) & (fc <= min(BAND[1], 0.45 * fs)) & np.isfinite(Ha) & np.isfinite(Ho)
        ref = case.truth(fc) if case is not None else Ho
        what = "analytic" if case is not None else "OSM"
        ea = np.abs(dsp.db(Ha[inb]) - dsp.db(ref[inb]))
        pa = np.abs(phase_diff_deg(Ha[inb], ref[inb]))
        hf = inb & (fc >= 5000)
        self.add(name, f"uncompensated: ac2 |H| vs {what}, max (all columns)", float(np.nanmax(ea)), "dB", None,
                 f"before the finder's delay is set; ac2 γ² down to {np.nanmin(g2a[hf]):.3f} at 5–20 kHz", status="INFO")
        self.add(name, f"uncompensated: ac2 phase vs {what}, max", float(np.nanmax(pa)), "°", None,
                 "the decorrelated windows' scatter with few averages, not a bias", status="INFO")
        if case is not None:
            eo = np.abs(dsp.db(Ho[inb]) - dsp.db(ref[inb]))
            po = np.abs(phase_diff_deg(Ho[inb], ref[inb]))
            self.add(name, "uncompensated: OSM |M|/|R| vs analytic, max", float(np.nanmax(eo)), "dB", None,
                     "per-bin scatter of a mean of ratios, mean ≈ 0", status="INFO")
            self.add(name, "uncompensated: OSM phase vs analytic, max", float(np.nanmax(po)), "°", None, "",
                     status="INFO")
            if L["tau_a"] is not None:
                self.add(name, "uncompensated: ac2 phase slope vs analytic", L["tau_a"] - case.delay_samples,
                         "samples", None, "", status="INFO")

    # ------------------------------------------------------------ spectrum
    def spectrum_case(self, name: str, d: Path, osm: dict, case: osm_fixtures.Case, settings: dict, sigma: float | None):
        fs = float(osm["harness"]["sampleRate"])
        f, module, *_ = osm_arrays(osm)
        sp = read_ac2_csv(d / "spectrum.csv").freq
        fa, la = sp["freq_hz"], sp["mag_db"]
        n = 2 ** int(settings["fft"])
        if len(fa) < n // 2 or abs(fa[1] - fs / n) > 1e-6:
            self.notes.append(f"{name}: ac2's spectrum is not on OSM's bins ({len(fa)} points); skipped")
            return
        la = la[: n // 2]
        lo = 20 * np.log10(np.maximum(module, 1e-30))
        # ac2's dBFS: a sine of peak a reads 20·log10(a); OSM's module is the RMS a/√2
        to_ac2 = 20 * math.log10(math.sqrt(2))
        if case.sine:
            fsin, a = case.sine
            k = int(round(fsin * n / fs))
            truth = 20 * math.log10(a)
            self.add(name, "ac2 spectrum, bin-centred sine vs analytic", la[k] - truth, "dB", "spectrum_tone_db",
                     f"{fsin:.2f} Hz at {truth:.2f} dBFS (peak convention)")
            self.add(name, "OSM module + 3.01 dB vs analytic", lo[k] + to_ac2 - truth, "dB", "spectrum_tone_db",
                     "OSM reads the RMS (a full-scale sine is −3.01 dB there)")
            self.rows["spectrum"].append([name, "tone", f"{fsin:.2f} Hz", truth, la[k], lo[k], lo[k] + to_ac2])
        if sigma is not None:
            band = (f >= 100) & (f <= 0.45 * fs)
            # expected per-bin level of white noise: a CG-normalised window spreads σ² over
            # its noise bandwidth Σw²/(Σw)²·N bins, read in the peak convention (×4 power)
            w = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(n) / n)
            enbw = n * float(np.sum(w * w)) / float(np.sum(w)) ** 2
            truth = 10 * math.log10(4 * sigma ** 2 * enbw / n)
            ac2_mean = 10 * math.log10(np.mean(10 ** (la[band] / 10)))
            osm_mean = 20 * math.log10(np.mean(module[band]))
            osm_conv = osm_mean + to_ac2 - rayleigh_mean_db()
            self.add(name, "ac2 spectrum, white noise per bin vs analytic", ac2_mean - truth, "dB", "spectrum_noise_db",
                     f"power mean over bins 100 Hz–{0.45 * fs / 1000:g} kHz; expected {truth:.2f} dBFS per "
                     f"{fs / n:.3f} Hz bin (Hann ENBW {enbw:.3f} bins)")
            self.add(name, "OSM module (+3.01, +1.05 dB) vs analytic", osm_conv - truth, "dB", "spectrum_noise_db",
                     "OSM averages amplitudes linearly: a Rayleigh mean reads √(π/4) (−1.05 dB) under the power mean")
            self.rows["spectrum"].append([name, "white noise", f"σ = {sigma:g}", truth, ac2_mean, osm_mean, osm_conv])

    # ------------------------------------------------------------ results
    def finish(self, manifest: dict, source: str) -> dict:
        r = self.rows
        self.tables["OSM settings (both analysers, per case)"] = {
            "columns": ["case", "OSM", "ac2 transfer", "ac2 spectrum", "fs"], "rows": r["settings"],
            "note": "OSM ticks every round(0.08·fs) samples and averages ticks; ac2's TF is its fixed MTW ladder "
                    "(no FFT size or window to match), its spectrum FIFO spans the audio OSM's FIFO spans."}
        self.tables["Transfer: worst column (dB / deg) and coherence"] = {
            "columns": ["case", "ac2−truth dB", "OSM−truth dB", "ac2−truth °", "OSM−truth °", "ac2−OSM dB",
                        "ac2−OSM °", "true γ²", "ac2 γ²", "OSM γ²", "OSM E[γ̂²]"],
            "rows": [[x if isinstance(x, str) else _fmt(x) for x in row] for row in r["tf"]],
            "note": f"max |difference| over columns {BAND[0]:g} Hz–{BAND[1] / 1000:g} kHz where γ² ≥ {GATE_G2}; "
                    "γ² means are over 1–20 kHz."}
        self.tables["Masked bins and columns"] = {
            "columns": ["case", "OSM bins: ref > 70 dB below peak", "OSM NaN-phase bins", "ac2 gap columns",
                        "columns below γ² gate", "columns judged"], "rows": r["masked"]}
        self.tables["Delay (samples)"] = {
            "columns": ["case", "truth", "OSM finder", "ac2 finder", "OSM slope", "ac2 slope", "ac2 inserted",
                        "OSM --delay"], "rows": [[x if isinstance(x, str) else _fmt(x, 3) for x in row] for row in r["delay"]]}
        if r["noise"]:
            self.tables["Noise: magnitude bias and coherence"] = {
                "columns": ["case", "SNR dB", "ac2 H1 dB", "OSM mean ratio dB", "expected OSM dB", "true γ²", "ac2 γ²",
                            "OSM γ²", "OSM E[γ̂²]"],
                "rows": [[x if isinstance(x, str) else _fmt(x, 4) for x in row] for row in r["noise"]]}
        if r["spectrum"]:
            self.tables["Spectrum (dBFS per bin)"] = {
                "columns": ["case", "signal", "", "analytic (ac2 convention)", "ac2", "OSM module", "OSM converted"],
                "rows": [[x if isinstance(x, str) else _fmt(x, 3) for x in row] for row in r["spectrum"]]}
        counts: dict[str, int] = {}
        for c in self.checks:
            counts[c.status] = counts.get(c.status, 0) + 1
        return {"source": source, "fixture": False, "manifest": manifest, "summary": counts,
                "checks": [asdict(c) for c in self.checks], "tables": self.tables, "notes": self.notes}


def _slug(title: str) -> str:
    return "_".join("".join(ch if ch.isalnum() else " " for ch in title.split(",")[0]).split()).lower()


def _fmt(x, nd=3):
    if x is None:
        return "—"
    if isinstance(x, (int, np.integer)):
        return str(int(x))
    x = float(x)
    return "—" if not math.isfinite(x) else f"{x:.{nd}f}"


# ---------------------------------------------------------------- recordings


def cut_recording(src: Path, cols: list[int], dst: Path, guard_s: float = 0.5) -> tuple[int, float]:
    """The span of a rig recording where the reference is playing (within 10 dB of its median
    active level, a guard trimmed off each end), as a meas/ref pair: both analysers end on
    signal, not on the silence a recording starts and stops with."""
    fs, x = wav.read(src)
    m, r = x[:, cols[0]], x[:, cols[1]]
    blk = int(fs * 0.05)
    nb = len(r) // blk
    lv = 10 * np.log10(np.mean(r[: nb * blk].reshape(nb, blk) ** 2, axis=1) + 1e-30)
    act = lv > np.median(lv[lv > lv.max() - 40]) - 10
    idx = np.flatnonzero(act)
    a, b = (idx[0] * blk + int(guard_s * fs), (idx[-1] + 1) * blk - int(guard_s * fs))
    wav.write(dst, fs, np.stack([m[a:b], r[a:b]], axis=1))
    return b - a, fs


# ---------------------------------------------------------------- the stage


def runs_dir() -> Path:
    """Where rig-run.sh puts fetched runs (the same rule as that script)."""
    if os.environ.get("CROSSCHECK_RUNS"):
        return Path(os.environ["CROSSCHECK_RUNS"])
    if Path("/work/ac2-crosscheck/runs").is_dir():
        return Path("/work/ac2-crosscheck/runs")
    return HERE / "runs"


def run_stage(out: Path, cfg_path: Path | None = None, tolerances: dict | None = None, cases: list[str] | None = None,
              recordings: bool = True, plots: bool = True) -> tuple[int, dict | None, str]:
    """Runs the stage into `out`; returns (exit code, results or None, a line for the user).
    A missing harness or ac2 binary is SKIP (exit 0)."""
    from . import report
    cfg = tomllib.loads((cfg_path or CONFIG).read_text())
    tol = tolerances or tomllib.loads(TOLERANCES.read_text())
    try:
        harness, bin_dir = find_tools(cfg)
    except Skip as e:
        return 0, None, f"osm: SKIP: {e}"
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    base = dict(cfg.get("settings", {}))
    want = cases if cases is not None else list(cfg.get("cases", {}).get("run", []))
    an = OsmAnalysis(tol)
    stage_rows = {}
    ver = subprocess.run([str(bin_dir / "ac2d"), "--version"], capture_output=True, text=True).stdout.strip()
    manifest = {"started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "rig": "none (offline)",
                "ac2_version": ver, "flags": {"harness": str(harness), "cases": want}}
    with tempfile.TemporaryDirectory(prefix="xc-osm-") as tmp, private_daemon(bin_dir, Path(tmp)) as (ac2, recdir):
        jobs: list[tuple[str, Path, osm_fixtures.Case | None, dict, bool]] = []
        for cname in want:
            case = osm_fixtures.CASES[cname]
            d = out / "osm" / cname
            d.mkdir(parents=True, exist_ok=True)
            osm_fixtures.generate(case, d / "pair.wav")
            jobs.append((cname, d, case, dict(base), case.align))
        for rc in (cfg.get("recordings", []) if recordings else []):
            src = runs_dir() / os.path.expanduser(rc["wav"])  # an absolute path stays as it is
            if not src.exists():
                an.notes.append(f"recording {rc['name']}: {src} not found, skipped")
                continue
            d = out / "osm" / rc["name"]
            d.mkdir(parents=True, exist_ok=True)
            cut_recording(src, list(rc.get("columns", [0, 1])), d / "pair.wav")
            jobs.append((rc["name"], d, None, dict(base), bool(rc.get("align"))))
        for name, d, case, s, align in jobs:
            try:
                fs, x = wav.read(d / "pair.wav")
                ac2info = run_ac2(ac2, recdir, name, d / "pair.wav", len(x), s, d, align)
                if align:
                    # OSM's operator sets OSM's own finder result (whole samples) as its delay
                    raw = run_harness(harness, d / "pair.wav", d / "osm-raw.json", s)
                    s["osm_delay"] = int(raw.get("estimated") or 0)
                osm = run_harness(harness, d / "pair.wav", d / "osm.json", s)
                an.rows["settings"].append([
                    name,
                    f"FFT{s['fft']} {s['window']}, FIFO {s['osm_average']} ticks (hop {osm['harness']['hop']})"
                    + (f", --delay {s['osm_delay']} (its finder)" if s.get("osm_delay") else ""),
                    f"MTW, --blocks {s['ac2_tf_blocks']}" + (", its finder's delay inserted" if align else ""),
                    f"--fft {2 ** int(s['fft'])}samples --window {s['window']} --average fifo:{ac2info['spectrum_fifo']}",
                    f"{fs:g}"])
                if case is None or case.kind == "tf":
                    an.tf_case(name, d, x, osm, ac2info, case, s)
                sigma = None
                if case is not None and case.kind == "tf" and case.snr_db is None and case.biquad is None \
                        and case.delay_samples == 0 and case.polarity == 1:
                    sigma = 10 ** (case.level_dbfs / 20)
                if case is not None and (case.kind == "spectrum" or sigma is not None):
                    an.spectrum_case(name, d, osm, case, s, sigma)
                stage_rows[name] = {"outcome": "ran", "detail": case.note if case else f"recording {d / 'pair.wav'}"}
            except (Ac2Error, RuntimeError, subprocess.TimeoutExpired) as e:
                stage_rows[name] = {"outcome": "error", "detail": str(e)[:300]}
                an.checks.append(Check(id=f"osm.{name}.ran", group=f"osm {name}", path=name, title="case ran", value=None, unit="", tol=None,
                                       status="FAIL", meaning=str(e)[:300]))
        shutil.copyfile(Path(tmp) / "ac2.log", out / "osm" / "ac2.log") if (Path(tmp) / "ac2.log").exists() else None
    manifest["stages"] = stage_rows
    res = an.finish(manifest, str(out))
    # report.write's plots are the rig stages' (paths and sources); this stage draws its own
    path = report.write(res, an, out / "report", plots=False)
    if plots:
        _plots(an, out / "report", path)
    s = res["summary"]
    return (1 if s.get("FAIL") else 0), res, f"{path}: " + ", ".join(f"{k} {s.get(k, 0)}" for k in report.STATUS_ORDER)


def _plots(an: OsmAnalysis, out: Path, md: Path):
    try:
        import matplotlib
        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
    except ImportError:
        return
    pngs = []
    for name, s in an.series.items():
        fc = s["fc"]
        fig, (a1, a2, a3) = plt.subplots(3, 1, figsize=(10, 9), sharex=True)
        for k, lab in (("ac2", "ac2"), ("osm", "OSM"), ("truth", "analytic")):
            if s.get(k) is None:
                continue
            a1.semilogx(fc, dsp.db(s[k]), lw=0.8, label=lab)
            a2.semilogx(fc, np.rad2deg(np.angle(s[k])), lw=0.8)
        a3.semilogx(fc, s["g2a"], lw=0.8, label="ac2 γ²")
        a3.semilogx(fc, s["g2o"], lw=0.8, label="OSM γ² (γ squared)")
        a1.set_ylabel("dB")
        a2.set_ylabel("deg")
        a3.set_ylabel("γ²")
        a1.legend()
        a3.legend()
        a1.set_title(f"OSM stage: {name}")
        fig.tight_layout()
        fig.savefig(out / f"osm-{name}.png", dpi=100)
        plt.close(fig)
        pngs.append(f"osm-{name}.png")
    txt = md.read_text().replace("none (plots off)", "\n".join(f"![{p[:-4]}]({p})" for p in pngs) or "none")
    md.write_text(txt)
