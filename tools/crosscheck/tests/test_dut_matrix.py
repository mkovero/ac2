"""The DUT matrix: designed polynomials, cases as paths, and the per-order coverage of the
harmonic checks (dutcov.py) on a fabricated two-case run."""
import json
import tomllib
from pathlib import Path

import numpy as np
import pytest

from crosscheck import baseline, dsp, dut, dutcov, dutrun, levels
from crosscheck.analyse import analyse

ROOT = Path(__file__).resolve().parent.parent
HOST = tomllib.loads((ROOT / "rigs" / "host.toml").read_text())
FS = 96000.0


@pytest.mark.parametrize("amp", [0.0316, 0.316, 0.9])
def test_chebyshev_poly_hits_its_targets_exactly(amp):
    c = dut.chebyshev_poly([-40.0, None, -50.0, -55.0], amp)
    h = dut.poly_harmonics(c, amp)
    assert abs(h[1] - amp) < 1e-12 * max(1.0, amp)
    got = 20 * np.log10(h[2:] / h[1] + 1e-300)
    assert np.allclose(got[[0, 2, 3]], [-40.0, -50.0, -55.0], atol=1e-9)
    assert got[1] < -250  # H3 absent


def test_chebyshev_poly_leaks_off_its_design_amplitude():
    # T_5(r·cos θ) puts r⁵ on H5 and the rest on H3 and H1: the targets hold only at a
    c = dut.chebyshev_poly([None, None, None, -40.0], 0.3)
    h = dut.poly_harmonics(c, 0.15)
    assert abs(20 * np.log10(h[5] / h[1]) - (-40.0 + 20 * np.log10(0.5 ** 4))) < 0.5
    assert h[3] > h[5]


def test_harmonics_dbfs_keep_their_distance_from_the_noise_at_every_level():
    for lv in (-10.0, -30.0):
        coef = dut.coefficients({"harmonics_dbfs": [-110.0, -110.0, -110.0, -110.0]}, FS, lv)
        t = dut.analytic(1000.0, levels.peak_amplitude(lv), coef, FS)
        assert all(abs(t[k] - (-110.0 - lv)) < 1e-9 for k in range(2, 6))


def test_coefficients_need_exactly_one_polynomial():
    with pytest.raises(ValueError):
        dut.coefficients({"poly": [0, 1], "harmonics_dbr": [-40.0]}, FS, -10.0)
    with pytest.raises(ValueError):
        dut.coefficients({}, FS, -10.0)
    with pytest.raises(ValueError):
        dut.coefficients({"harmonics_dbr": [-40.0]}, FS)  # designed: needs the level


def test_cases_become_paths_with_their_own_config():
    rig = tomllib.loads((ROOT / "rigs" / "host.toml").read_text())
    names = dutrun.expand_cases(rig)
    assert names == ["dut-loud", "dut-loud-hp", "dut-floor", "dut-floor-hp"]
    p = rig["paths"]["dut-loud-hp"]
    assert p["stage"] == "dut" and p["case"] == "loud-hp" and p["meas_in"] == rig["paths"]["dut"]["meas_in"]
    assert rig["stages"]["sine"]["dut-floor_hz"] == rig["stages"]["sine"]["dut_hz"]
    assert rig["stages"]["ac2_sweep"]["dut-floor"] == rig["stages"]["ac2_sweep"]["dut"]
    assert rig["stages"]["ac2_sweep"]["target_floor_dbr"]["dut-loud"] == -100.0
    dc = dutrun.dut_config(rig, "dut-loud-hp")
    assert dc["pre"][0]["type"] == "hp1" and "cases" not in dc and dc["client"] == "ac2-dut"
    assert dutrun.dut_config(rig, "dut-floor")["pre"] == rig["dut"]["pre"]  # the shared anti-alias filter


def test_a_case_replaces_the_shared_polynomial():
    rig = {"paths": {"dut": {"kind": "digital"}}, "stages": {},
           "dut": {"poly": [0, 1, 0.1], "cases": [{"name": "a", "harmonics_dbr": [-40.0]}]}}
    dutrun.expand_cases(rig)
    dc = dutrun.dut_config(rig, "dut-a")
    assert "poly" not in dc and dc["harmonics_dbr"] == [-40.0]
    rig["dut"]["cases"].append({"name": "a"})
    with pytest.raises(ValueError):
        dutrun.expand_cases(rig)


def test_no_cases_is_the_one_dut_path():
    rig = {"paths": {"dut": {}}, "stages": {}, "dut": {"poly": [0, 1]}}
    assert dutrun.expand_cases(rig) == ["dut"]
    assert dutrun.dut_config(rig, "dut") == {"poly": [0, 1]}


@pytest.mark.parametrize("lv", [-10.0, -30.0])
def test_host_matrix_truth_and_headroom(lv):
    rig = tomllib.loads((ROOT / "rigs" / "host.toml").read_text())
    amp = levels.peak_amplitude(lv)
    for n in dutrun.expand_cases(rig):
        dc = dutrun.dut_config(rig, n)
        coef = dut.coefficients(dc, FS, lv)
        want = dut.target_dbr(dc, lv)
        t = dut.analytic(1000.0, amp, coef, FS)
        # the 20 Hz high-pass still lowers 1 kHz by 0.002 dB, which the equal-level cases
        # amplify through the leakage of the higher orders
        assert all(abs(t[k] - want[k - 2]) < 0.05 for k in range(2, 6)), n
        assert dut.peak_out(amp, coef, FS) < 0.5
        for f in rig["stages"]["sine"]["dut_hz"]:
            assert 5 * f < FS / 2  # H5 of every tone has a truth


def test_host_anti_alias_filter_is_flat_at_the_tones_and_down_where_h5_folds():
    pre = dut.design_chain(HOST["dut"]["pre"], FS)
    g = 20 * np.log10(np.abs(dut.response(pre, [5000.0, 9600.0], FS)))
    assert abs(g[0]) < 0.02
    assert g[1] < -20.0


# ---------------------------------------------------------------- coverage


def _e(k, f, status, delta=None, model=False, src="ac2 sweep 10Hz-5.5s", level=-10.0, case="a"):
    return dutcov.entry("dut-" + case, case, src, k, f, status, delta, -60.0, 0.65 if model else 0.0, True, level)


def test_summary_keeps_model_limited_cells_out_of_the_verdict():
    es = [_e(2, 1000, "PASS", -0.05), _e(2, 2000, "WARN", 0.7), _e(2, 50, "FAIL", 3.0, model=True),
          _e(2, 5000, "INCONCLUSIVE"), _e(2, 100, "MISSING")]
    s = dutcov.summarise(es)
    assert (s["judged"], s["total"], s["inconclusive"], s["missing"], s["model_judged"]) == (3, 5, 1, 1, 1)
    assert s["status"] == "WARN" and s["worst"] == 0.7 and s["model_worst"] == 3.0
    assert s["worst_at"]["f"] == 2000
    assert dutcov.summarise([_e(3, 50, "PASS", 0.1, model=True)])["status"] == "INCONCLUSIVE"


def test_cells_and_grid():
    assert dutcov.cell([]) == ""
    assert dutcov.cell([_e(2, 1000, "PASS", -0.05), _e(2, 1000, "INCONCLUSIVE")]) == "1/2 -0.05"
    assert dutcov.cell([_e(2, 50, "PASS", 0.04, model=True)]) == "1/1 (+0.04) †"
    assert dutcov.cell([_e(2, 50, "FAIL", 0.9)]) == "1/1 +0.90 F"
    es = [_e(2, 1000, "PASS", 0.01), _e(2, 1000, "PASS", 0.02, level=-30.0),
          _e(2, 1000, "PASS", 0.0, src="steady sine"), _e(3, 50, "MISSING")]
    cols, rows = dutcov.grid(es, by_level=True)
    assert cols[:4] == ["H", "source", "level", "50 Hz"]
    assert [r[:3] for r in rows] == [["H2", "steady sine", "-10 dBFS"], ["H2", "ac2 sweep", "-10 dBFS"],
                                     ["H2", "ac2 sweep", "-30 dBFS"], ["H3", "ac2 sweep", "-10 dBFS"]]
    assert rows[3][-6:] == ["0/1", "0", "1", "—", "—", "INCONCLUSIVE"]


def _case_run(tmp: Path, cases: dict, drop=None) -> Path:
    """A fabricated run of DUT cases: exact steady-sine and sweep readings at the truth, the
    sweep's floor 40 dB under each reading; `drop(case, fc, k)` blanks a sweep column."""
    lv, amp = -10.0, levels.peak_amplitude(-10.0)
    tones = [50.0, 1000.0, 5000.0]
    man = {"rig": "host", "flags": {"emit": "-10dbfs", "stages": ["dut"]}, "paths": {}}
    for name, cfg in cases.items():
        pname = f"dut-{name}"
        man["paths"][pname] = {"kind": "digital", "mains_hz": None, "primary_sweep": "s", "stage": "dut",
                               "case": name}
        coef = dut.coefficients(cfg, FS, lv)
        pd = tmp / pname
        (pd / "dut").mkdir(parents=True)
        (pd / "dut" / "dut.json").write_text(json.dumps({"coefficients": coef, "fs": FS, "amp": amp,
                                                         "stop": {"xruns": 0}}))
        ts = []
        for f in tones:
            t = dut.analytic(f, amp, coef, FS)
            ts.append({"f": f, "ratio_db": 0.0, "ratio_deg": 0.0, "meas_level_dbfs": lv, "ref_level_dbfs": lv,
                       "h_dbr": {str(k): v for k, v in t.items()}, "floor_dbr": {str(k): -150.0 for k in t}})
        (pd / "sine").mkdir()
        (pd / "sine" / "results.json").write_text(json.dumps({"emit_dbfs": lv, "path": pname, "fs": FS,
                                                              "tones": ts}))
        sw = pd / "ac2_sweep" / "s"
        sw.mkdir(parents=True)
        info = {"sample_rate": FS, "rate": 0.7, "duration": 5.8, "repeats": 1, "arrival": 0.0, "reference_level": 0.0}
        lines = ["# ac2 trace export v3", "# kind: sweep", "# sweep_info: " + json.dumps(info),
                 "freq_hz,mag_db,phase_deg,h2_db,h2_floor_db,h3_db,h3_floor_db,h4_db,h4_floor_db,h5_db,h5_floor_db"]
        for fc in dsp.log_centres(10.0, 40000.0, 48):
            t = dut.analytic(float(fc), amp, coef, FS)
            row = [fc, 0.0, 0.0]
            for k in range(2, 6):
                gone = k not in t or k * fc > 40000.0 or (drop and drop(name, fc, k))
                row += [np.nan, np.nan] if gone else [t[k], t[k] - 40.0]
            lines.append(",".join(f"{x:.9g}" for x in row))
        (sw / "trace.csv").write_text("\n".join(lines) + "\n")
        (sw / "info.json").write_text(json.dumps({"level_dbfs": lv, "from_hz": 10.0, "to_hz": 40000.0,
                                                  "duration_s": 5.5, "repeats": 1}))
    (tmp / "manifest.json").write_text(json.dumps(man))
    return tmp


def test_case_run_coverage_and_one_baseline(tmp_path):
    from crosscheck import report
    hp = {"type": "hp1", "hz": 20.0}
    root = _case_run(tmp_path / "run", {"a": {"harmonics_dbr": [-40.0, -45.0, -50.0, -55.0]},
                                        "b": {"harmonics_dbr": [-40.0, -45.0, -50.0, -55.0], "pre": [hp]}},
                     drop=lambda case, fc, k: case == "a" and k == 3 and 900 < fc < 1100)
    res, an = analyse(root)
    cov = {c["id"]: c for c in res["checks"] if c["group"] == "dut coverage"}
    assert set(cov) == {f"dut.coverage.h{k}.{s}" for k in range(2, 6) for s in ("ac2 sweep", "steady sine")}
    h3 = cov["dut.coverage.h3.ac2 sweep"]
    s = h3["detail"]["summary"]
    # 2 cases × 3 tones; H3 of 5 kHz (15 kHz) is due; case a's 1 kHz column is gone
    assert (s["total"], s["missing"]) == (6, 1)
    assert s["model_judged"] == 1  # case b at 50 Hz: the 20 Hz pre-filter is −0.65 dB there
    assert h3["status"] == "PASS" and abs(h3["value"]) < 0.05
    sine = cov["dut.coverage.h2.steady sine"]["detail"]["summary"]
    assert (sine["judged"], sine["total"], sine["model_judged"]) == (6, 6, 0)
    assert "DUT coverage: harmonic order × tone, all cases" in res["tables"]
    report.write(res, an, root / "report", plots=False)
    # every case and the coverage rows are one stage, one baseline per level
    assert baseline.stages_of(res) == ["dut"]
    assert baseline.stage_level(res, "dut") == -10.0
    _, written = baseline.write_baselines(root, tmp_path / "bl", force=True)
    assert [p.name for p in written] == ["dut-10dbfs.json"] and written[0].parent.name == "host"
    # and the cells come back out of results.json for the multi-level grid
    cells = dutcov.entries_of(json.loads((root / "report" / "results.json").read_text()))
    assert len(cells) == sum(c["detail"]["summary"]["total"] for c in cov.values())


def test_a_failed_sine_stage_leaves_its_cells_missing(tmp_path):
    root = _case_run(tmp_path / "run", {"a": {"harmonics_dbr": [-40.0, -45.0, -50.0, -55.0]}})
    (root / "dut-a" / "sine" / "results.json").unlink()
    d = json.loads((root / "dut-a" / "dut" / "dut.json").read_text())
    d.update(level_dbfs=-10.0, tones=[{"f": 50.0}, {"f": 1000.0}])
    (root / "dut-a" / "dut" / "dut.json").write_text(json.dumps(d))
    res, _ = analyse(root)
    s = next(c for c in res["checks"] if c["id"] == "dut.coverage.h2.steady sine")["detail"]["summary"]
    assert (s["judged"], s["total"], s["missing"]) == (0, 2, 2)


def test_designed_polynomial_is_silent_at_rest():
    # the even T_k's constant is DC only; a DUT offset at rest would read as LF "noise"
    for h in ([-40.0, -45.0, -50.0, -55.0], [None, -60.0]):
        c = dut.chebyshev_poly(h, 0.316)
        assert c[0] == 0.0
        assert np.allclose(20 * np.log10(dut.poly_harmonics(c, 0.316)[2:len(h) + 2] / 0.316 + 1e-300)[
            [i for i, x in enumerate(h) if x is not None]], [x for x in h if x is not None], atol=1e-9)


def test_harmonics_show_as_the_direct_estimates_coherence_deficit():
    # a sweep through x + c·x² with the linear part 40 dB down above 4 kHz: there the band
    # power holds H2 of the sweep at f/2, which does not correlate with the reference at f,
    # so the power estimate's excess over the linear part is −10·log10 γ²
    fs, T = 48000.0, 4.0
    t = np.arange(int(fs * T)) / fs
    f1, f2 = 20.0, 20000.0
    L = T / np.log(f2 / f1)
    x = 0.5 * np.sin(2 * np.pi * f1 * L * (np.exp(t / L) - 1))
    X = np.fft.rfft(x)
    fx = np.fft.rfftfreq(len(x), 1 / fs)
    lin = np.fft.irfft(X * np.where(fx > 4000, 0.01, 1.0), len(x))
    y = lin + 0.05 * x ** 2
    cen = np.array([1000.0, 2000.0, 8000.0, 12000.0])
    H, coh = dsp.cross_spectrum_bands(y, x, fs, cen, 1 / 6)
    excess = -10 * np.log10(coh)
    Hl, _ = dsp.cross_spectrum_bands(lin, x, fs, cen, 1 / 6)
    true_excess = 20 * np.log10(np.abs(H) / np.abs(Hl))
    assert np.all(excess[:2] < 0.01)                     # linear part dominates: no gate
    assert np.all(excess[2:] > 1.0)                      # harmonics dominate: gated
    assert np.allclose(excess[2:], true_excess[2:], atol=0.5)


def test_noise_of_a_band_is_relative_to_its_linear_part():
    # A band whose output is mostly uncorrelated power (a nonlinear path's harmonics from f/k)
    # holds a linear part γ² of it: the noise is that much larger against H.
    from crosscheck import dsp
    rng = np.random.default_rng(3)
    fs, n = 48000, 1 << 16
    ref = rng.standard_normal(n)
    other = rng.standard_normal(n)
    noise = 1e-3 * rng.standard_normal(n)
    meas = 0.1 * ref + other + 1e-3 * rng.standard_normal(n)
    f = np.array([1000.0, 4000.0])
    _, coh = dsp.cross_spectrum_bands(meas, ref, fs, f, 1 / 6, delay_s=0.0)
    plain = dsp.band_noise_rel(meas, noise, fs, f, 1 / 6)
    lin = dsp.band_noise_rel(meas, noise, fs, f, 1 / 6, coh=coh)
    assert np.allclose(lin / plain, 1 / np.sqrt(coh), rtol=0.05)
    assert np.all(lin / plain > 4)
