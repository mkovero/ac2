import json

import pytest

from crosscheck import baseline as B
from crosscheck import comparison as C


def test_generated_tables_are_current():
    """comparison.md's tables are those of the committed baselines: after a baseline update,
    run `python -m crosscheck comparison` and commit both."""
    current, _ = C.update(C.DOC, C.BASELINES, check=True)
    assert current, "comparison.md is stale: run `python -m crosscheck comparison`"


def e(value, status="PASS", unit="dB", tol=(0.03, 0.1)):
    return {"status": status, "value": value, "unit": unit, "tol": list(tol) if tol else None}


def test_cell_worst_judged_value_with_location_and_counts():
    m = C.matches_for({"checks": {
        "xone.mag.ac2 sweep 10Hz-5.5s|REW offline.20-100": e(0.02),
        "xone.mag.ac2 sweep 10Hz-5.5s|REW offline.100-1000": e(-0.05, "WARN"),
        "xone.mag.ac2 sweep 10Hz-5.5s|REW offline.1000-20000": e(None, "INCONCLUSIVE"),
        "xone.mag.ac2 sweep dist|REW offline.20-100": e(9.0),  # another sweep: not this row
    }}, C.RIG_ROWS[0][1])
    assert C.cell(m) == "−0.050 dB @ 100–1000 Hz (WARN 1, PASS 1, INCONCLUSIVE 1)"
    # a harmonic row shows the order; an INFO-only row shows its value; bounds only show counts
    h = C.matches_for({"checks": {"x.h3.REW offline import@60.5Hz": e(0.6, tol=(3, 6)),
                                  "x.h2.REW offline import@50Hz": e(-0.1, tol=(3, 6))}},
                      r"(?P<o>h[2-5])\.REW offline import@")
    assert C.cell(h) == "+0.600 dB @ H3 60.5 Hz (PASS 2)"
    assert C.cell([("", None, e(-3.4, "INFO", "µs", None))]) == "−3.40 µs (INFO 1)"
    assert C.cell([("", None, e(None, "INCONCLUSIVE"))]) == "INCONCLUSIVE 1"
    assert C.fmt_value(-1e-9, "dB") == "0.000 dB" and C.fmt_value(0.023, "rel") == "+2.3 %"


def test_render_from_a_baseline_directory(tmp_path):
    rig = {"schema": 1, "rig": "r", "stage": "xone", "level_dbfs": -30.0,
           "provenance": {"run": "20261007T114727Z", "ac2_build": "0.0.0+26388938b456", "rew_version": "5.40"},
           "summary": {"PASS": 1}, "checks": {"xone.level.ac2_vs_rew_meas_minus_ref": e(-0.0004, tol=(0.1, 0.3))}}
    osm = {"schema": 1, "rig": "none (offline)", "stage": "osm", "level_dbfs": None,
           "provenance": {"run": "osm-x", "ac2_build": "ac2d (build 0.0.0+befb61dc0bb2)", "osm_version": "v1.5.2",
                          "flags": {"cases": ["identity", "delay10_5"]}},
           "summary": {"PASS": 2, "INFO": 1},
           "checks": {"osm.delay10_5.osm_delay_finder_vs_analytic": e(-0.5, unit="samples", tol=(0.5, 1)),
                      "osm.identity.ac2_tf_h_vs_analytic": e(0.0),
                      "osm.delay48.uncompensated_ac2_h_vs_analytic": e(0.417, "INFO", tol=None)}}
    older = {**rig, "level_dbfs": -50.0, "provenance": {**rig["provenance"], "run": "20261006T000000Z"},
             "checks": {"xone.level.ac2_vs_rew_meas_minus_ref": e(0.5, "WARN", tol=(0.1, 0.3))}}
    for sub, name, b in (("r", "xone-30dbfs.json", rig), ("r", "xone-50dbfs.json", older), ("host", "osm.json", osm)):
        (tmp_path / sub).mkdir(exist_ok=True)
        (tmp_path / sub / name).write_text(B.dumps(b))
    out = C.render(tmp_path)
    assert out.startswith(C.BEGIN) and out.rstrip().endswith(C.END)
    # only the newest baseline of a rig and stage is shown
    assert "| ac2 meas÷ref vs REW (meas − ref), absolute level | 0.000 dB (PASS 1) |" in out  # −0.0004 rounds to 0
    assert "xone −30 dBFS (20261007T114727Z, ac2 2638893)" in out and "−50 dBFS" not in out
    # OSM cases in the planned order, then the rest; INFO-only checks not tabled
    assert out.index("| identity |") < out.index("| delay10_5 |")
    assert "| delay10_5 |  | −0.500 |" in out
    assert "+0.417" not in out
    doc = tmp_path / "doc.md"
    doc.write_text(f"intro\n{C.BEGIN}\nold\n{C.END}\noutro\n")
    current, _ = C.update(doc, tmp_path, check=True)
    assert not current and "old" in doc.read_text()
    C.update(doc, tmp_path)
    assert doc.read_text() == "intro\n" + out + "outro\n"
    assert C.update(doc, tmp_path, check=True)[0]


def test_missing_markers_refused(tmp_path):
    doc = tmp_path / "doc.md"
    doc.write_text("no markers\n")
    with pytest.raises(ValueError, match="marker"):
        C.update(doc, tmp_path)
