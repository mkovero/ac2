import json
from pathlib import Path

import pytest

from crosscheck import baseline as B

TOL = {"compare": {"pass_fraction": 0.5, "step": {"dB": 0.02, "°": 0.1, "rel": 0.02, "µs": 0.2, "": 0.01}}}
RUNS = Path("/work/ac2-crosscheck/runs")


def chk(id_, group, value, status="PASS", unit="dB", tol=(0.05, 0.2), path="xone"):
    return {"id": id_, "group": group, "path": path, "title": "", "value": value, "unit": unit,
            "tol": list(tol) if tol else None, "status": status, "meaning": "", "detail": {}}


def results(checks, emit="-50dbfs", emit_speaker=None):
    return {"source": "x", "fixture": False, "summary": {},
            "manifest": {"rig": "pupu", "started": "2026-10-07T11:34:33Z", "ac2_version": "0.0.0+abc",
                         "rew_version": "5.40", "flags": {"emit": emit, "emit_speaker": emit_speaker},
                         "paths": {"xone": {"kind": "electrical"}, "genelec": {"kind": "speaker"}}},
            "checks": checks, "tables": {}, "notes": []}


def write_run(root: Path, name: str, res: dict, tones=None) -> Path:
    d = root / name
    (d / "report").mkdir(parents=True)
    (d / "report" / "results.json").write_text(json.dumps(res))
    if tones:
        (d / "xone" / "sine").mkdir(parents=True)
        (d / "xone" / "sine" / "results.json").write_text(
            json.dumps({"tones": [{"f": f, "f_requested": r} for f, r in tones]}))
    return d


def test_tone_keys_use_the_requested_frequency():
    nom = {"xone": {47.8: 50.0, 97.05: 100.0}}
    k, f, fn = B.check_key(chk("xone.sine_mag.REW offline.47.8", "magnitude vs sine", 0.0), nom)
    assert (k, f, fn) == ("xone.sine_mag.REW offline@50Hz", 47.8, 50.0)
    nom2 = {"xone": {51.275: 50.0}}
    assert B.check_key(chk("xone.h2.ac2 sweep 10Hz-5.5s.51.275", "harmonics", 0.0), nom2)[0] == \
        "xone.h2.ac2 sweep 10Hz-5.5s@50Hz"
    # a decimal nominal and a sweep name with a decimal in it
    assert B.check_key(chk("xone.gd.ac2 ±1/12-oct fit.31.5", "group delay", 0.0), {"xone": {31.5: 31.5}})[0] == \
        "xone.gd.ac2 ±1/12-oct fit@31.5Hz"
    # band ranges and named checks keep their id
    for i, g in (("xone.mag.ac2 sweep 10Hz-5.5s|REW offline.20-100", "magnitude"),
                 ("xone.delay.ac2_arrival.10Hz-5.5s", "delay"), ("xone.level.ac2_ref_vs_sine", "level")):
        assert B.check_key(chk(i, g, 0.0), nom)[0] == i


def test_old_results_get_keys_from_the_run_sine_results(tmp_path):
    r = results([chk("xone.sine_mag.REW offline.97.05", "magnitude vs sine", 0.01)])
    d = write_run(tmp_path, "run", r, tones=[(97.05, 100.0)])
    res = B.load_results(d)
    assert res["checks"][0]["key"] == "xone.sine_mag.REW offline@100Hz"


def test_moved_tones_match_by_nominal_or_nearest():
    a = B.make_baseline(B.annotate(results([chk("xone.sine_mag.REW offline.51.275", "magnitude vs sine", 0.0)]),
                                   {"xone": {51.275: 50.0}}), "xone", "a")
    b = B.make_baseline(B.annotate(results([chk("xone.sine_mag.REW offline.47.8", "magnitude vs sine", 0.0)]),
                                   {"xone": {47.8: 50.0}}), "xone", "b")
    assert list(a["checks"]) == list(b["checks"])
    # without the plan (fixtures, older runs) the played frequencies differ; nearest within 6 % pairs them
    c = B.make_baseline(B.annotate(results([chk("xone.sine_mag.REW offline.47.8", "magnitude vs sine", 0.0),
                                            chk("xone.sine_mag.REW offline.97.05", "magnitude vs sine", 0.0)]),
                                   {}), "xone", "c")
    pairs, missing, new = B.match(a["checks"], c["checks"])
    assert pairs == [("xone.sine_mag.REW offline@50Hz", "xone.sine_mag.REW offline@47.8Hz")]
    assert missing == [] and new == ["xone.sine_mag.REW offline@97.05Hz"]


def test_status_and_value_changes():
    base = B.make_baseline(B.annotate(results([
        chk("xone.level.ac2_ref_vs_sine", "level", -0.006, tol=(0.1, 0.3)),
        chk("xone.mag.a|b.20-100", "magnitude", 0.010),
        chk("xone.mag.a|b.100-1000", "magnitude", 0.010),
        chk("xone.delay.rew.REW offline import.IR peak", "delay", 5.0, status="INFO", unit="µs", tol=None),
        chk("xone.etc", "ETC", 1.0, tol=(2.0, 5.0)),
    ]), {}), "xone", "base")
    cur = B.make_baseline(B.annotate(results([
        chk("xone.level.ac2_ref_vs_sine", "level", -0.125, status="WARN", tol=(0.1, 0.3)),
        chk("xone.mag.a|b.20-100", "magnitude", 0.040),  # 0.03 > max(0.02, 0.5·0.05)
        chk("xone.mag.a|b.100-1000", "magnitude", 0.024),  # 0.014 within the step
        chk("xone.delay.rew.REW offline import.IR peak", "delay", 9.0, status="INFO", unit="µs", tol=None),
        chk("xone.coherence.tf", "coherence", 1.0, unit="", tol=None, status="INFO"),
    ]), {}), "xone", "cur")
    d = B.diff(base, cur, TOL)
    assert [r["key"] for r in d["worse"]] == ["xone.level.ac2_ref_vs_sine"]
    assert d["better"] == []
    assert sorted(r["key"] for r in d["moved"]) == ["xone.level.ac2_ref_vs_sine", "xone.mag.a|b.20-100"]
    assert [r["key"] for r in d["drift"]] == ["xone.delay.rew.REW offline import.IR peak"]
    assert d["missing"] == ["xone.etc"] and d["new"] == ["xone.coherence.tf"]
    assert B.gates(d)
    # the reverse: a cleared WARN is better and, its value having moved, still flagged
    r = B.diff(cur, base, TOL)
    assert [x["key"] for x in r["better"]] == ["xone.level.ac2_ref_vs_sine"] and r["worse"] == []
    same = B.diff(base, base, TOL)
    assert not B.gates(same)


def test_levels_are_kept_apart(tmp_path):
    root = tmp_path / "baselines"
    r30 = results([chk("xone.level.ac2_ref_vs_sine", "level", 0.0, tol=(0.1, 0.3))], emit="-30dbfs")
    lines, written = B.write_baselines(write_run(tmp_path, "r30", r30), root)
    assert [p.name for p in written] == ["xone-30dbfs.json"]
    r50 = results([chk("xone.level.ac2_ref_vs_sine", "level", 0.5, status="FAIL", tol=(0.1, 0.3))])
    d50 = write_run(tmp_path, "r50", r50)
    rc, blocks, path = B.compare(d50, root, tolerances=None, out=tmp_path / "o1")
    assert rc == 0 and "no baseline" in blocks[0]["skipped"]
    # an explicit file of another level is refused too
    rc, blocks, _ = B.compare(d50, root / "pupu" / "xone-30dbfs.json", out=tmp_path / "o2")
    assert rc == 0 and "not comparable" in blocks[0]["skipped"]
    assert (tmp_path / "o2" / "compare.md").exists()


def test_baseline_refuses_fails_unless_forced(tmp_path):
    root = tmp_path / "baselines"
    r = results([chk("xone.level.ac2_ref_vs_sine", "level", 0.5, status="FAIL", tol=(0.1, 0.3)),
                 chk("genelec.etc", "ETC", 1.0, tol=(2.0, 5.0), path="genelec")], emit_speaker="-50dbfs")
    d = write_run(tmp_path, "r", r)
    with pytest.raises(ValueError, match="FAIL"):
        B.write_baselines(d, root)
    # a stage without FAILs can be taken alone
    _, w = B.write_baselines(d, root, stages=["genelec"])
    assert [p.name for p in w] == ["genelec-50dbfs.json"]
    _, w = B.write_baselines(d, root, force=True)
    assert sorted(p.name for p in w) == ["genelec-50dbfs.json", "xone-50dbfs.json"]
    b = json.loads((root / "pupu" / "xone-50dbfs.json").read_text())
    assert b["level_dbfs"] == -50.0 and b["provenance"]["run"] == "r" and b["provenance"]["ac2_build"] == "0.0.0+abc"
    # a run compared with its own baseline gates nothing
    rc, _, _ = B.compare(d, root, out=tmp_path / "o")
    assert rc == 0


@pytest.mark.skipif(not (RUNS / "20261007T113432Z").exists(), reason="pupu runs not on this host")
def test_real_runs_share_tone_keys():
    a = B.load_results(RUNS / "20261007T100854Z")  # 50 Hz played at 51.275, 100 at 101.3
    b = B.load_results(RUNS / "20261007T113432Z")  # 47.8 and 97.05
    ka = {c["key"] for c in a["checks"] if c["path"] == "xone"}
    kb = {c["key"] for c in b["checks"] if c["path"] == "xone"}
    assert "xone.sine_mag.REW offline@50Hz" in ka & kb
    assert "xone.h2.ac2 sweep 10Hz-5.5s@100Hz" in ka & kb
    assert not any(k.endswith(("@51.275Hz", "@47.8Hz", "@97.05Hz", "@101.3Hz")) for k in ka | kb)


def osm_results(checks):
    return {"source": "x", "fixture": False, "summary": {},
            "manifest": {"rig": "none (offline)", "stage": "osm", "started": "2026-10-10T03:39:20Z",
                         "ac2_version": "ac2d 0.0.0 (build 0.0.0+abc)", "osm_version": "v1.5.2 Open Sound Meter",
                         "flags": {"cases": ["identity", "delay48"]}},
            "checks": checks, "tables": {}, "notes": []}


def test_osm_run_is_one_host_stage(tmp_path):
    root = tmp_path / "baselines"
    r = osm_results([chk("osm.identity.ac2_tf_h_vs_analytic", "osm identity", 0.0, path="identity"),
                     chk("osm.delay48.ac2_delay_finder_vs_analytic", "osm delay48", 0.0, unit="samples",
                         tol=(0.05, 0.5), path="delay48"),
                     chk("osm.delay48.uncompensated_ac2_phase_slope", "osm delay48", -0.32, status="INFO",
                         unit="samples", tol=None, path="delay48")])
    d = write_run(tmp_path, "osm-a", r)
    res = B.load_results(d)
    assert B.stages_of(res) == ["osm"] and B.stage_level(res, "osm") is None
    lines, written = B.write_baselines(d, root)
    assert written == [root / "host" / "osm.json"]
    b = json.loads(written[0].read_text())
    assert b["stage"] == "osm" and b["level_dbfs"] is None and len(b["checks"]) == 3
    assert b["provenance"]["osm_version"] == "v1.5.2 Open Sound Meter"
    rc, blocks, path = B.compare(d, root, out=tmp_path / "o")
    assert rc == 0 and blocks[0]["diff"]["matched"] == 3 and "OSM v1.5.2" in path.read_text()
    # the suite's samples step: 0.01, or half the 0.05 pass limit of ac2's finder
    r["checks"][1]["value"] = 0.03
    rc, blocks, _ = B.compare(write_run(tmp_path, "osm-b", r), root, out=tmp_path / "o2")
    assert rc == 1 and [m["key"] for m in blocks[0]["diff"]["moved"]] == ["osm.delay48.ac2_delay_finder_vs_analytic"]
    r["checks"][1]["value"] = 0.02
    rc, _, _ = B.compare(write_run(tmp_path, "osm-c", r), root, out=tmp_path / "o3")
    assert rc == 0


def test_every_baseline_unit_has_a_compare_step():
    steps, _ = B.compare_settings(B._tolerances(None))
    units = {e["unit"] for p in B.BASELINES.glob("*/*.json") for e in json.loads(p.read_text())["checks"].values()}
    assert units and units <= set(steps), units - set(steps)


def test_baseline_keeps_population_and_signed_parts():
    c = chk("xone.mag.a|b.100-1000", "magnitude", 0.12)
    c["detail"] = {"mean": -0.1234567891, "spread": 0.12, "n": 37, "delay_removed_s": 1e-6}
    nan = chk("xone.mag.a|b.20-100", "magnitude", 0.1)
    nan["detail"] = {"mean": float("nan"), "n": 4}
    b = B.make_baseline(B.annotate(results([c, nan, chk("xone.etc", "ETC", 1.0)]), {}), "xone", "r")
    e = b["checks"]["xone.mag.a|b.100-1000"]
    assert e["n"] == 37 and e["mean"] == -0.123457 and e["spread"] == 0.12
    assert "delay_removed_s" not in e
    assert b["checks"]["xone.mag.a|b.20-100"] == {"status": "PASS", "value": 0.1, "unit": "dB",
                                                   "tol": [0.05, 0.2], "n": 4}
    assert set(b["checks"]["xone.etc"]) == {"status", "value", "unit", "tol"}
    # one line per check survives the extra fields
    assert json.loads(B.dumps(b)) == b
