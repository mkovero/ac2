"""What a run (or the 2026-10-07 fixtures) holds, loaded into one shape for the analysis.

Run directory layout (written by `run.py`, read here):

    manifest.json                 flags, levels, versions, stage outcomes
    cal/ac2_cal.json              `ac2 cal list --json`
    cal/store_excerpt.json        mic-input sensitivity + active curve points (ac2's store)
    cal/rew_input_cal.{before,applied}.json
    ambient/in1.wav, window.json, ac2_spl_{z,a,c}.jsonl, rew_spl_{Z,A,C}.json, rew_rta.json, ac2_rta.csv
    <path>/sine/results.json, noise.wav
    <path>/rew/rec.wav, play.json, {meas,ref}_{fr,fr_spl,ir,gd,dist,rt60,summary}.json
    <path>/ac2_sweep/<variant>/trace.csv, events.jsonl, rec.wav, rec.json, plan.json
    <path>/ac2_tf/trace.csv, rec.wav, rec.json
    <path>/dut/dut.json           digital path: the DUT's command, coefficients, truth tables, xruns
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from . import dsp, formats, wav

REFERENCE = Path(__file__).resolve().parent.parent / "reference"


@dataclass
class Raw:
    fs: float
    meas: np.ndarray
    ref: np.ndarray


@dataclass
class Sweep:
    name: str
    trace: formats.Ac2Trace
    level_dbfs: float
    raw: Raw | None = None
    done: dict | None = None
    plan: dict | None = None
    start_hz: float | None = None
    end_hz: float | None = None
    duration_s: float | None = None
    lf_harmonics: str = "standard"


@dataclass
class RewSet:
    label: str
    level_dbfs: float  # level the stimulus was played at (sine convention)
    meas_fr: formats.RewCurve | None = None
    ref_fr: formats.RewCurve | None = None  # None: a live measurement with the loopback as cal ref
    meas_fr_spl: formats.RewCurve | None = None
    meas_ir: formats.RewIR | None = None
    ref_ir: formats.RewIR | None = None
    meas_gd: formats.RewCurve | None = None
    ref_gd: formats.RewCurve | None = None
    meas_dist: formats.RewDistortion | None = None
    rt60: dict | None = None
    summary: dict | None = None


@dataclass
class PathData:
    name: str
    kind: str  # electrical | speaker | digital
    sweeps: dict[str, Sweep] = field(default_factory=dict)
    primary: str | None = None
    tf: formats.Ac2Trace | None = None
    tf_raw: Raw | None = None
    tf_blocks: int | None = None  # the live TF's FIFO blocks, from its stage's info.json
    rew: RewSet | None = None
    rew_live: RewSet | None = None
    rec: Raw | None = None  # the REW stage's recording (meas, ref)
    truth: dict | None = None
    noise: Raw | None = None
    notes: list[str] = field(default_factory=list)
    mains_hz: float | None = 50.0  # None: a path without mains (the digital DUT)
    dut: dict | None = None  # <path>/dut/dut.json: what the analytic truth is computed from


@dataclass
class RunData:
    root: Path
    paths: dict[str, PathData]
    ambient: dict | None = None
    cal: dict | None = None
    manifest: dict = field(default_factory=dict)
    fixture: bool = False


def _raw(path: Path, meas_col: int = 0, ref_col: int = 1) -> Raw | None:
    if not path.exists():
        return None
    fs, x = wav.read(path)
    return Raw(fs=fs, meas=x[:, meas_col], ref=x[:, ref_col])


def _json(p: Path):
    return json.loads(p.read_text()) if p.exists() else None


def _maybe(fn, p: Path):
    return fn(p) if p.exists() else None


def load(root) -> RunData:
    root = Path(root)
    if (root / "manifest.json").exists():
        return load_run(root)
    if (root / "rew_fr1.json").exists() and (root / "ac2_sweep30.csv").exists():
        return load_fixtures(root)
    raise FileNotFoundError(f"{root}: neither a run directory (manifest.json) nor the 2026-10-07 fixtures")


def load_fixtures(root: Path) -> RunData:
    """The hand run of 2026-10-07 on pupu, Xone path only."""
    p = PathData(name="xone", kind="electrical")
    for name, file, lvl in (("10Hz-5.5s -30 (ac2_sweep30)", "ac2_sweep30.csv", -30.0),
                            ("E0 10Hz-5.5s -30", "e123/E0.csv", -30.0),
                            ("E1 3Hz-5.5s -30", "e123/E1.csv", -30.0),
                            ("E2 10Hz-11s -30", "e123/E2.csv", -30.0),
                            ("10Hz-5.5s -50 (ac2_sweep1)", "ac2_sweep1.csv", -50.0)):
        if (root / file).exists():
            t = formats.read_ac2_csv(root / file)
            src = t.meta.get("source", "")
            s = Sweep(name=name, trace=t, level_dbfs=lvl)
            s.start_hz = 3.0 if "3 Hz" in src else 10.0
            s.end_hz = 40000.0
            s.duration_s = 11.0 if "11 s" in src else 5.5
            p.sweeps[name] = s
    p.primary = "10Hz-5.5s -30 (ac2_sweep30)"
    p.tf = _maybe(formats.read_ac2_csv, root / "ac2_tf_rewsweep.csv")
    p.rec = _raw(root / "rec1.wav")
    p.rew = RewSet(label="REW offline import of rec1 (-50 dBFS)", level_dbfs=-50.0,
                   meas_fr=formats.read_rew_curve(root / "rew_fr1.json"),
                   ref_fr=formats.read_rew_curve(root / "rew_fr2.json"),
                   meas_dist=_maybe(formats.read_rew_distortion, root / "rew_dist1.json"))
    p.rew_live = RewSet(label="REW live -30 dBFS (loopback as cal and timing ref)", level_dbfs=-30.0,
                        meas_fr=formats.read_rew_curve(root / "rew_fr3.json"),
                        meas_ir=_maybe(formats.read_rew_ir, root / "rew3_ir_lin.json"),
                        meas_gd=_maybe(formats.read_rew_curve, root / "rew3_group-delay.json"),
                        meas_dist=_maybe(formats.read_rew_distortion, root / "rew_dist3.json"))
    p.truth = json.loads((REFERENCE / "pupu-2026-10-07-xone-sine.json").read_text())
    p.notes.append("fixtures: the TF trace was captured while REW's -50 dBFS sweep played (rec1), "
                   "so its comparison is with the direct cross-spectrum of rec1")
    p.notes.append("fixtures: steady-sine truth is the documented table (reference/), not raw data")
    return RunData(root=root, paths={"xone": p}, fixture=True,
                   manifest={"source": "fixtures 2026-10-07 (hand run)"})


def mic_curve_in_columns(meta: dict) -> bool:
    """ac2's export names the mic on every trace; only "(curve: …, in the columns)" means
    the trace's magnitudes carry the curve's correction. A live TF does; a sweep gets the
    input's curve "applied after capture as a display edit, not in the columns": ac2 shows
    it corrected, but its exported columns are of the raw inputs."""
    mic = str(meta.get("mic", "")).lower()
    return "in the columns" in mic and "not in the columns" not in mic


def _remove_curve(mag_db: np.ndarray, f: np.ndarray, curve) -> np.ndarray:
    """Magnitudes as the raw inputs give them: the correction ac2 (and REW, which gets the
    same curve normalised at 1 kHz) subtracted taken back out. Phase is never touched."""
    return mag_db - dsp.mic_correction_db(curve, f)


def load_run(root: Path) -> RunData:
    man = json.loads((root / "manifest.json").read_text())
    cal = _json(root / "cal" / "store_excerpt.json")
    curve = (cal or {}).get("curve_points")
    paths = {}
    for pname, pinfo in man.get("paths", {}).items():
        d = root / pname
        if not d.exists():
            continue
        p = PathData(name=pname, kind=pinfo.get("kind", "electrical"),
                     mains_hz=pinfo["mains_hz"] if "mains_hz" in pinfo else man.get("mains_hz", 50.0))
        p.dut = _json(d / "dut" / "dut.json")
        sd = d / "ac2_sweep"
        if sd.exists():
            for v in sorted(x for x in sd.iterdir() if (x / "trace.csv").exists()):
                info = _json(v / "info.json") or {}
                s = Sweep(name=v.name, trace=formats.read_ac2_csv(v / "trace.csv"),
                          level_dbfs=info.get("level_dbfs", np.nan), done=_json(v / "done.json"),
                          plan=_json(v / "plan.json"), start_hz=info.get("from_hz"), end_hz=info.get("to_hz"),
                          duration_s=info.get("duration_s"), lf_harmonics=info.get("lf_harmonics") or "standard")
                rec_cols = info.get("rec_columns")  # [meas col, ref col] in ac2's raw file
                if rec_cols and (v / "rec.wav").exists():
                    s.raw = _raw(v / "rec.wav", rec_cols[0], rec_cols[1])
                p.sweeps[v.name] = s
            p.primary = pinfo.get("primary_sweep") or (next(iter(p.sweeps)) if p.sweeps else None)
        td = d / "ac2_tf"
        if (td / "trace.csv").exists():
            p.tf = formats.read_ac2_csv(td / "trace.csv")
            if (td / "info.json").exists():
                p.tf_blocks = json.loads((td / "info.json").read_text()).get("blocks")
            # every relative comparison (direct cross-spectra, steady sines, sweeps) is of the
            # raw inputs; the absolute-SPL rows put the correction back for all sources alike
            if curve and mic_curve_in_columns(p.tf.meta):
                b = p.tf.freq
                b["mag_db"] = _remove_curve(b["mag_db"], b["freq_hz"], curve)
                p.tf.meta["mic"] = str(p.tf.meta["mic"]).replace("in the columns", "taken out at load")
            info = _json(td / "info.json") or {}
            if info.get("rec_columns") and (td / "rec.wav").exists():
                p.tf_raw = _raw(td / "rec.wav", *info["rec_columns"])
        rd = d / "rew"
        if (rd / "meas_fr.json").exists():
            play = _json(rd / "play.json") or {}
            meas_fr = formats.read_rew_curve(rd / "meas_fr.json")
            if curve and play.get("cal_file"):
                # imported with the cal file applied (speaker): its dBFS response carries the curve
                meas_fr.mag = _remove_curve(meas_fr.mag, meas_fr.f, curve)
            p.rew = RewSet(label="REW offline import", level_dbfs=play.get("level_dbfs", np.nan),
                           meas_fr=meas_fr,
                           ref_fr=_maybe(formats.read_rew_curve, rd / "ref_fr.json"),
                           meas_fr_spl=_maybe(formats.read_rew_curve, rd / "meas_fr_spl.json"),
                           meas_ir=_maybe(formats.read_rew_ir, rd / "meas_ir.json"),
                           ref_ir=_maybe(formats.read_rew_ir, rd / "ref_ir.json"),
                           meas_gd=_maybe(formats.read_rew_curve, rd / "meas_gd.json"),
                           ref_gd=_maybe(formats.read_rew_curve, rd / "ref_gd.json"),
                           meas_dist=_maybe(formats.read_rew_distortion, rd / "meas_dist.json"),
                           rt60=_json(rd / "meas_rt60.json"), summary=_json(rd / "meas_summary.json"))
            p.rec = _raw(rd / "rec.wav")
        sine = _json(d / "sine" / "results.json")
        if sine:
            p.truth = sine
            plan = _json(d / "sine" / "plan.json")
            if plan:
                p.truth["plan"] = plan
        p.noise = _raw(d / "sine" / "noise.wav")
        paths[pname] = p
    amb = None
    ad = root / "ambient"
    if ad.exists():
        amb = {"dir": ad, "window": _json(ad / "window.json")}
    return RunData(root=root, paths=paths, ambient=amb, cal=cal, manifest=man)
