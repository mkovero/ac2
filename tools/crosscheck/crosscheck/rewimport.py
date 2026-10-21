"""`python -m crosscheck rew-import <run dir>`: redoes a run's REW import step on the rig from the
recordings the run saved (`<stage>/rew/rec_meas.wav`, `rec_ref.wav`): each channel against the
stimulus, then REW's `A / B`. Nothing is played. REW's input calibration is set from the run's
own `cal/` as the run set it, the soundcard cal from the run's rig file, and REW's settings are
put back afterwards. The run records it in `rew_reimport.json`. The import saves REW's
pictures for the graph comparison (`visual_rew.save_graphs`); `--rta` adds REW's RTA picture
of the rig's input (capture only) under `<run>/rta/rew`."""
from __future__ import annotations

import json
import os
import time
import tomllib
from pathlib import Path

from . import stages
from .rew import Rew, RewError

SIDECAR = "rew_reimport.json"


def stages_of(run: Path) -> list[str]:
    """The stages whose REW recordings the run kept."""
    return sorted(p.parent.parent.name for p in run.glob("*/rew/rec_meas.wav") if (p.parent / "rec_ref.wav").exists())


def reimport(run: Path, want: list[str] | None = None, rew: Rew | None = None, suite_commit: str | None = None,
             rta: bool = False) -> dict:
    run = Path(os.path.expanduser(str(run))).resolve()
    man = json.loads((run / "manifest.json").read_text())
    rig = tomllib.loads((run / "rig.toml").read_text())
    have = stages_of(run)
    want = want or have
    missing = [s for s in want if s not in have]
    if missing:
        raise FileNotFoundError(f"{run}: no REW recordings (rew/rec_meas.wav, rec_ref.wav) for {', '.join(missing)}")
    rew = rew or Rew(rig["rew"]["api"])
    ex = json.loads((run / "cal" / "store_excerpt.json").read_text())
    cf = run / "cal" / "mic_curve_rew.txt"
    cal_file = str(cf) if cf.exists() else ""
    keep = bool(rig["rew"].get("keep_measurements", False))
    sc = rig["rew"].get("soundcard_cal") or None
    rec = {"when": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
           "suite_commit": suite_commit or os.environ.get("CROSSCHECK_SUITE_COMMIT") or None,
           "rew_version": rew.version(), "run_rew_version": man.get("rew_version"),
           "what": "REW import redone from the run's recordings: meas and ref each against the stimulus, "
                   "then REW's Arithmetic A / B (meas ÷ ref); nothing played",
           "input_cal": {"dBFSAt94dBSPL": ex["rew_dbfs_at_94"], "fullScaleSineVrms": ex.get("full_scale_vrms", 1.0),
                         "calFilePath": cal_file},
           "soundcard_cal": sc,
           "stages": {}}
    snap = rew.snapshot()
    try:
        rew.set_input_cal(ex["rew_dbfs_at_94"], ex.get("full_scale_vrms", 1.0), cal_file)
        for s in want:
            d = run / s / "rew"
            play = json.loads((d / "play.json").read_text())
            speaker = bool(play.get("speaker"))
            ids = stages.rew_import(rew, d, play["stimulus"], s, speaker, keep=keep, soundcard_cal=sc, mic_cal_file=cal_file)
            rec["stages"][s] = {"stimulus": play["stimulus"], "apply_cal": speaker or bool(sc), "rew_ids": ids}
        if rta:
            # the graph comparison's RTA picture: REW's RTA on the rig's input, capture only
            from . import visual_rew
            rec["rta"] = visual_rew.save_rta(rew, run / "rta" / "rew")
    finally:
        errs = rew.restore(snap)
        rec["rew_restored"] = not errs
        if errs:
            rec["restore_errors"] = errs
        old = json.loads((run / SIDECAR).read_text()) if (run / SIDECAR).exists() else []
        (run / SIDECAR).write_text(json.dumps(old + [rec], indent=1))
    return rec


def main(a) -> int:
    try:
        rec = reimport(a.run, [s.strip() for s in a.stages.split(",") if s.strip()] if a.stages else None,
                       rta=bool(getattr(a, "rta", False)))
    except (FileNotFoundError, RewError) as e:
        print(f"stopped: {e}")
        return 2
    for s, x in rec["stages"].items():
        print(f"{s}: meas {x['rew_ids']['meas']}, ref {x['rew_ids']['ref']}, A / B {x['rew_ids']['div']}"
              + ("" if x["rew_ids"]["kept"] else f", deleted {len(x['rew_ids'].get('deleted', []))}"))
    if rec.get("rta"):
        print(f"rta: {rec['rta']['graphs']['rta']['file']} ({rec['rta']['export']}, limits {rec['rta']['limits']})")
    print(f"REW restored: {rec['rew_restored']}; recorded in {Path(a.run) / SIDECAR}")
    return 0 if rec["rew_restored"] else 1
