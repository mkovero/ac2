"""Analysis rows against the DUT's analytic truth on a fabricated digital-path run directory."""
import json
import tomllib
from pathlib import Path

import numpy as np
import pytest

from crosscheck import baseline, dsp, dut, levels
from crosscheck.analyse import analyse

ROOT = Path(__file__).resolve().parent.parent
RIG = tomllib.loads((ROOT / "rigs" / "pupu.toml").read_text())
FS = 96000.0
AMP = levels.peak_amplitude(-10.0)


def _run(tmp: Path, sine_err=None, sweep_err=None, xruns=0) -> Path:
    coef = dut.coefficients(RIG["dut"], FS)
    tmp.mkdir(parents=True)
    pd = tmp / "dut"  # the path's directory
    d = pd / "dut"
    d.mkdir(parents=True)
    (d / "dut.json").write_text(json.dumps({"coefficients": coef, "fs": FS, "amp": AMP,
                                            "stop": {"xruns": xruns, "xruns_line": f"xruns {xruns}"}}))
    man = {"rig": "pupu", "mains_hz": 50.0, "flags": {"emit": "-10dbfs", "stages": ["dut"]},
           "paths": {"dut": {"kind": "digital", "mains_hz": None, "primary_sweep": "10Hz-5.5s"}}}
    (tmp / "manifest.json").write_text(json.dumps(man))
    sine_err = sine_err or {}
    tones = []
    for f in RIG["stages"]["sine"]["dut_hz"]:
        t = dut.analytic(float(f), AMP, coef, FS)
        tones.append({"f": float(f), "ratio_db": 0.0, "ratio_deg": 0.0, "meas_level_dbfs": -10.0, "ref_level_dbfs": -10.0,
                      "h_dbr": {str(k): v + sine_err.get((f, k), 0.0) for k, v in t.items()},
                      "floor_dbr": {str(k): -115.0 for k in t}})
    (pd / "sine").mkdir()
    (pd / "sine" / "results.json").write_text(json.dumps({"emit_dbfs": -10.0, "path": "dut", "fs": FS,
                                                            "tones": tones}))
    sw = pd / "ac2_sweep" / "10Hz-5.5s"
    sw.mkdir(parents=True)
    f = dsp.log_centres(10.0, 40000.0, 48)
    info = {"sample_rate": FS, "rate": 0.7, "duration": 5.8, "repeats": 1, "arrival": 0.0, "reference_level": 0.0}
    lines = ["# ac2 trace export v3", "# kind: sweep", "# sweep_info: " + json.dumps(info),
             "freq_hz,mag_db,phase_deg,h2_db,h2_floor_db,h3_db,h3_floor_db,h4_db,h4_floor_db,h5_db,h5_floor_db"]
    sweep_err = sweep_err or (lambda fc, k: 0.0)
    for fc in f:
        t = dut.analytic(float(fc), AMP, coef, FS)
        row = [fc, 0.0, 0.0]
        for k in range(2, 6):
            row += [t[k] + sweep_err(fc, k), -105.0] if k in t else [np.nan, np.nan]
        lines.append(",".join(f"{x:.9g}" for x in row))
    (sw / "trace.csv").write_text("\n".join(lines) + "\n")
    (sw / "info.json").write_text(json.dumps({"level_dbfs": -10.0, "from_hz": 10.0, "to_hz": 40000.0,
                                              "duration_s": 5.5, "repeats": 1}))
    return tmp


def _rows(res, prefix):
    return {c["id"]: c for c in res["checks"] if c["id"].startswith(prefix)}


def test_exact_readings_pass_and_offsets_are_judged(tmp_path):
    root = _run(tmp_path / "run", sine_err={(1040, 3): 0.2},
                sweep_err=lambda fc, k: 0.8 if k == 4 and 4000 < fc < 6000 else 0.0)
    res, _ = analyse(root)
    s = _rows(res, "dut.dut.h")
    assert s["dut.dut.h2.steady sine.1040"]["status"] == "PASS"
    assert abs(s["dut.dut.h3.steady sine.1040"]["value"] - 0.2) < 1e-6
    assert s["dut.dut.h3.steady sine.1040"]["status"] == "WARN"  # the method check: 0.1 / 0.3 dB
    assert s["dut.dut.h2.ac2 sweep 10Hz-5.5s.15"]["status"] == "PASS"
    assert s["dut.dut.h4.ac2 sweep 10Hz-5.5s.5000"]["status"] == "WARN"
    assert "Wiener" in s["dut.dut.h2.ac2 sweep 10Hz-5.5s.15"]["meaning"]  # 15 Hz: pre-filter −4.4 dB
    g = _rows(res, "dut.dut.grid")
    assert g["dut.dut.grid.h2.ac2 sweep 10Hz-5.5s"]["status"] == "PASS"
    assert g["dut.dut.grid.h4.ac2 sweep 10Hz-5.5s"]["status"] == "WARN"
    assert abs(g["dut.dut.grid.h4.ac2 sweep 10Hz-5.5s"]["value"] - 0.8) < 1e-6
    x = _rows(res, "dut.dut.xruns")["dut.dut.xruns"]
    assert x["status"] == "INFO" and x["value"] == 0
    # no mains on the digital path: no mains exclusions, no mains table
    assert not any("mains" in n for n in res["tables"])
    # the path-generic rows against the steady sine run too
    assert any(c["id"].startswith("dut.h2.ac2 sweep") for c in res["checks"])


def test_a_missed_harmonic_fails_and_xruns_warn(tmp_path):
    # the sweep reads H2 at its floor around 1 kHz although the truth is −50 dBr
    root = _run(tmp_path / "run", xruns=3, sweep_err=lambda fc, k: -56.0 if k == 2 and 900 < fc < 1200 else 0.0)
    res, _ = analyse(root)
    c = _rows(res, "dut.dut.h2.ac2 sweep 10Hz-5.5s.1040")["dut.dut.h2.ac2 sweep 10Hz-5.5s.1040"]
    assert c["status"] == "FAIL"
    assert _rows(res, "dut.dut.grid.h2.ac2")["dut.dut.grid.h2.ac2 sweep 10Hz-5.5s"]["status"] == "FAIL"
    assert _rows(res, "dut.dut.xruns")["dut.dut.xruns"]["status"] == "WARN"


def test_baseline_accepts_the_new_path(tmp_path):
    from crosscheck import report
    root = _run(tmp_path / "run")
    res, an = analyse(root)
    report.write(res, an, root / "report", plots=False)
    assert baseline.stage_level(res, "dut") == -10.0
    rc, blocks, _ = baseline.compare(root, tmp_path / "no-baselines")
    assert rc == 0 and "no baseline" in blocks[0]["skipped"]
    lines, written = baseline.write_baselines(root, tmp_path / "bl", force=True)
    assert written and written[0].name == "dut-10dbfs.json"


@pytest.mark.parametrize("f", [12.0, 40.5])
def test_lf_tones_carry_the_pre_filter_note(f):
    from crosscheck.analyse import _pre_note
    coef = dut.coefficients(RIG["dut"], FS)
    assert _pre_note(coef, FS, f) is not None
    assert _pre_note(coef, FS, 1040.0) is None
